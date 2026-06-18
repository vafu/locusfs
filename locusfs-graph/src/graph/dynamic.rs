use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use futures_core::Stream;
use tokio::sync::broadcast::{self, Receiver, Sender};
use tokio::sync::{RwLock, mpsc};

use crate::{
    GraphChange, GraphError, LocusValue, NodeId, NodeKind, PropertyKey, PropertySpec, RelationName,
    Result,
};

use super::{
    GraphWatch, GraphWatchEvent, GraphWatchTarget, NodeAccess, NodeMutationProvider, NodeProvider,
    PropertyMutationProvider, PropertyProvider, RelationMutationProvider, RelationProvider,
    WatchProvider,
};

type NodeProviders = BTreeMap<NodeKind, Arc<dyn NodeProvider>>;
type NodeMutationProviders = BTreeMap<NodeKind, Arc<dyn NodeMutationProvider>>;
type PropertyProviders = BTreeMap<NodeKind, Arc<dyn PropertyProvider>>;
type PropertyMutationProviders = BTreeMap<NodeKind, Arc<dyn PropertyMutationProvider>>;
type RelationProviders = BTreeMap<NodeKind, Arc<dyn RelationProvider>>;
type RelationMutationProviders = BTreeMap<NodeKind, Arc<dyn RelationMutationProvider>>;
type WatchProviders = BTreeMap<NodeKind, Arc<dyn WatchProvider>>;
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
    overlay: Arc<RwLock<RelationOverlay>>,
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
    watches: WatchProviders,
}

#[derive(Clone, Default)]
struct RelationOverlay {
    links: BTreeMap<NodeId, BTreeMap<RelationName, BTreeSet<NodeId>>>,
}

impl DynamicGraph {
    pub fn new() -> Self {
        let (changes, _) = broadcast::channel(1024);
        Self {
            providers: Arc::new(RwLock::new(ProviderRegistry::default())),
            overlay: Arc::new(RwLock::new(RelationOverlay::default())),
            changes,
        }
    }

    pub fn subscribe_global_changes(&self) -> GraphChangeReceiver {
        self.changes.subscribe()
    }

    pub fn subscribe_changes(&self) -> GraphChangeReceiver {
        self.subscribe_global_changes()
    }

    pub fn subscribe_global_change_stream(&self) -> GraphChangeSubscription {
        GraphChangeSubscription::new(self.changes.subscribe())
    }

    pub fn subscribe_change_stream(&self) -> GraphChangeSubscription {
        self.subscribe_global_change_stream()
    }

    pub fn emit_global_change(&self, change: GraphChange) -> Result<()> {
        let _ = self.changes.send(change);
        Ok(())
    }

