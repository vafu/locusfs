use crate::{NodeId, NodeKind, PropertyKey, RelationName};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphChange {
    NodeKindChanged {
        kind: NodeKind,
    },
    NodeAdded {
        node: NodeId,
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
    PropertyAdded {
        node: NodeId,
        key: PropertyKey,
    },
    PropertyRemoved {
        node: NodeId,
        key: PropertyKey,
    },
    RelationChanged {
        source: NodeId,
        relation: RelationName,
    },
    RelationAdded {
        source: NodeId,
        relation: RelationName,
    },
    RelationRemoved {
        source: NodeId,
        relation: RelationName,
    },
}
