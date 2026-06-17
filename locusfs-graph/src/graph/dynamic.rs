use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use futures_core::Stream;
use tokio::sync::RwLock;
use tokio::sync::broadcast::{self, Receiver, Sender};

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
type RegistryReadGuard<'a> = tokio::sync::RwLockReadGuard<'a, ProviderRegistry>;
type RegistryWriteGuard<'a> = tokio::sync::RwLockWriteGuard<'a, ProviderRegistry>;

pub type GraphChangeReceiver = Receiver<GraphChange>;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GraphChangeStreamError {
    Lagged(u64),
    Closed,
}

pub struct GraphChangeSubscription {
    receiver: GraphChangeReceiver,
}

impl GraphChangeSubscription {
    fn new(receiver: GraphChangeReceiver) -> Self {
        Self { receiver }
    }

    pub async fn recv(&mut self) -> std::result::Result<GraphChange, GraphChangeStreamError> {
        self.receiver.recv().await.map_err(|error| match error {
            broadcast::error::RecvError::Lagged(count) => GraphChangeStreamError::Lagged(count),
            broadcast::error::RecvError::Closed => GraphChangeStreamError::Closed,
        })
    }

    pub fn into_stream(
        self,
    ) -> impl Stream<Item = std::result::Result<GraphChange, GraphChangeStreamError>> {
        futures_util::stream::unfold(self, |mut subscription| async move {
            let item = subscription.recv().await;
            match item {
                Ok(change) => Some((Ok(change), subscription)),
                Err(GraphChangeStreamError::Lagged(count)) => {
                    Some((Err(GraphChangeStreamError::Lagged(count)), subscription))
                }
                Err(GraphChangeStreamError::Closed) => None,
            }
        })
    }
}

