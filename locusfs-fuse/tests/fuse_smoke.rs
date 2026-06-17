use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use locusfs_fuse::{FuseMountConfig, mount};
use locusfs_graph::{DynamicGraph, InMemoryProvider, LocusValue, NodeId, NodeKind, PropertyKey};

#[test]
#[ignore = "requires host /dev/fuse access"]
fn generic_nodes_props_and_relations_work_through_real_fuse_mount() {
    let mountpoint = Path::new("/tmp/locusfs-test");
    cleanup_mount(mountpoint);
    fs::create_dir_all(mountpoint).unwrap();

    let graph = test_graph();
    let _mount = mount(FuseMountConfig::new(mountpoint), graph.clone()).unwrap();
    wait_for_mount(mountpoint);

    let source = mountpoint.join("node/57");
    let target = mountpoint.join("node/6");
    let source_node = test_node("57");
    let target_node = test_node("6");
    let other_target_node = test_node("7");
    let title = PropertyKey::new("title").unwrap();
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();
    fs::create_dir(mountpoint.join("node/7")).unwrap();

    fs::write(source.join("title"), "value\n").unwrap();
    assert_eq!(fs::read_to_string(source.join("title")).unwrap(), "value\n");
    fs::write(target.join("title"), "target\n").unwrap();
    fs::write(mountpoint.join("node/7/title"), "other target\n").unwrap();

    symlink("../../node/6", source.join("linked-to")).unwrap();
    assert_eq!(
        fs::read_link(source.join("linked-to")).unwrap(),
        Path::new("../../node/6")
    );
    assert_eq!(
        fs::read_to_string(source.join("linked-to/title")).unwrap(),
        "target\n"
    );

    assert_poll_wakes_after_property_change(
        source.join("title"),
        graph.clone(),
        source_node.clone(),
        title.clone(),
        "updated direct\n",
    );
    assert_poll_wakes_after_property_change(
        source.join("linked-to/title"),
        graph.clone(),
        target_node,
        title,
        "updated through symlink\n",
    );
    assert_meta_watch_wakes_after_relation_change(
        mountpoint,
        graph.clone(),
        source_node,
        other_target_node,
        "other target\n",
    );

    fs::remove_file(source.join("linked-to")).unwrap();
    fs::remove_file(source.join("title")).unwrap();
    fs::remove_file(target.join("title")).unwrap();

    drop(_mount);
    cleanup_mount(mountpoint);
}

fn assert_meta_watch_wakes_after_relation_change(
    mountpoint: &Path,
    graph: DynamicGraph,
    source_node: NodeId,
    other_target_node: NodeId,
    expected: &str,
) {
    let data_path = mountpoint.join("node/57/linked-to/title");
    let watch_path = mountpoint.join("watch");
    let expected = expected.to_string();
    let (ready_sender, ready_receiver) = mpsc::channel();
    let watcher = thread::spawn(move || {
        let mut watch_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(watch_path)
            .unwrap();
        watch_file.write_all(b"/node/57/linked-to/title\n").unwrap();
        ready_sender.send(()).unwrap();

        wait_for_poll(&watch_file);
        let mut events = String::new();
        watch_file.read_to_string(&mut events).unwrap();
        assert!(
            events.contains("change"),
            "unexpected watch events: {events:?}",
        );
        assert_eq!(read_path_retrying(&data_path), expected);
    });

    ready_receiver.recv().unwrap();
    thread::sleep(Duration::from_millis(50));
    let relation = locusfs_graph::RelationName::new("linked-to").unwrap();
    graph
        .remove_link(&source_node, &relation, &test_node("6"))
        .unwrap();
    graph
        .set_link(&source_node, &relation, &other_target_node)
        .unwrap();

    watcher.join().unwrap();
}

fn test_graph() -> DynamicGraph {
    let kind = NodeKind::new("node").unwrap();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph.register_node_provider(provider.clone()).unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_property_mutation_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_relation_mutation_provider(kind, provider)
        .unwrap();
    graph
}

fn assert_poll_wakes_after_property_change(
    path: impl AsRef<Path>,
    graph: DynamicGraph,
    node: NodeId,
    key: PropertyKey,
    value: &str,
) {
    let path = path.as_ref().to_path_buf();
    let value = value.to_string();
    let expected = value.clone();
    let (ready_sender, ready_receiver) = mpsc::channel();
    let watcher = thread::spawn(move || {
        let mut file = fs::File::open(path).unwrap();
        assert!(!read_file(&mut file).unwrap().is_empty());
        ready_sender.send(()).unwrap();
        wait_for_poll(&file);
        assert_eq!(read_file(&mut file).unwrap(), expected);
    });

    ready_receiver.recv().unwrap();
    thread::sleep(Duration::from_millis(50));
    graph
        .set_property(
            &node,
            &key,
            LocusValue::String(value.trim_end().to_string()),
        )
        .unwrap();

    watcher.join().unwrap();
}

fn wait_for_poll(file: &fs::File) {
    let mut pollfd = libc::pollfd {
        fd: file.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pollfd, 1, 2_000) };
    assert_eq!(result, 1, "poll should wake within timeout");
    assert_ne!(pollfd.revents & libc::POLLIN, 0, "expected POLLIN");
}

fn read_file(file: &mut fs::File) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    Ok(value)
}

fn read_path_retrying(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_error = None;
    while Instant::now() < deadline {
        match fs::read_to_string(path) {
            Ok(value) => return value,
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
    panic!(
        "failed to read {} after poll wake: {:?}",
        path.display(),
        last_error
    );
}

fn test_node(local: &str) -> NodeId {
    NodeId::new(NodeKind::new("node").unwrap(), local).unwrap()
}

fn wait_for_mount(mountpoint: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if mountpoint.join("node").is_dir() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("mount did not become ready at {}", mountpoint.display());
}

fn cleanup_mount(mountpoint: &Path) {
    let _ = Command::new("fusermount3")
        .arg("-u")
        .arg(mountpoint)
        .status();
    let _ = fs::remove_dir_all(mountpoint);
}
