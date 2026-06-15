mod dynamic;
mod memory;

use crate::{LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName, Result};

pub use dynamic::DynamicGraph;
pub use memory::InMemoryProvider;

pub trait NodeProvider: Send + Sync + 'static {
    fn kind(&self) -> &NodeKind;
    fn contains_node(&self, node: &NodeId) -> Result<bool>;
    fn nodes(&self) -> Result<Vec<NodeId>>;
}

pub trait NodeMutationProvider: Send + Sync + 'static {
    fn create_node(&self, node: &NodeId) -> Result<()>;
    fn remove_node(&self, node: &NodeId) -> Result<()>;
}

pub trait PropertyProvider: Send + Sync + 'static {
    fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec>;
    fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>>;
    fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue>;
}

pub trait PropertyMutationProvider: Send + Sync + 'static {
    fn set_property(&self, subject: &NodeId, key: &PropertyKey, value: LocusValue) -> Result<()>;
    fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()>;
}

pub trait RelationProvider: Send + Sync + 'static {
    fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>>;
    fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>>;
}

pub trait RelationMutationProvider: Send + Sync + 'static {
    fn set_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()>;
    fn remove_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()>;
}

#[cfg(test)]
mod test;
