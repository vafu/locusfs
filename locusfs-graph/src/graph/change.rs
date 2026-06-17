use crate::{NodeId, NodeKind, PropertyKey, RelationName};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphChange {
    NodeKindChanged {
        kind: NodeKind,
    },
    NodeChanged {
        node: NodeId,
    },
    NodeRemoved {
        node: NodeId,
    },
    PropertyChanged {
        node: NodeId,
        key: PropertyKey,
    },
    RelationChanged {
        source: NodeId,
        relation: RelationName,
    },
}