    pub fn emit_change(&self, change: GraphChange) -> Result<()> {
        self.emit_global_change(change)
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

    pub async fn register_watch_provider<P>(&self, kind: NodeKind, provider: P) -> Result<()>
    where
        P: WatchProvider,
    {
        self.write_providers()
            .await
            .watches
            .insert(kind, Arc::new(provider));
        Ok(())
    }

    pub async fn watch(&self, target: GraphWatchTarget) -> Result<GraphWatch> {
        if let Some(provider) = self.watch_provider_for_target(&target).await {
            return provider.watch(target).await;
        }
        Ok(self.fallback_watch(target))
    }

    async fn watch_provider_for_target(
        &self,
        target: &GraphWatchTarget,
    ) -> Option<Arc<dyn WatchProvider>> {
        let kind = match target {
            GraphWatchTarget::Kind(kind) => kind,
            GraphWatchTarget::Node(node) | GraphWatchTarget::Property(node, _) => node.kind(),
        };
        self.read_providers().await.watches.get(kind).cloned()
    }

    fn fallback_watch(&self, target: GraphWatchTarget) -> GraphWatch {
        let mut changes = self.subscribe_global_changes();
        let (sender, receiver) = mpsc::channel::<GraphWatchEvent>(64);
        tokio::spawn(async move {
            loop {
                let change = match changes.recv().await {
                    Ok(change) => change,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = sender.send(GraphWatchEvent::Change).await;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(event) = watch_event_for_change(&target, change) else {
                    continue;
                };
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        GraphWatch::new(receiver)
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
        self.emit_global_change(GraphChange::NodeAdded { node: node.clone() })?;
        self.emit_global_change(GraphChange::NodeKindChanged {
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
        self.emit_global_change(GraphChange::NodeRemoved { node: node.clone() })?;
        self.emit_global_change(GraphChange::NodeKindChanged {
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

    pub async fn kind_access(&self, kind: &NodeKind) -> Result<NodeAccess> {
        Ok(self.node_provider_for_kind(kind).await?.access())
    }

    pub async fn node_access(&self, node: &NodeId) -> Result<NodeAccess> {
        Ok(self.node_provider_for_node(node).await?.access())
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
        self.emit_global_change(GraphChange::PropertyChanged {
            node: subject.clone(),
            key: key.clone(),
        })
    }

    pub async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        self.property_mutation_provider_for_node(subject)
            .await?
            .remove_property(subject, key)
            .await?;
        self.emit_global_change(GraphChange::PropertyChanged {
            node: subject.clone(),
            key: key.clone(),
        })
    }

    pub async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        let mut relations = match self.relation_provider_for_node(source).await {
            Ok(provider) => match provider.relations(source).await {
                Ok(relations) => relations,
                Err(GraphError::NotFound { .. }) => Vec::new(),
                Err(error) => return Err(error),
            },
            Err(GraphError::NotFound { .. }) => Vec::new(),
            Err(error) => return Err(error),
        };
        relations.extend(self.overlay_relations(source).await);
        relations.sort();
        relations.dedup();
        Ok(relations)
    }

    pub async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        let provider_result = match self.relation_provider_for_node(source).await {
            Ok(provider) => provider.targets(source, relation).await,
            Err(error) => Err(error),
        };
        let mut targets = match provider_result {
            Ok(targets) => targets,
            Err(GraphError::NotFound { .. }) => Vec::new(),
            Err(error) => return Err(error),
        };
        targets.extend(self.overlay_targets(source, relation).await);
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            Err(GraphError::NotFound {
                kind: "relation",
                name: format!("{source}/{relation}"),
            })
        } else {
            Ok(targets)
        }
    }

    pub async fn set_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> Result<()> {
        if !self.contains_node(source).await? {
            return Err(GraphError::NotFound {
                kind: "node",
                name: source.to_string(),
            });
        }
        if !self.contains_node(target).await? {
            return Err(GraphError::NotFound {
                kind: "node",
                name: target.to_string(),
            });
        }
        match self.relation_mutation_provider_for_node(source).await {
            Ok(provider) => match provider.set_link(source, relation, target).await {
                Ok(()) => {}
                Err(GraphError::Unsupported { .. }) => {
                    self.set_overlay_link(source, relation, target).await;
                }
                Err(error) => return Err(error),
            },
            Err(GraphError::Unsupported { .. }) | Err(GraphError::NotFound { .. }) => {
                self.set_overlay_link(source, relation, target).await;
            }
            Err(error) => return Err(error),
        }
        self.emit_global_change(GraphChange::RelationChanged {
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
        if self.remove_overlay_link(source, relation, target).await {
            self.emit_global_change(GraphChange::RelationChanged {
                source: source.clone(),
                relation: relation.clone(),
            })?;
            return Ok(());
        }
        self.relation_mutation_provider_for_node(source)
            .await?
            .remove_link(source, relation, target)
            .await?;
        self.emit_global_change(GraphChange::RelationChanged {
            source: source.clone(),
            relation: relation.clone(),
        })
    }

    async fn remove_inbound_links(&self, target: &NodeId) -> Result<()> {
        let overlay_changes = self.remove_overlay_inbound_links(target).await;
        for (source, relation) in overlay_changes {
            self.emit_global_change(GraphChange::RelationChanged { source, relation })?;
        }

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
                                self.emit_global_change(GraphChange::RelationChanged {
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

    async fn overlay_relations(&self, source: &NodeId) -> Vec<RelationName> {
        let overlay = self.overlay.read().await;
        overlay
            .links
            .get(source)
            .map(|relations| relations.keys().cloned().collect())
            .unwrap_or_default()
    }

    async fn overlay_targets(&self, source: &NodeId, relation: &RelationName) -> Vec<NodeId> {
        let overlay = self.overlay.read().await;
        overlay
            .links
            .get(source)
            .and_then(|relations| relations.get(relation))
            .map(|targets| targets.iter().cloned().collect())
            .unwrap_or_default()
    }

    async fn set_overlay_link(&self, source: &NodeId, relation: &RelationName, target: &NodeId) {
        self.overlay
            .write()
            .await
            .links
            .entry(source.clone())
            .or_default()
            .entry(relation.clone())
            .or_default()
            .insert(target.clone());
    }

    async fn remove_overlay_link(
        &self,
        source: &NodeId,
        relation: &RelationName,
        target: &NodeId,
    ) -> bool {
        let mut overlay = self.overlay.write().await;
        let Some(relations) = overlay.links.get_mut(source) else {
            return false;
        };
        let Some(targets) = relations.get_mut(relation) else {
            return false;
        };
        let removed = targets.remove(target);
        if targets.is_empty() {
            relations.remove(relation);
        }
        if relations.is_empty() {
            overlay.links.remove(source);
        }
        removed
    }

    async fn remove_overlay_inbound_links(&self, target: &NodeId) -> Vec<(NodeId, RelationName)> {
        let mut overlay = self.overlay.write().await;
        let mut changed = Vec::new();
        let mut empty_sources = Vec::new();
        for (source, relations) in &mut overlay.links {
            let mut empty_relations = Vec::new();
            for (relation, targets) in relations.iter_mut() {
                if targets.remove(target) {
                    changed.push((source.clone(), relation.clone()));
                }
                if targets.is_empty() {
                    empty_relations.push(relation.clone());
                }
            }
            for relation in empty_relations {
                relations.remove(&relation);
            }
            if relations.is_empty() {
                empty_sources.push(source.clone());
            }
        }
        for source in empty_sources {
            overlay.links.remove(&source);
        }
        changed
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
                    + providers.watches.len()
            })
            .unwrap_or_default();
        formatter
            .debug_struct("DynamicGraph")
            .field("provider_count", &provider_count)
            .finish()
    }
}

fn watch_event_for_change(
    target: &GraphWatchTarget,
    change: GraphChange,
) -> Option<GraphWatchEvent> {
    match (target, change) {
        (GraphWatchTarget::Kind(kind), GraphChange::NodeAdded { node }) if node.kind() == kind => {
            Some(GraphWatchEvent::NodeAdded(node))
        }
        (GraphWatchTarget::Kind(kind), GraphChange::NodeChanged { node })
            if node.kind() == kind =>
        {
            Some(GraphWatchEvent::NodeChanged(node))
        }
        (GraphWatchTarget::Kind(kind), GraphChange::NodeRemoved { node })
            if node.kind() == kind =>
        {
            Some(GraphWatchEvent::NodeRemoved(node))
        }
        (GraphWatchTarget::Node(watched), GraphChange::NodeAdded { node }) if &node == watched => {
            Some(GraphWatchEvent::NodeAdded(node))
        }
        (GraphWatchTarget::Node(watched), GraphChange::NodeChanged { node })
            if &node == watched =>
        {
            Some(GraphWatchEvent::NodeChanged(node))
        }
        (GraphWatchTarget::Node(watched), GraphChange::NodeRemoved { node })
            if &node == watched =>
        {
            Some(GraphWatchEvent::NodeRemoved(node))
        }
        (
            GraphWatchTarget::Property(watched, key),
            GraphChange::PropertyChanged { node, key: changed },
        ) if &node == watched && &changed == key => Some(GraphWatchEvent::Change),
        (GraphWatchTarget::Property(watched, _), GraphChange::NodeRemoved { node })
            if &node == watched =>
        {
            Some(GraphWatchEvent::Change)
        }
        _ => None,
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
