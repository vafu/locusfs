mod access;
mod change;
#[cfg(feature = "dynamic")]
mod dynamic;
#[cfg(feature = "in-memory")]
mod memory;
#[cfg(feature = "provider-tracing")]
mod trace;
#[cfg(feature = "watch-provider")]
mod watch;

use async_trait::async_trait;

use crate::{
    LocusValue, NodeId, NodeKind, PathName, PropertyKey, PropertySpec, RelationName, Result,
};

pub use access::NodeAccess;
pub use change::GraphChange;
#[cfg(feature = "dynamic")]
pub use dynamic::{
    DynamicGraph, GraphChangeReceiver, GraphChangeStreamError, GraphChangeSubscription,
};
#[cfg(feature = "in-memory")]
pub use memory::InMemoryProvider;
#[cfg(feature = "provider-tracing")]
pub use trace::TracedProvider;
#[cfg(feature = "watch-provider")]
pub use watch::{GraphWatch, GraphWatchEvent, GraphWatchTarget, WatchProvider};

/// Directory identity used by provider-owned filesystem path layouts.
///
/// `Node` represents the normal directory for a graph node. `Virtual` lets a
/// provider expose additional directories below one of its nodes without adding
/// synthetic graph nodes for every path segment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GraphPathDirectory {
    Node(NodeId),
    Virtual { owner: NodeKind, local: String },
}

/// Filesystem entry resolved by a [`PathProvider`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GraphPathEntry {
    Directory(GraphPathDirectory),
    Property { node: NodeId, key: PropertyKey },
    Symlink { target: NodeId },
}

/// Named child entry exposed by a [`PathProvider`] directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphPathChild {
    pub name: PathName,
    pub entry: GraphPathEntry,
}

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

/// Optional provider-owned filesystem path layout.
///
/// FUSE asks this trait before falling back to generic graph property/relation
/// layout. Implementations may expose virtual directories, remap path children
/// to real graph properties, and provide a watch target for directory updates.
#[async_trait]
pub trait PathProvider: Send + Sync + 'static {
    fn kind(&self) -> &NodeKind;

    async fn lookup_child(
        &self,
        parent: &GraphPathDirectory,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>>;

    async fn children(&self, parent: &GraphPathDirectory) -> Result<Option<Vec<GraphPathChild>>>;

    #[cfg(feature = "watch-provider")]
    async fn watch_target(
        &self,
        directory: &GraphPathDirectory,
    ) -> Result<Option<GraphWatchTarget>>;
}

#[cfg(all(
    test,
    feature = "dynamic",
    feature = "in-memory",
    feature = "provider-tracing",
    feature = "watch-provider"
))]
mod test;
