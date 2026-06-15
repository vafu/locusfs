use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

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
            inner: Arc::default(),
        }
    }

    fn read_state(&self) -> Result<std::sync::RwLockReadGuard<'_, ProviderState>> {
        self.inner.read().map_err(|_| GraphError::Internal {
            reason: "in-memory provider lock poisoned",
        })
    }

    fn write_state(&self) -> Result<std::sync::RwLockWriteGuard<'_, ProviderState>> {
        self.inner.write().map_err(|_| GraphError::Internal {
            reason: "in-memory provider lock poisoned",
        })
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

impl NodeProvider for InMemoryProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.ensure_kind(node)?;
        let state = self.read_state()?;
        Ok(state.nodes.contains_key(node))
    }

    fn nodes(&self) -> Result<Vec<NodeId>> {
        let state = self.read_state()?;
        Ok(state.nodes.keys().cloned().collect())
    }
}

impl NodeMutationProvider for InMemoryProvider {
    fn create_node(&self, node: &NodeId) -> Result<()> {
        self.ensure_kind(node)?;
        let mut state = self.write_state()?;
        state.nodes.entry(node.clone()).or_default();
        Ok(())
    }

    fn remove_node(&self, node: &NodeId) -> Result<()> {
        self.ensure_kind(node)?;
        let mut state = self.write_state()?;
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

impl PropertyProvider for InMemoryProvider {
    fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        let state = self.read_state()?;
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

    fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        let state = self.read_state()?;
        let node = self.existing_node(&state, subject)?;
        Ok(node
            .properties
            .iter()
            .map(|(key, value)| PropertySpec::read_write(key.clone(), value.kind()))
            .collect())
    }

    fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        let state = self.read_state()?;
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

impl PropertyMutationProvider for InMemoryProvider {
    fn set_property(&self, subject: &NodeId, key: &PropertyKey, value: LocusValue) -> Result<()> {
        let mut state = self.write_state()?;
        let node = self.existing_node_mut(&mut state, subject)?;
        node.properties.insert(key.clone(), value);
        Ok(())
    }

    fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        let mut state = self.write_state()?;
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

impl RelationProvider for InMemoryProvider {
    fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        let state = self.read_state()?;
        let node = self.existing_node(&state, source)?;
        Ok(node.links.keys().cloned().collect())
    }

    fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        let state = self.read_state()?;
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

impl RelationMutationProvider for InMemoryProvider {
    fn set_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()> {
        let mut state = self.write_state()?;
        let source_node = self.existing_node_mut(&mut state, source)?;
        source_node
            .links
            .entry(relation.clone())
            .or_default()
            .insert(target.clone());
        Ok(())
    }

    fn remove_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) -> Result<()> {
        let mut state = self.write_state()?;
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
