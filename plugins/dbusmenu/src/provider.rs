use async_trait::async_trait;
use locusfs_graph::{
    GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget, LocusValue,
    NodeAccess, NodeId, NodeKind, NodeProvider, PathName, PathProvider, PropertyKey,
    PropertyMutationProvider, PropertyProvider, PropertySpec, RelationName, RelationProvider,
    Result,
};
use tokio::runtime::Handle;

use crate::state::{DbusMenuState, SharedDbusMenuState};
use crate::{DBUSMENU_ITEM_KIND, DBUSMENU_MENU_KIND};

#[derive(Clone)]
pub struct DbusMenuProvider {
    kind: NodeKind,
    state: SharedDbusMenuState,
    runtime: Handle,
}

impl DbusMenuProvider {
    pub(crate) fn new(kind: NodeKind, state: SharedDbusMenuState, runtime: Handle) -> Self {
        Self {
            kind,
            state,
            runtime,
        }
    }

    async fn with_state<T>(
        &self,
        operation: impl FnOnce(&DbusMenuState) -> Result<T>,
    ) -> Result<T> {
        let state = self.state.read().await;
        operation(&state)
    }
}

#[async_trait]
impl NodeProvider for DbusMenuProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn access(&self) -> NodeAccess {
        if matches!(self.kind.as_str(), DBUSMENU_MENU_KIND | DBUSMENU_ITEM_KIND) {
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
impl PathProvider for DbusMenuProvider {
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
impl PropertyProvider for DbusMenuProvider {
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
impl RelationProvider for DbusMenuProvider {
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.with_state(|state| state.relations(source)).await
    }

    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.targets(source, relation))
            .await
    }
}

#[async_trait]
impl PropertyMutationProvider for DbusMenuProvider {
    async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        match value {
            LocusValue::Bool(true) => {}
            other => {
                return Err(GraphError::InvalidValue {
                    kind: "DBusMenu activation value",
                    value: other.to_string(),
                    reason: "write true to activate a menu item",
                });
            }
        }
        let target = self
            .with_state(|state| state.activation_target(subject, key))
            .await?;
        self.runtime
            .spawn(crate::runtime::activate_item(target))
            .await
            .map_err(|error| GraphError::Io(format!("activate DBusMenu task failed: {error}")))?
    }

    async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        Err(GraphError::InvalidValue {
            kind: "DBusMenu property",
            value: format!("{subject}/{key}"),
            reason: "property removal is not supported by DBusMenu",
        })
    }
}
