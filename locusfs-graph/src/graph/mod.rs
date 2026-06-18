mod access;
mod change;
mod dynamic;
mod memory;
mod trace;
mod watch;

use async_trait::async_trait;

use crate::{LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName, Result};

pub use access::NodeAccess;
pub use change::GraphChange;
pub use dynamic::{
    DynamicGraph, GraphChangeReceiver, GraphChangeStreamError, GraphChangeSubscription,
};
pub use memory::InMemoryProvider;
pub use trace::TracedProvider;
pub use watch::{GraphWatch, GraphWatchEvent, GraphWatchTarget, WatchProvider};

#[async_trait]
pub trait NodeProvider: Send + Sync + 'static {
    fn kind(&self) -> &NodeKind;
    fn access(&self) -> NodeAccess {
        NodeAccess::read_only()
    }
    async fn contains_node(&self, node: &NodeId) -> Result<bool>;
    async fn nodes(&self) -> Result<Vec<NodeId>>;
}

#[async_trait]
pub trait NodeMutationProvider: Send + Sync + 'static {
    async fn create_node(&self, node: &NodeId) -> Result<()>;
    async fn remove_node(&self, node: &NodeId) -> Result<()>;
}

#[async_trait]
pub trait PropertyProvider: Send + Sync + 'static {
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec>;
    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>>;
    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue>;
}

#[async_trait]
pub trait PropertyMutationProvider: Send + Sync + 'static {
    async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()>;
    async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()>;
}

#[async_trait]
pub trait RelationProvider: Send + Sync + 'static {
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>>;
    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>>;
}

#[async_trait]
pub trait RelationMutationProvider: Send + Sync + 'static {
    async fn set_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()>;
    async fn remove_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()>;
}

#[cfg(test)]
mod test;
