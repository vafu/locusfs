use std::path::PathBuf;

use fuse3::FileType;
use locusfs_graph::{GraphPathDirectory, NodeId, NodeKind, PropertyKey, RelationName};

use crate::layout::encode_segment;

pub const ROOT_INO: u64 = 1;
pub const WATCH_FILE_NAME: &str = "watch";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FsEntry {
    Root,
    WatchFile,
    KindDir(NodeKind),
    NodeDir(NodeId),
    PropertyFile(NodeId, PropertyKey),
    RelationDir(NodeId, RelationName),
    RelationLink {
        source: NodeId,
        relation: RelationName,
        target: NodeId,
    },
    RelationTargetLink {
        source: NodeId,
        relation: RelationName,
        target: NodeId,
    },
    PathDir {
        directory: GraphPathDirectory,
        parent: Box<FsEntry>,
    },
    PathLink {
        target: NodeId,
        parent: Box<FsEntry>,
    },
}

#[derive(Debug)]
pub struct DirEntry {
    pub ino: u64,
    pub kind: FileType,
    pub name: String,
}

impl DirEntry {
    pub fn new(ino: u64, kind: FileType, name: impl Into<String>) -> Self {
        Self {
            ino,
            kind,
            name: name.into(),
        }
    }
}

pub fn parent_entry(entry: &FsEntry) -> FsEntry {
    match entry {
        FsEntry::Root => FsEntry::Root,
        FsEntry::WatchFile => FsEntry::Root,
        FsEntry::KindDir(_) => FsEntry::Root,
        FsEntry::NodeDir(node) => FsEntry::KindDir(node.kind().clone()),
        FsEntry::PropertyFile(node, _) => FsEntry::NodeDir(node.clone()),
        FsEntry::RelationLink { source: node, .. } => FsEntry::NodeDir(node.clone()),
        FsEntry::RelationDir(source, _) => FsEntry::NodeDir(source.clone()),
        FsEntry::RelationTargetLink {
            source, relation, ..
        } => FsEntry::RelationDir(source.clone(), relation.clone()),
        FsEntry::PathDir { parent, .. } | FsEntry::PathLink { parent, .. } => *parent.clone(),
    }
}

pub fn direct_relation_link_target(target: &NodeId) -> PathBuf {
    relation_link_target_with_parent_depth(target, 2)
}

pub fn nested_relation_link_target(target: &NodeId) -> PathBuf {
    relation_link_target_with_parent_depth(target, 3)
}

fn relation_link_target_with_parent_depth(target: &NodeId, parent_depth: usize) -> PathBuf {
    let mut path = PathBuf::new();
    for _ in 0..parent_depth {
        path.push("..");
    }
    path.join(encoded_kind_and_local(target))
}

fn encoded_kind_and_local(target: &NodeId) -> PathBuf {
    let kind = encode_segment(target.kind().as_str()).expect("valid node kind should encode");
    let local = encode_segment(target.local()).expect("valid node local id should encode");
    PathBuf::from(kind).join(local)
}
