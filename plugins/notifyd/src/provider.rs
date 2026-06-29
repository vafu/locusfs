use async_trait::async_trait;
use locusfs_graph::{
    GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, GraphWatchTarget, LocusValue,
    NodeAccess, NodeId, NodeKind, NodeProvider, PathName, PathProvider, PropertyKey,
    PropertyMutationProvider, PropertyProvider, PropertySpec, RelationName, RelationProvider,
    Result,
};
use tokio::sync::mpsc;

use crate::NOTIFICATION_KIND;
use crate::state::{NotifydCommandTarget, NotifydState, SharedNotifydState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NotifydCommand {
    Discard {
        notification_id: String,
    },
    InvokeAction {
        notification_id: String,
        action_key: String,
    },
    DiscardAll,
    SetDnd(bool),
}

#[derive(Clone)]
pub struct NotifydProvider {
    kind: NodeKind,
    state: SharedNotifydState,
    commands: mpsc::UnboundedSender<NotifydCommand>,
}

impl NotifydProvider {
    pub(crate) fn new(
        kind: NodeKind,
        state: SharedNotifydState,
        commands: mpsc::UnboundedSender<NotifydCommand>,
    ) -> Self {
        Self {
            kind,
            state,
            commands,
        }
    }

    async fn with_state<T>(&self, operation: impl FnOnce(&NotifydState) -> Result<T>) -> Result<T> {
        let state = self.state.read().await;
        operation(&state)
    }
}

#[async_trait]
impl NodeProvider for NotifydProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn access(&self) -> NodeAccess {
        if self.kind.as_str() == NOTIFICATION_KIND {
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
impl PathProvider for NotifydProvider {
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
impl PropertyProvider for NotifydProvider {
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
impl PropertyMutationProvider for NotifydProvider {
    async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        let target = self
            .with_state(|state| state.command_target(subject, key, &value))
            .await?;
        let command = match target {
            NotifydCommandTarget::Discard { notification_id } => {
                NotifydCommand::Discard { notification_id }
            }
            NotifydCommandTarget::InvokeAction {
                notification_id,
                action_key,
            } => NotifydCommand::InvokeAction {
                notification_id,
                action_key,
            },
            NotifydCommandTarget::DiscardAll => NotifydCommand::DiscardAll,
            NotifydCommandTarget::SetDnd(enabled) => NotifydCommand::SetDnd(enabled),
        };
        self.commands
            .send(command)
            .map_err(|error| GraphError::Io(error.to_string()))
    }

    async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        Err(GraphError::InvalidValue {
            kind: "notifyd property",
            value: format!("{subject}/{key}"),
            reason: "property removal is not supported by notifyd",
        })
    }
}

#[async_trait]
impl RelationProvider for NotifydProvider {
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.with_state(|state| state.relations(source)).await
    }

    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.targets(source, relation))
            .await
    }
}
