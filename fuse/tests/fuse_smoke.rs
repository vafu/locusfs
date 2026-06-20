use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use locusfs_fuse::{FuseMountConfig, mount};
use locusfs_graph::{DynamicGraph, InMemoryProvider, LocusValue, NodeId, NodeKind, PropertyKey};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires host /dev/fuse access"]
async fn generic_nodes_props_and_relations_work_through_real_fuse_mount() {
    let mountpoint = Path::new("/tmp/locusfs-test");
    cleanup_mount(mountpoint);
    fs::create_dir_all(mountpoint).unwrap();

    let graph = test_graph().await;
    let _mount = mount(FuseMountConfig::new(mountpoint), graph.clone())
        .await
        .unwrap();
    wait_for_mount(mountpoint).await;

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
    )
    .await;
    assert_poll_wakes_after_property_change(
        source.join("linked-to/title"),
        graph.clone(),
        target_node,
        title,
        "updated through symlink\n",
    )
    .await;
    assert_meta_watch_wakes_after_relation_change(
        mountpoint,
        graph.clone(),
        source_node,
        other_target_node,
        "other target\n",
    )
    .await;

    drop(_mount);
    cleanup_mount(mountpoint);
}

async fn assert_meta_watch_wakes_after_relation_change(
    mountpoint: &Path,
    graph: DynamicGraph,
    source_node: NodeId,
    other_target_node: NodeId,
    expected: &str,
) {
    let data_path = mountpoint.join("node/57/linked-to/title");
    let watch_path = mountpoint.join("watch");
    let expected = expected.to_string();
    let (ready_sender, mut ready_receiver) = mpsc::channel(1);
    let (changed_sender, mut changed_receiver) = mpsc::channel(1);
    let watcher = tokio::task::spawn_blocking(move || {
        let mut watch_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(watch_path)
            .unwrap();
        watch_file.write_all(b"/node/57/linked-to/title\n").unwrap();
        watch_file.seek(SeekFrom::Start(0)).unwrap();
        ready_sender.blocking_send(()).unwrap();

        changed_receiver.blocking_recv().unwrap();
        let mut events = String::new();
        watch_file.read_to_string(&mut events).unwrap();
        assert!(
            events.contains("change"),
            "unexpected watch events: {events:?}",
        );
        assert_path_value_retrying(&data_path, &expected);
    });

    ready_receiver.recv().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    let relation = locusfs_graph::RelationName::new("linked-to").unwrap();
    graph
        .remove_link(&source_node, &relation, &test_node("6"))
        .await
        .unwrap();
    graph
        .set_link(&source_node, &relation, &other_target_node)
        .await
        .unwrap();
    changed_sender.send(()).await.unwrap();

    watcher.await.unwrap();
}

async fn test_graph() -> DynamicGraph {
    let kind = NodeKind::new("node").unwrap();
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_mutation_provider(kind, provider)
        .await
        .unwrap();
    graph
}

async fn assert_poll_wakes_after_property_change(
    path: impl AsRef<Path>,
    graph: DynamicGraph,
    node: NodeId,
    key: PropertyKey,
    value: &str,
) {
    let path = path.as_ref().to_path_buf();
    let value = value.to_string();
    let expected = value.clone();
    let (ready_sender, mut ready_receiver) = mpsc::channel(1);
    let watcher = tokio::task::spawn_blocking(move || {
        let mut file = fs::File::open(path).unwrap();
        assert!(!read_file(&mut file).unwrap().is_empty());
        ready_sender.blocking_send(()).unwrap();
        wait_for_poll_readable(&file);
        assert_eq!(read_file(&mut file).unwrap(), expected);
    });

    ready_receiver.recv().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    graph
        .set_property(
            &node,
            &key,
            LocusValue::String(value.trim_end().to_string()),
        )
        .await
        .unwrap();

    watcher.await.unwrap();
}

fn wait_for_poll_readable(file: &fs::File) {
    let mut pollfd = libc::pollfd {
        fd: file.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pollfd, 1, 2_000) };
    assert_eq!(result, 1, "poll should wake within timeout");
    assert_ne!(
        pollfd.revents & libc::POLLIN,
        0,
        "expected POLLIN, got revents={:#x}",
        pollfd.revents
    );
}

fn read_file(file: &mut fs::File) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    Ok(value)
}

fn assert_path_value_retrying(path: &Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_value = None;
    let mut last_error = None;
    while Instant::now() < deadline {
        match fs::read_to_string(path) {
            Ok(value) if value == expected => return,
            Ok(value) => {
                last_value = Some(value);
                blocking_retry_delay();
            }
            Err(error) => {
                last_error = Some(error);
                blocking_retry_delay();
            }
        }
    }
    panic!(
        "failed to read expected value from {}: expected {expected:?}, last value: {last_value:?}, last error: {last_error:?}",
        path.display()
    );
}

fn test_node(local: &str) -> NodeId {
    NodeId::new(NodeKind::new("node").unwrap(), local).unwrap()
}

async fn wait_for_mount(mountpoint: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if mountpoint.join("node").is_dir() {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("mount did not become ready at {}", mountpoint.display());
}

fn blocking_retry_delay() {
    unsafe {
        libc::poll(std::ptr::null_mut(), 0, 25);
    }
}

fn cleanup_mount(mountpoint: &Path) {
    let _ = Command::new("fusermount3")
        .arg("-u")
        .arg(mountpoint)
        .status();
    let _ = Command::new("fusermount3")
        .arg("-uz")
        .arg(mountpoint)
        .status();
    let _ = fs::remove_dir_all(mountpoint);
}
