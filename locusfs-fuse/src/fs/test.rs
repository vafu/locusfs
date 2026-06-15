use locusfs_graph::{NodeId, NodeKind, RelationName};

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
fn read_slicing_respects_offset_and_size() {
    assert_eq!(slice_for_read(b"abcdef", 2, 3), b"cde");
    assert_eq!(slice_for_read(b"abcdef", 9, 3), b"");
}

#[test]
fn relation_symlink_targets_point_back_to_node_dir() {
    let target = test_node("6");
    assert_eq!(
        relation_link_target(&target),
        std::path::PathBuf::from("../../../../../node/6")
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
