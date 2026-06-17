use std::collections::BTreeMap;
use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};

use crate::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName,
    Result,
};

use super::{
    NodeMutationProvider, NodeProvider, PropertyMutationProvider, PropertyProvider,
    RelationMutationProvider, RelationProvider,
};

type NodeProviders = BTreeMap<NodeKind, Arc<dyn NodeProvider>>;
type NodeMutationProviders = BTreeMap<NodeKind, Arc<dyn NodeMutationProvider>>;
type PropertyProviders = BTreeMap<NodeKind, Arc<dyn PropertyProvider>>;
type PropertyMutationProviders = BTreeMap<NodeKind, Arc<dyn PropertyMutationProvider>>;
type RelationProviders = BTreeMap<NodeKind, Arc<dyn RelationProvider>>;
type RelationMutationProviders = BTreeMap<NodeKind, Arc<dyn RelationMutationProvider>>;
type RegistryReadGuard<'a> = std::sync::RwLockReadGuard<'a, ProviderRegistry>;
type RegistryWriteGuard<'a> = std::sync::RwLockWriteGuard<'a, ProviderRegistry>;

#[derive(Clone, Default)]
pub struct DynamicGraph {
    providers: Arc<RwLock<ProviderRegistry>>,
    change_subscribers: Arc<Mutex<Vec<Sender<GraphChange>>>>,
}

#[derive(Clone, Default)]
struct ProviderRegistry {
    nodes: NodeProviders,
    node_mutations: NodeMutationProviders,
    properties: PropertyProviders,
    property_mutations: PropertyMutationProviders,
    relations: RelationProviders,
    relation_mutations: RelationMutationProviders,
}

