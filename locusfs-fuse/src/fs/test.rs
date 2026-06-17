use locusfs_graph::{NodeId, NodeKind, PropertyKey, RelationName};

use super::watch;
use super::*;

#[test]
fn stable_inodes_are_allocated_for_same_entry() {
    let mut table = InodeTable::new();
    let node = test_node("57");
    let first = table.inode(FsEntry::NodeDir(node.clone())).unwrap();
    let second = table.inode(FsEntry::NodeDir(node)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn forgotten_inodes_are_removed_from_cache() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));
    let ino = table.acquire(entry).unwrap();

    table.forget(ino, 1);

    assert!(table.entry(ino).is_none());
}

#[test]
fn entry_timestamps_are_stable_until_touched() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));

    let first = table.times(&entry);
    let second = table.times(&entry);

    assert_eq!(first, second);
}

#[test]
fn touching_entry_updates_mtime_and_ctime() {
    let mut table = InodeTable::new();
    let entry = FsEntry::NodeDir(test_node("57"));
    let before = table.times(&entry);

    wait_for_clock_tick();
    table.touch(&entry);
    let after = table.times(&entry);

    assert_eq!(after.created, before.created);
    assert_eq!(after.accessed, before.accessed);
    assert!(after.modified > before.modified);
    assert!(after.changed > before.changed);
}

fn wait_for_clock_tick() {
    unsafe {
        libc::poll(std::ptr::null_mut(), 0, 1);
    }
}

#[test]
fn watch_registry_tracks_unread_property_changes() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let entry = FsEntry::PropertyFile(node.clone(), key.clone());
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&entry).unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(registry.notify_property_change(&node, &key).is_empty());
    assert!(registry.has_unread_change(handle));

    registry.mark_read(handle);

    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_tracks_unread_node_changes_for_open_properties() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let entry = FsEntry::PropertyFile(node.clone(), key);
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&entry).unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(registry.notify_node_change(&node).is_empty());
    assert!(registry.has_unread_change(handle));

    registry.mark_read(handle);

    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_marks_configured_watch_pending_for_subject_change() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let handle = registry.open(&FsEntry::WatchFile).unwrap();

    registry
        .configure_watch(
            handle,
            "/node/57/title".to_string(),
            watch::WatchTarget {
                subject: watch::WatchSubjectKey::Property(node.clone(), key.clone()),
                dependencies: Vec::new(),
            },
        )
        .unwrap();

    assert!(!registry.has_unread_change(handle));
    assert!(registry.notify_property_change(&node, &key).is_empty());
    assert!(registry.has_unread_change(handle));

    let event = String::from_utf8(registry.read_watch(handle).unwrap()).unwrap();
    assert_eq!(event, "change\n");
    assert!(!registry.has_unread_change(handle));
}

#[test]
fn watch_registry_fans_out_shared_subjects_to_multiple_open_files() {
    let node = test_node("57");
    let key = PropertyKey::new("title").unwrap();
    let mut registry = WatchRegistry::new();
    let first = registry.open(&FsEntry::WatchFile).unwrap();
    let second = registry.open(&FsEntry::WatchFile).unwrap();

    let target = watch::WatchTarget {
        subject: watch::WatchSubjectKey::Property(node.clone(), key.clone()),
        dependencies: Vec::new(),
    };
    registry
        .configure_watch(
            first,
            "/context/selected/window/title".to_string(),
            target.clone(),
        )
        .unwrap();
    registry
        .configure_watch(second, "/node/57/title".to_string(), target)
        .unwrap();

    registry.notify_property_change(&node, &key);

    assert!(registry.has_unread_change(first));
    assert!(registry.has_unread_change(second));
    assert_eq!(registry.read_watch(first).unwrap(), b"change\n");
    assert!(!registry.has_unread_change(first));
    assert!(registry.has_unread_change(second));
}

#[test]
fn read_slicing_respects_offset_and_size() {
    assert_eq!(slice_for_read(b"abcdef", 2, 3), b"cde");
    assert_eq!(slice_for_read(b"abcdef", 9, 3), b"");
}

#[test]
fn relation_symlink_targets_point_back_to_node_dir() {
    let target = test_node("6");
    assert_eq!(
        direct_relation_link_target(&target),
        std::path::PathBuf::from("../../node/6")
    );
    assert_eq!(
        nested_relation_link_target(&target),
        std::path::PathBuf::from("../../../node/6")
    );
}

#[test]
fn relation_entries_are_hashable_and_stable() {
    let source = test_node("57");
    let relation = RelationName::new("linked-to").unwrap();
    let target = test_node("6");
    let mut table = InodeTable::new();
    let first = table
        .inode(FsEntry::RelationLink {
            source: source.clone(),
            relation: relation.clone(),
            target: target.clone(),
        })
        .unwrap();
    let second = table
        .inode(FsEntry::RelationLink {
            source,
            relation,
            target,
        })
        .unwrap();
    assert_eq!(first, second);
}

fn test_kind() -> NodeKind {
    NodeKind::new("node").unwrap()
}

fn test_node(local: &str) -> NodeId {
    NodeId::new(test_kind(), local).unwrap()
}
