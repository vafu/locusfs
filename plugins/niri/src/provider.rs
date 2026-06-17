use locusfs_graph::{
    GraphError, LocusValue, NodeId, NodeKind, NodeProvider, PropertyKey, PropertyProvider,
    PropertySpec, RelationName, RelationProvider, Result,
};

use crate::ipc::SharedNiriState;
use crate::state::NiriState;

#[derive(Clone)]
pub struct NiriProvider {
    kind: NodeKind,
    state: SharedNiriState,
}

impl NiriProvider {
    pub(crate) fn new(kind: NodeKind, state: SharedNiriState) -> Self {
        Self { kind, state }
    }

    fn with_state<T>(&self, operation: impl FnOnce(&NiriState) -> Result<T>) -> Result<T> {
        let state = self.state.read().map_err(|_| GraphError::Internal {
            reason: "niri provider state lock poisoned",
        })?;
        operation(&state)
    }
}

impl NodeProvider for NiriProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.with_state(|state| state.contains_node(node))
    }

    fn nodes(&self) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.nodes(&self.kind))
    }
}

impl PropertyProvider for NiriProvider {
    fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        self.with_state(|state| state.property_spec(subject, key))
    }

    fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        self.with_state(|state| state.properties(subject))
    }

    fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.with_state(|state| state.property(subject, key))
    }
}

impl RelationProvider for NiriProvider {
    fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.with_state(|state| state.relations(source))
    }

    fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.targets(source, relation))
    }
}