impl DynamicGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe_changes(&self) -> Result<Receiver<GraphChange>> {
        let (sender, receiver) = mpsc::channel();
        self.change_subscribers
            .lock()
            .map_err(|_| GraphError::Internal {
                reason: "graph change subscriber lock poisoned",
            })?
            .push(sender);
        Ok(receiver)
    }

    pub fn emit_change(&self, change: GraphChange) -> Result<()> {
        let mut subscribers = self
            .change_subscribers
            .lock()
            .map_err(|_| GraphError::Internal {
                reason: "graph change subscriber lock poisoned",
            })?;
        subscribers.retain(|subscriber| subscriber.send(change.clone()).is_ok());
        Ok(())
    }

    pub fn register_node_provider<P>(&self, provider: P) -> Result<()>
    where
        P: NodeProvider,
    {
        let kind = provider.kind().clone();
        self.write_providers()?
            .nodes
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub fn register_node_mutation_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: NodeMutationProvider,
    {
        self.write_providers()?
            .node_mutations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub fn register_property_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: PropertyProvider,
    {
        self.write_providers()?
            .properties
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub fn register_property_mutation_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: PropertyMutationProvider,
    {
        self.write_providers()?
            .property_mutations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub fn register_relation_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: RelationProvider,
    {
        self.write_providers()?
            .relations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub fn register_relation_mutation_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: RelationMutationProvider,
    {
        self.write_providers()?
            .relation_mutations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    fn node_provider_for_kind(&self, kind: &NodeKind) -> Result<Arc<dyn NodeProvider>> {
        let providers = self.read_providers()?;
        providers
            .nodes
            .get(kind)
            .cloned()
            .ok_or_else(|| missing_provider("node", kind))
    }

    fn node_provider_for_node(&self, node: &NodeId) -> Result<Arc<dyn NodeProvider>> {
        self.node_provider_for_kind(node.kind())
    }

    fn node_mutation_provider_for_node(
        &self,
        node: &NodeId,
    ) -> Result<Arc<dyn NodeMutationProvider>> {
        let providers = self.read_providers()?;
        providers
            .node_mutations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| unsupported_provider("node mutation"))
    }

    fn property_provider_for_node(&self, node: &NodeId) -> Result<Arc<dyn PropertyProvider>> {
        let providers = self.read_providers()?;
        providers
            .properties
            .get(node.kind())
            .cloned()
            .ok_or_else(|| missing_provider("property", node.kind()))
    }

    fn property_mutation_provider_for_node(
        &self,
        node: &NodeId,
    ) -> Result<Arc<dyn PropertyMutationProvider>> {
        let providers = self.read_providers()?;
        providers
            .property_mutations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| unsupported_provider("property mutation"))
    }

    fn relation_provider_for_node(&self, node: &NodeId) -> Result<Arc<dyn RelationProvider>> {
        let providers = self.read_providers()?;
        providers
            .relations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| missing_provider("relation", node.kind()))
    }

    fn relation_mutation_provider_for_node(
        &self,
        node: &NodeId,
    ) -> Result<Arc<dyn RelationMutationProvider>> {
        let providers = self.read_providers()?;
        providers
            .relation_mutations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| unsupported_provider("relation mutation"))
    }

    fn read_providers(&self) -> Result<RegistryReadGuard<'_>> {
        self.providers.read().map_err(|_| GraphError::Internal {
            reason: "provider registry lock poisoned",
        })
    }

    fn write_providers(&self) -> Result<RegistryWriteGuard<'_>> {
        self.providers.write().map_err(|_| GraphError::Internal {
            reason: "provider registry lock poisoned",
        })
    }

    pub fn create_node(&self, node: &NodeId) -> Result<()> {
        self.node_mutation_provider_for_node(node)?
            .create_node(node)?;
        self.emit_change(GraphChange::NodeChanged { node: node.clone() })?;
        self.emit_change(GraphChange::NodeKindChanged {
            kind: node.kind().clone(),
        })
    }

    pub fn remove_node(&self, node: &NodeId) -> Result<()> {
        if !self.contains_node(node)? {
            return Err(GraphError::NotFound {
                kind: "node",
                name: node.to_string(),
            });
        }
        self.remove_inbound_links(node)?;
        self.node_mutation_provider_for_node(node)?
            .remove_node(node)?;
        self.emit_change(GraphChange::NodeRemoved { node: node.clone() })?;
        self.emit_change(GraphChange::NodeKindChanged {
            kind: node.kind().clone(),
        })
    }

    pub fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.node_provider_for_node(node)?.contains_node(node)
    }

    pub fn node_kinds(&self) -> Result<Vec<NodeKind>> {
        let providers = self.read_providers()?;
        Ok(providers.nodes.keys().cloned().collect())
    }

    pub fn nodes(&self) -> Result<Vec<NodeId>> {
        let providers = {
            let providers = self.read_providers()?;
            providers.nodes.values().cloned().collect::<Vec<_>>()
        };
        let mut nodes = Vec::new();
        for provider in providers {
            nodes.extend(provider.nodes()?);
        }
        nodes.sort();
        Ok(nodes)
    }

    pub fn nodes_by_kind(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = self.node_provider_for_kind(kind)?.nodes()?;
        nodes.sort();
        Ok(nodes)
    }

    pub fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        self.property_provider_for_node(subject)?
            .property_spec(subject, key)
    }

    pub fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        self.property_provider_for_node(subject)?
            .properties(subject)
    }

    pub fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.property_provider_for_node(subject)?
            .property(subject, key)
    }

    pub fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        self.property_mutation_provider_for_node(subject)?
            .set_property(subject, key, value)?;
        self.emit_change(GraphChange::PropertyChanged {
            node: subject.clone(),
            key: key.clone(),
        })
    }

    pub fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        self.property_mutation_provider_for_node(subject)?
            .remove_property(subject, key)?;
        self.emit_change(GraphChange::PropertyChanged {
            node: subject.clone(),
            key: key.clone(),
        })
    }

    pub fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.relation_provider_for_node(source)?.relations(source)
    }

    pub fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.relation_provider_for_node(source)?
            .targets(source, relation)
    }

    pub fn set_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        if !self.contains_node(target)? {
            return Err(GraphError::NotFound {
                kind: "node",
                name: target.to_string(),
            });
        }
        self.relation_mutation_provider_for_node(source)?
            .set_link(source, relation, target)?;
        self.emit_change(GraphChange::RelationChanged {
            source: source.clone(),
            relation: relation.clone(),
        })
    }

    pub fn remove_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        self.relation_mutation_provider_for_node(source)?
            .remove_link(source, relation, target)?;
        self.emit_change(GraphChange::RelationChanged {
            source: source.clone(),
            relation: relation.clone(),
        })
    }

    fn remove_inbound_links(&self, target: &NodeId) -> Result<()> {
        let (node_providers, relation_providers, relation_mutations) = {
            let providers = self.read_providers()?;
            (
                providers.nodes.values().cloned().collect::<Vec<_>>(),
                providers.relations.clone(),
                providers.relation_mutations.clone(),
            )
        };

        for node_provider in node_providers {
            for source in node_provider.nodes()? {
                let Some(relation_provider) = relation_providers.get(source.kind()) else {
                    continue;
                };
                let relations = match relation_provider.relations(&source) {
                    Ok(relations) => relations,
                    Err(GraphError::NotFound { .. }) => continue,
                    Err(error) => return Err(error),
                };
                for relation in relations {
                    let targets = match relation_provider.targets(&source, &relation) {
                        Ok(targets) => targets,
                        Err(GraphError::NotFound { .. }) => continue,
                        Err(error) => return Err(error),
                    };
                    if targets.iter().any(|candidate| candidate == target) {
                        let mutation_provider = relation_mutations
                            .get(source.kind())
                            .cloned()
                            .ok_or_else(|| unsupported_provider("relation mutation"))?;
                        match mutation_provider.remove_link(&source, &relation, target) {
                            Ok(()) | Err(GraphError::NotFound { .. }) => {}
                            Err(error) => return Err(error),
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl fmt::Debug for DynamicGraph {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let provider_count = self
            .providers
            .read()
            .map(|providers| {
                providers.nodes.len()
                    + providers.node_mutations.len()
                    + providers.properties.len()
                    + providers.property_mutations.len()
                    + providers.relations.len()
                    + providers.relation_mutations.len()
            })
            .unwrap_or_default();
        formatter
            .debug_struct("DynamicGraph")
            .field("provider_count", &provider_count)
            .finish()
    }
}

fn missing_provider(capability: &'static str, kind: &NodeKind) -> GraphError {
    GraphError::NotFound {
        kind: "provider",
        name: format!("{capability} provider for {kind}"),
    }
}

fn unsupported_provider(capability: &'static str) -> GraphError {
    GraphError::Unsupported {
        operation: match capability {
            "node mutation" => "node mutation provider",
            "property mutation" => "property mutation provider",
            "relation mutation" => "relation mutation provider",
            _ => "provider capability",
        },
    }
}
