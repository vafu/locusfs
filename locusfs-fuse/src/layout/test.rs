use std::path::PathBuf;

use locusfs_graph::{NodeId, NodeKind, PropertyKey, RelationName};

use crate::layout::{Layout, decode_segment, encode_segment};

#[test]
fn encodes_shell_hostile_segments() {
    assert_eq!(
        encode_segment("node:57/title active").unwrap(),
        "node%3A57%2Ftitle%20active"
    );
}

#[test]
fn decodes_segments() {
    assert_eq!(decode_segment("node%3A57").unwrap(), "node:57");
}

#[test]
fn encoded_reserved_segments_are_rejected() {
    assert!(decode_segment("%2E").is_err());
    assert!(decode_segment("%2E%2E").is_err());
}

#[test]
fn rejects_bad_percent_encoding() {
    assert!(decode_segment("node%").is_err());
    assert!(decode_segment("node%XX").is_err());
}

#[test]
fn rejects_nul_segments() {
    assert!(encode_segment("node\0id").is_err());
    assert!(decode_segment("node%00id").is_err());
}

#[test]
fn round_trips_slashes_and_utf8() {
    let value = "workspace/日本語";
    assert_eq!(
        decode_segment(&encode_segment(value).unwrap()).unwrap(),
        value
    );
}

#[test]
fn builds_generic_node_paths() {
    let node = NodeId::new(NodeKind::new("node").unwrap(), "57").unwrap();
    let key = PropertyKey::new("title").unwrap();
    assert_eq!(Layout::watch(), PathBuf::from("watch"));
    assert_eq!(
        Layout::node_property(&node, &key).unwrap(),
        PathBuf::from("node/57/title")
    );
}

#[test]
fn builds_inline_relation_link_paths() {
    let source = NodeId::new(NodeKind::new("window").unwrap(), "57").unwrap();
    let target = NodeId::new(NodeKind::new("workspace").unwrap(), "1").unwrap();
    let relation = RelationName::new("on-workspace").unwrap();

    assert_eq!(
        Layout::node_relation_link(&source, &relation, &target).unwrap(),
        PathBuf::from("window/57/on-workspace")
    );
    assert_eq!(
        Layout::node_relation_target_link(&source, &relation, &target).unwrap(),
        PathBuf::from("window/57/on-workspace/workspace%3A1")
    );
}
