use std::path::PathBuf;

use fuser::FileType;
use locusfs_graph::{NodeId, NodeKind, PropertyKey, RelationName};

use crate::layout::encode_segment;

pub const ROOT_INO: u64 = 1;
pub const NODES_INO: u64 = 2;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FsEntry {
    Root,
    NodesDir,
    KindDir(NodeKind),
    NodeDir(NodeId),
    PropsDir(NodeId),
    PropertyFile(NodeId, PropertyKey),
    OutDir(NodeId),
    RelationDir(NodeId, RelationName),
    RelationTargetKindDir {
        source: NodeId,
        relation: RelationName,
        target_kind: NodeKind,
    },
    RelationLink {
        source: NodeId,
        relation: RelationName,
        target: NodeId,
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
        FsEntry::NodesDir => FsEntry::Root,
        FsEntry::KindDir(_) => FsEntry::NodesDir,
        FsEntry::NodeDir(node) => FsEntry::KindDir(node.kind().clone()),
        FsEntry::PropsDir(node) | FsEntry::OutDir(node) => FsEntry::NodeDir(node.clone()),
        FsEntry::PropertyFile(node, _) => FsEntry::PropsDir(node.clone()),
        FsEntry::RelationDir(source, _) => FsEntry::OutDir(source.clone()),
        FsEntry::RelationTargetKindDir {
            source, relation, ..
        } => FsEntry::RelationDir(source.clone(), relation.clone()),
        FsEntry::RelationLink {
            source,
            relation,
            target,
        } => FsEntry::RelationTargetKindDir {
            source: source.clone(),
            relation: relation.clone(),
            target_kind: target.kind().clone(),
        },
    }
}

pub fn relation_link_target(target: &NodeId) -> PathBuf {
    let kind = encode_segment(target.kind().as_str()).expect("valid node kind should encode");
    let local = encode_segment(target.local()).expect("valid node local id should encode");
    PathBuf::from("../../../../../").join(kind).join(local)
}
