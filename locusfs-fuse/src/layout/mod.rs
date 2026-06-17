mod segment;

use std::path::PathBuf;

pub use segment::{decode_segment, encode_segment};

use locusfs_graph::{NodeId, NodeKind, PropertyKey, RelationName, Result};

/// Path builder for the public FUSE filesystem layout.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Layout;

impl Layout {
    pub fn watch() -> PathBuf {
        PathBuf::from("watch")
    }

    pub fn kind_dir(kind: &NodeKind) -> Result<PathBuf> {
        Ok(PathBuf::from(encode_segment(kind.as_str())?))
    }

    pub fn node_dir(node: &NodeId) -> Result<PathBuf> {
        Ok(Self::kind_dir(node.kind())?.join(encode_segment(node.local())?))
    }

    pub fn node_property(node: &NodeId, key: &PropertyKey) -> Result<PathBuf> {
        Ok(Self::node_dir(node)?.join(encode_segment(key.as_str())?))
    }

    pub fn node_relation_dir(node: &NodeId, relation: &RelationName) -> Result<PathBuf> {
        Ok(Self::node_dir(node)?.join(encode_segment(relation.as_str())?))
    }

    pub fn node_relation_link(
        node: &NodeId,
        relation: &RelationName,
        _target: &NodeId,
    ) -> Result<PathBuf> {
        Self::node_relation_dir(node, relation)
    }

    pub fn node_relation_target_link(
        node: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<PathBuf> {
        Ok(Self::node_relation_dir(node, relation)?.join(encode_segment(&target.to_string())?))
    }
}

#[cfg(test)]
mod test;
