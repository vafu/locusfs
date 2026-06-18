use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::{NodeId, NodeKind, PropertyKey, Result};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GraphWatchTarget {
    Kind(NodeKind),
    Node(NodeId),
    NodeChild(NodeId, String),
    Property(NodeId, PropertyKey),
    Relation(NodeId, crate::RelationName),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphWatchEvent {
    Change,
    NodeAdded(NodeId),
    NodeChanged(NodeId),
    NodeRemoved(NodeId),
    PropertyAdded(NodeId, PropertyKey),
    PropertyChanged(NodeId, PropertyKey),
    PropertyRemoved(NodeId, PropertyKey),
    RelationAdded(NodeId, crate::RelationName),
    RelationChanged(NodeId, crate::RelationName),
    RelationRemoved(NodeId, crate::RelationName),
}

pub struct GraphWatch {
    receiver: mpsc::Receiver<GraphWatchEvent>,
}

impl GraphWatch {
    pub fn new(receiver: mpsc::Receiver<GraphWatchEvent>) -> Self {
        Self { receiver }
    }

    pub fn try_recv(&mut self) -> Option<GraphWatchEvent> {
        self.receiver.try_recv().ok()
    }

    pub async fn recv(&mut self) -> Option<GraphWatchEvent> {
        self.receiver.recv().await
    }
}

impl std::fmt::Debug for GraphWatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("GraphWatch").finish_non_exhaustive()
    }
}

#[async_trait]
pub trait WatchProvider: Send + Sync + 'static {
    async fn watch(&self, target: GraphWatchTarget) -> Result<GraphWatch>;
}
