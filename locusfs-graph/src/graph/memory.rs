use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::{
    GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName, Result,
};

use super::{
    NodeMutationProvider, NodeProvider, PropertyMutationProvider, PropertyProvider,
    RelationMutationProvider, RelationProvider,
};

#[derive(Clone, Debug)]
pub struct InMemoryProvider {
    kind: NodeKind,
    inner: Arc<RwLock<ProviderState>>,
}

#[derive(Clone, Debug, Default)]
struct ProviderState {
    nodes: BTreeMap<NodeId, Node>,
}

#[derive(Clone, Debug, Default)]
struct Node {
    properties: BTreeMap<PropertyKey, LocusValue>,
    links: BTreeMap<RelationName, BTreeSet<NodeId>>,
}

impl InMemoryProvider {
    pub fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            inner: Arc::new(RwLock::new(ProviderState::default())),
        }
    }

    async fn read_state(&self) -> tokio::sync::RwLockReadGuard<'_, ProviderState> {
        self.inner.read().await
    }

    async fn write_state(&self) -> tokio::sync::RwLockWriteGuard<'_, ProviderState> {
        self.inner.write().await
    }

    fn ensure_kind(&self, node: &NodeId) -> Result<()> {
        if node.kind() == &self.kind {
            Ok(())
        } else {
            Err(GraphError::NotFound {
                kind: "node kind provider",
                name: node.kind().to_string(),
            })
        }
    }

    fn existing_node<'a>(&self, state: &'a ProviderState, node: &NodeId) -> Result<&'a Node> {
        state.nodes.get(node).ok_or_else(|| GraphError::NotFound {
            kind: "node",
            name: node.to_string(),
        })
    }

    fn existing_node_mut<'a>(
        &self,
        state: &'a mut ProviderState,
        node: &NodeId,
    ) -> Result<&'a mut Node> {
        state
            .nodes
            .get_mut(node)
            .ok_or_else(|| GraphError::NotFound {
                kind: "node",
                name: node.to_string(),
            })
    }
}

#[async_trait]
impl NodeProvider for InMemoryProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.ensure_kind(node)?;
        let state = self.read_state().await;
        Ok(state.nodes.contains_key(node))
    }

    async fn nodes(&self) -> Result<Vec<NodeId>> {
        let state = self.read_state().await;
        Ok(state.nodes.keys().cloned().collect())
    }
}

#[async_trait]
impl NodeMutationProvider for InMemoryProvider {
    async fn create_node(&self, node: &NodeId) -> Result<()> {
        self.ensure_kind(node)?;
        let mut state = self.write_state().await;
        state.nodes.entry(node.clone()).or_default();
        Ok(())
    }

    async fn remove_node(&self, node: &NodeId) -> Result<()> {
        self.ensure_kind(node)?;
        let mut state = self.write_state().await;
        state
            .nodes
            .remove(node)
            .ok_or_else(|| GraphError::NotFound {
                kind: "node",
                name: node.to_string(),
            })?;

        for existing in state.nodes.values_mut() {
            existing.links.retain(|_, targets| {
                targets.remove(node);
                !targets.is_empty()
            });
        }

        Ok(())
    }
}

#[async_trait]
impl PropertyProvider for InMemoryProvider {
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        let state = self.read_state().await;
        let node = self.existing_node(&state, subject)?;
        let value = node
            .properties
            .get(key)
            .ok_or_else(|| GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })?;
        Ok(PropertySpec::read_write(key.clone(), value.kind()))
    }

    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        let state = self.read_state().await;
        let node = self.existing_node(&state, subject)?;
        Ok(node
            .properties
            .iter()
            .map(|(key, value)| PropertySpec::read_write(key.clone(), value.kind()))
            .collect())
    }

    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        let state = self.read_state().await;
        let node = self.existing_node(&state, subject)?;
        node.properties
            .get(key)
            .cloned()
            .ok_or_else(|| GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
    }
}

#[async_trait]
impl PropertyMutationProvider for InMemoryProvider {
    async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        let mut state = self.write_state().await;
        let node = self.existing_node_mut(&mut state, subject)?;
        node.properties.insert(key.clone(), value);
        Ok(())
    }

    async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        let mut state = self.write_state().await;
        let node = self.existing_node_mut(&mut state, subject)?;
        if node.properties.remove(key).is_some() {
            Ok(())
        } else {
            Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
        }
    }
}

#[async_trait]
impl RelationProvider for InMemoryProvider {
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        let state = self.read_state().await;
        let node = self.existing_node(&state, source)?;
        Ok(node.links.keys().cloned().collect())
    }

    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        let state = self.read_state().await;
        let node = self.existing_node(&state, source)?;
        node.links
            .get(relation)
            .map(|targets| targets.iter().cloned().collect())
            .ok_or_else(|| GraphError::NotFound {
                kind: "relation",
                name: format!("{source}/{relation}"),
            })
    }
}

#[async_trait]
impl RelationMutationProvider for InMemoryProvider {
    async fn set_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        let mut state = self.write_state().await;
        let source_node = self.existing_node_mut(&mut state, source)?;
        source_node
            .links
            .entry(relation.clone())
            .or_default()
            .insert(target.clone());
        Ok(())
    }

    async fn remove_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        let mut state = self.write_state().await;
        let source_node = self.existing_node_mut(&mut state, source)?;
        let removed = {
            let targets =
                source_node
                    .links
                    .get_mut(relation)
                    .ok_or_else(|| GraphError::NotFound {
                        kind: "relation",
                        name: format!("{source}/{relation}"),
                    })?;
            let removed = targets.remove(target);
            let is_empty = targets.is_empty();
            if is_empty {
                source_node.links.remove(relation);
            }
            removed
        };
        if removed {
            Ok(())
        } else {
            Err(GraphError::NotFound {
                kind: "relation target",
                name: format!("{relation}/{target}"),
            })
        }
    }
}