#[derive(Clone)]
pub struct DynamicGraph {
    providers: Arc<RwLock<ProviderRegistry>>,
    changes: Sender<GraphChange>,
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
        let (changes, _) = broadcast::channel(1024);
        Self {
            providers: Arc::new(RwLock::new(ProviderRegistry::default())),
            changes,
        }
    }

    pub fn subscribe_changes(&self) -> GraphChangeReceiver {
        self.changes.subscribe()
    }

    pub fn subscribe_change_stream(&self) -> GraphChangeSubscription {
        GraphChangeSubscription::new(self.changes.subscribe())
    }

    pub fn emit_change(&self, change: GraphChange) -> Result<()> {
        let _ = self.changes.send(change);
        Ok(())
    }

    pub async fn register_node_provider<P>(&self, provider: P) -> Result<()>
    where
        P: NodeProvider,
    {
        let kind = provider.kind().clone();
        self.write_providers()
            .await
            .nodes
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub async fn register_node_mutation_provider<P>(
        &self,
        kind: NodeKind,
        provider: P,
    ) -> Result<()>
    where
        P: NodeMutationProvider,
    {
        self.write_providers()
            .await
            .node_mutations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub async fn register_property_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: PropertyProvider,
    {
        self.write_providers()
            .await
            .properties
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub async fn register_property_mutation_provider<P>(
        &self,
        kind: NodeKind,
        provider: P,
    ) -> Result<()>
    where
        P: PropertyMutationProvider,
    {
        self.write_providers()
            .await
            .property_mutations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub async fn register_relation_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: RelationProvider,
    {
        self.write_providers()
            .await
            .relations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub async fn register_relation_mutation_provider<P>(
        &self,
        kind: NodeKind,
        provider: P,
    ) -> Result<()>
    where
        P: RelationMutationProvider,
    {
        self.write_providers()
            .await
            .relation_mutations
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    async fn node_provider_for_kind(&self, kind: &NodeKind) -> Result<Arc<dyn NodeProvider>> {
        let providers = self.read_providers().await;
        providers
            .nodes
            .get(kind)
            .cloned()
            .ok_or_else(|| missing_provider("node", kind))
    }

    async fn node_provider_for_node(&self, node: &NodeId) -> Result<Arc<dyn NodeProvider>> {
        self.node_provider_for_kind(node.kind()).await
    }

    async fn node_mutation_provider_for_node(
        &self,
        node: &NodeId,
    ) -> Result<Arc<dyn NodeMutationProvider>> {
        let providers = self.read_providers().await;
        providers
            .node_mutations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| unsupported_provider("node mutation"))
    }

    async fn property_provider_for_node(&self, node: &NodeId) -> Result<Arc<dyn PropertyProvider>> {
        let providers = self.read_providers().await;
        providers
            .properties
            .get(node.kind())
            .cloned()
            .ok_or_else(|| missing_provider("property", node.kind()))
    }

    async fn property_mutation_provider_for_node(
        &self,
        node: &NodeId,
    ) -> Result<Arc<dyn PropertyMutationProvider>> {
        let providers = self.read_providers().await;
        providers
            .property_mutations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| unsupported_provider("property mutation"))
    }

    async fn relation_provider_for_node(&self, node: &NodeId) -> Result<Arc<dyn RelationProvider>> {
        let providers = self.read_providers().await;
        providers
            .relations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| missing_provider("relation", node.kind()))
    }

    async fn relation_mutation_provider_for_node(
        &self,
        node: &NodeId,
    ) -> Result<Arc<dyn RelationMutationProvider>> {
        let providers = self.read_providers().await;
        providers
            .relation_mutations
            .get(node.kind())
            .cloned()
            .ok_or_else(|| unsupported_provider("relation mutation"))
    }

    async fn read_providers(&self) -> RegistryReadGuard<'_> {
        self.providers.read().await
    }

    async fn write_providers(&self) -> RegistryWriteGuard<'_> {
        self.providers.write().await
    }

    pub async fn create_node(&self, node: &NodeId) -> Result<()> {
        self.node_mutation_provider_for_node(node)
            .await?
            .create_node(node)
            .await?;
        self.emit_change(GraphChange::NodeChanged { node: node.clone() })?;
        self.emit_change(GraphChange::NodeKindChanged {
            kind: node.kind().clone(),
        })
    }

    pub async fn remove_node(&self, node: &NodeId) -> Result<()> {
        if !self.contains_node(node).await? {
            return Err(GraphError::NotFound {
                kind: "node",
                name: node.to_string(),
            });
        }
        self.remove_inbound_links(node).await?;
        self.node_mutation_provider_for_node(node)
            .await?
            .remove_node(node)
            .await?;
        self.emit_change(GraphChange::NodeRemoved { node: node.clone() })?;
        self.emit_change(GraphChange::NodeKindChanged {
            kind: node.kind().clone(),
        })
    }

    pub async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.node_provider_for_node(node)
            .await?
            .contains_node(node)
            .await
    }

    pub async fn node_kinds(&self) -> Result<Vec<NodeKind>> {
        let providers = self.read_providers().await;
        Ok(providers.nodes.keys().cloned().collect())
    }

    pub async fn nodes(&self) -> Result<Vec<NodeId>> {
        let providers = {
            let providers = self.read_providers().await;
            providers.nodes.values().cloned().collect::<Vec<_>>()
        };
        let mut nodes = Vec::new();
        for provider in providers {
            nodes.extend(provider.nodes().await?);
        }
        nodes.sort();
        Ok(nodes)
    }

    pub async fn nodes_by_kind(&self, kind: &NodeKind) -> Result<Vec<NodeId>> {
        let mut nodes = self.node_provider_for_kind(kind).await?.nodes().await?;
        nodes.sort();
        Ok(nodes)
    }

    pub async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        self.property_provider_for_node(subject)
            .await?
            .property_spec(subject, key)
            .await
    }

    pub async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        self.property_provider_for_node(subject)
            .await?
            .properties(subject)
            .await
    }

    pub async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.property_provider_for_node(subject)
            .await?
            .property(subject, key)
            .await
    }

    pub async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        self.property_mutation_provider_for_node(subject)
            .await?
            .set_property(subject, key, value)
            .await?;
        self.emit_change(GraphChange::PropertyChanged {
            node: subject.clone(),
            key: key.clone(),
        })
    }

    pub async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        self.property_mutation_provider_for_node(subject)
            .await?
            .remove_property(subject, key)
            .await?;
        self.emit_change(GraphChange::PropertyChanged {
            node: subject.clone(),
            key: key.clone(),
        })
    }

    pub async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.relation_provider_for_node(source)
            .await?
            .relations(source)
            .await
    }

    pub async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.relation_provider_for_node(source)
            .await?
            .targets(source, relation)
            .await
    }

    pub async fn set_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        if !self.contains_node(target).await? {
            return Err(GraphError::NotFound {
                kind: "node",
                name: target.to_string(),
            });
        }
        self.relation_mutation_provider_for_node(source)
            .await?
            .set_link(source, relation, target)
            .await?;
        self.emit_change(GraphChange::RelationChanged {
            source: source.clone(),
            relation: relation.clone(),
        })
    }

    pub async fn remove_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        self.relation_mutation_provider_for_node(source)
            .await?
            .remove_link(source, relation, target)
            .await?;
        self.emit_change(GraphChange::RelationChanged {
            source: source.clone(),
            relation: relation.clone(),
        })
    }

    async fn remove_inbound_links(&self, target: &NodeId) -> Result<()> {
        let (node_providers, relation_providers, relation_mutations) = {
            let providers = self.read_providers().await;
            (
                providers.nodes.values().cloned().collect::<Vec<_>>(),
                providers.relations.clone(),
                providers.relation_mutations.clone(),
            )
        };

        for node_provider in node_providers {
            for source in node_provider.nodes().await? {
                let Some(relation_provider) = relation_providers.get(source.kind()) else {
                    continue;
                };
                let relations = match relation_provider.relations(&source).await {
                    Ok(relations) => relations,
                    Err(GraphError::NotFound { .. }) => continue,
                    Err(error) => return Err(error),
                };
                for relation in relations {
                    let targets = match relation_provider.targets(&source, &relation).await {
                        Ok(targets) => targets,
                        Err(GraphError::NotFound { .. }) => continue,
                        Err(error) => return Err(error),
                    };
                    if targets.iter().any(|candidate| candidate == target) {
                        let mutation_provider = relation_mutations
                            .get(source.kind())
                            .cloned()
                            .ok_or_else(|| unsupported_provider("relation mutation"))?;
                        match mutation_provider
                            .remove_link(&source, &relation, target)
                            .await
                        {
                            Ok(()) => {
                                self.emit_change(GraphChange::RelationChanged {
                                    source: source.clone(),
                                    relation: relation.clone(),
                                })?;
                            }
                            Err(GraphError::NotFound { .. }) => {}
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
            .try_read()
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
