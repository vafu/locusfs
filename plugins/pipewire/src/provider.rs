use async_trait::async_trait;
use locusfs_graph::{
    GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget, LocusValue, NodeAccess,
    NodeId, NodeKind, NodeProvider, PathName, PathProvider, PropertyKey, PropertyProvider,
    PropertySpec, RelationName, RelationProvider, Result,
};

use crate::state::{PipeWireState, SharedPipeWireState};
use crate::{PIPEWIRE_SINK_KIND, PIPEWIRE_SOURCE_KIND};

#[derive(Clone)]
pub struct PipeWireProvider {
    kind: NodeKind,
    state: SharedPipeWireState,
}

impl PipeWireProvider {
    pub(crate) fn new(kind: NodeKind, state: SharedPipeWireState) -> Self {
        Self { kind, state }
    }

    async fn with_state<T>(
        &self,
        operation: impl FnOnce(&PipeWireState) -> Result<T>,
    ) -> Result<T> {
        let state = self.state.read().await;
        operation(&state)
    }
}

#[async_trait]
impl NodeProvider for PipeWireProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn access(&self) -> NodeAccess {
        if matches!(
            self.kind.as_str(),
            PIPEWIRE_SINK_KIND | PIPEWIRE_SOURCE_KIND
        ) {
            NodeAccess::hidden()
        } else {
            NodeAccess::read_only()
        }
    }

    async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.with_state(|state| state.contains_node(node)).await
    }

    async fn nodes(&self) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.nodes(&self.kind)).await
    }
}

#[async_trait]
impl PathProvider for PipeWireProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    async fn lookup_child(
        &self,
        parent: &GraphPathDirectory,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        self.with_state(|state| state.path_lookup_child(parent, name))
            .await
    }

    async fn children(&self, parent: &GraphPathDirectory) -> Result<Option<Vec<GraphPathChild>>> {
        self.with_state(|state| state.path_children(parent)).await
    }

    async fn watch_target(
        &self,
        directory: &GraphPathDirectory,
    ) -> Result<Option<GraphWatchTarget>> {
        self.with_state(|state| state.path_watch_target(directory))
            .await
    }
}

#[async_trait]
impl PropertyProvider for PipeWireProvider {
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        self.with_state(|state| state.property_spec(subject, key))
            .await
    }

    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        self.with_state(|state| state.properties(subject)).await
    }

    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.with_state(|state| state.property(subject, key)).await
    }
}

#[async_trait]
impl RelationProvider for PipeWireProvider {
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.with_state(|state| state.relations(source)).await
    }

    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.targets(source, relation))
            .await
    }
}
