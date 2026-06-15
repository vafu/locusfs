use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use locusfs_fuse::{FuseMountConfig, mount};
use locusfs_graph::{DynamicGraph, InMemoryProvider, NodeKind};

#[test]
#[ignore = "requires host /dev/fuse access"]
fn generic_nodes_props_and_out_relations_work_through_real_fuse_mount() {
    let mountpoint = Path::new("/tmp/locusfs-test");
    cleanup_mount(mountpoint);
    fs::create_dir_all(mountpoint).unwrap();

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
    let _mount = mount(FuseMountConfig::new(mountpoint), graph).unwrap();
    wait_for_mount(mountpoint);

    let source = mountpoint.join("nodes/node/57");
    let target = mountpoint.join("nodes/node/6");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();

    fs::write(source.join("props/title"), "value\n").unwrap();
    assert_eq!(
        fs::read_to_string(source.join("props/title")).unwrap(),
        "value\n"
    );

    fs::create_dir(source.join("out/linked-to")).unwrap();
    fs::create_dir(source.join("out/linked-to/node")).unwrap();
    symlink("../../../../../node/6", source.join("out/linked-to/node/6")).unwrap();
    assert_eq!(
        fs::read_link(source.join("out/linked-to/node/6")).unwrap(),
        Path::new("../../../../../node/6")
    );

    let relation_target_kinds = fs::read_dir(source.join("out/linked-to"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(relation_target_kinds, vec!["node"]);

    let relation_targets = fs::read_dir(source.join("out/linked-to/node"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(relation_targets, vec!["6"]);

    fs::remove_file(source.join("out/linked-to/node/6")).unwrap();
    fs::remove_dir(source.join("out/linked-to/node")).unwrap();
    assert!(
        fs::read_dir(source.join("out/linked-to"))
            .unwrap()
            .next()
            .is_none()
    );

    fs::remove_file(source.join("props/title")).unwrap();
    assert!(fs::read_dir(source.join("props")).unwrap().next().is_none());

    drop(_mount);
    cleanup_mount(mountpoint);
}

fn wait_for_mount(mountpoint: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if mountpoint.join("nodes").is_dir() {
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
