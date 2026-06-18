use crate::{
    DynamicGraph, GraphChange, GraphError, GraphWatchEvent, GraphWatchTarget, InMemoryProvider,
    LocusValue, NodeAccess, NodeId, NodeKind, NodeProvider, PropertyKey, PropertyProvider,
    PropertySpec, RelationName, Result, TracedProvider, ValueKind,
};

use async_trait::async_trait;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn node_provider_access_defaults_to_read_only() {
    let kind = NodeKind::new("node").unwrap();
    let node = NodeId::new(kind.clone(), "57").unwrap();
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(StaticNodeProvider {
            kind: kind.clone(),
            node: node.clone(),
        })
        .await
        .unwrap();

    assert_eq!(
        graph.kind_access(&kind).await.unwrap(),
        NodeAccess::read_only()
    );
    assert_eq!(
        graph.node_access(&node).await.unwrap(),
        NodeAccess::read_only()
    );
}

#[tokio::test]
async fn in_memory_provider_declares_read_write_node_access() {
    let kind = NodeKind::new("node").unwrap();
    let node = NodeId::new(kind.clone(), "57").unwrap();
    let graph = in_memory_graph(kind.clone()).await;

    graph.create_node(&node).await.unwrap();

    assert_eq!(
        graph.kind_access(&kind).await.unwrap(),
        NodeAccess::read_write()
    );
    assert_eq!(
        graph.node_access(&node).await.unwrap(),
        NodeAccess::read_write()
    );
}

#[tokio::test]
async fn traced_provider_forwards_node_access() {
    let kind = NodeKind::new("node").unwrap();
    let provider = TracedProvider::new("test", InMemoryProvider::new(kind.clone()));
    let graph = DynamicGraph::new();
    graph.register_node_provider(provider).await.unwrap();

    assert_eq!(
        graph.kind_access(&kind).await.unwrap(),
        NodeAccess::read_write()
    );
}

#[tokio::test]
async fn nodes_are_created_explicitly() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let node = NodeId::new(kind, "57").unwrap();

    graph.create_node(&node).await.unwrap();

    assert_eq!(graph.nodes().await.unwrap(), vec![node]);
}

#[tokio::test]
async fn properties_round_trip_through_graph_contract() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let node = NodeId::new(kind, "57").unwrap();
    let key = PropertyKey::new("title").unwrap();

    graph.create_node(&node).await.unwrap();
    graph
        .set_property(&node, &key, LocusValue::String("value".to_string()))
        .await
        .unwrap();

    assert_eq!(
        graph.property(&node, &key).await.unwrap(),
        LocusValue::String("value".to_string())
    );

    assert_eq!(
        graph
            .properties(&node)
            .await
            .unwrap()
            .into_iter()
            .map(|spec| spec.into_key())
            .collect::<Vec<_>>(),
        vec![key.clone()]
    );

    graph.remove_property(&node, &key).await.unwrap();
    assert!(matches!(
        graph.property(&node, &key).await.unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[tokio::test]
async fn links_round_trip_through_graph_contract() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let source = NodeId::new(kind.clone(), "57").unwrap();
    let relation = RelationName::new("linked-to").unwrap();
    let target = NodeId::new(kind, "6").unwrap();

    graph.create_node(&source).await.unwrap();
    graph.create_node(&target).await.unwrap();
    graph.set_link(&source, &relation, &target).await.unwrap();
    assert_eq!(
        graph.targets(&source, &relation).await.unwrap(),
        vec![target.clone()]
    );
    assert_eq!(
        graph.relations(&source).await.unwrap(),
        vec![relation.clone()]
    );

    graph
        .remove_link(&source, &relation, &target)
        .await
        .unwrap();
    assert!(matches!(
        graph.targets(&source, &relation).await.unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[tokio::test]
async fn links_fallback_to_overlay_when_source_provider_is_read_only() {
    let workspace_kind = NodeKind::new("workspace").unwrap();
    let project_kind = NodeKind::new("project").unwrap();
    let workspace = NodeId::new(workspace_kind.clone(), "1").unwrap();
    let project = NodeId::new(project_kind.clone(), "locusfs").unwrap();
    let relation = RelationName::new("project").unwrap();
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(StaticNodeProvider {
            kind: workspace_kind,
            node: workspace.clone(),
        })
        .await
        .unwrap();
    register_in_memory_provider(&graph, project_kind).await;
    graph.create_node(&project).await.unwrap();

    graph
        .set_link(&workspace, &relation, &project)
        .await
        .unwrap();

    assert_eq!(
        graph.relations(&workspace).await.unwrap(),
        vec![relation.clone()]
    );
    assert_eq!(
        graph.targets(&workspace, &relation).await.unwrap(),
        vec![project.clone()]
    );

    graph
        .remove_link(&workspace, &relation, &project)
        .await
        .unwrap();
    assert!(matches!(
        graph.targets(&workspace, &relation).await.unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[tokio::test]
async fn overlay_relations_require_existing_source_and_target_nodes() {
    let workspace_kind = NodeKind::new("workspace").unwrap();
    let project_kind = NodeKind::new("project").unwrap();
    let workspace = NodeId::new(workspace_kind.clone(), "1").unwrap();
    let project = NodeId::new(project_kind, "locusfs").unwrap();
    let relation = RelationName::new("project").unwrap();
    let graph = DynamicGraph::new();
    graph
        .register_node_provider(StaticNodeProvider {
            kind: workspace_kind,
            node: workspace.clone(),
        })
        .await
        .unwrap();

    assert!(matches!(
        graph
            .set_link(&workspace, &relation, &project)
            .await
            .unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[tokio::test]
async fn graph_mutations_emit_semantic_changes() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let mut changes = graph.subscribe_global_changes();
    let source = NodeId::new(kind.clone(), "57").unwrap();
    let target = NodeId::new(kind, "6").unwrap();
    let key = PropertyKey::new("title").unwrap();
    let relation = RelationName::new("linked-to").unwrap();

    graph.create_node(&source).await.unwrap();
    graph.create_node(&target).await.unwrap();
    graph
        .set_property(&source, &key, LocusValue::String("value".to_string()))
        .await
        .unwrap();
    graph.set_link(&source, &relation, &target).await.unwrap();

    let mut emitted = Vec::new();
    while let Ok(change) = changes.try_recv() {
        emitted.push(change);
    }
    assert!(emitted.contains(&GraphChange::NodeAdded {
        node: source.clone()
    }));
    assert!(emitted.contains(&GraphChange::PropertyChanged {
        node: source.clone(),
        key
    }));
    assert!(emitted.contains(&GraphChange::RelationChanged { source, relation }));
}

#[tokio::test]
async fn graph_watch_fallback_maps_kind_node_events() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let node = NodeId::new(kind.clone(), "57").unwrap();
    let mut watch = graph.watch(GraphWatchTarget::Kind(kind)).await.unwrap();

    graph.create_node(&node).await.unwrap();

    let event = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(event) = watch.try_recv() {
                break event;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(event, GraphWatchEvent::NodeAdded(node));
}

#[tokio::test]
async fn graph_watch_fallback_filters_property_events() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let node = NodeId::new(kind, "57").unwrap();
    let watched = PropertyKey::new("title").unwrap();
    let other = PropertyKey::new("subtitle").unwrap();
    graph.create_node(&node).await.unwrap();
    let mut watch = graph
        .watch(GraphWatchTarget::Property(node.clone(), watched.clone()))
        .await
        .unwrap();

    graph
        .set_property(&node, &other, LocusValue::String("ignore".to_string()))
        .await
        .unwrap();
    graph
        .set_property(&node, &watched, LocusValue::String("value".to_string()))
        .await
        .unwrap();

    let event = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(event) = watch.try_recv() {
                break event;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(event, GraphWatchEvent::Change);
    assert!(watch.try_recv().is_none());
}

#[tokio::test]
async fn graph_watch_fallback_property_watch_ignores_broad_node_changes() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let node = NodeId::new(kind, "57").unwrap();
    let watched = PropertyKey::new("selected").unwrap();
    graph.create_node(&node).await.unwrap();
    let mut watch = graph
        .watch(GraphWatchTarget::Property(node.clone(), watched.clone()))
        .await
        .unwrap();

    graph
        .emit_global_change(GraphChange::NodeChanged { node: node.clone() })
        .unwrap();
    graph
        .set_property(&node, &watched, LocusValue::Bool(true))
        .await
        .unwrap();

    let event = timeout(Duration::from_secs(1), async {
        loop {
            if let Some(event) = watch.try_recv() {
                break event;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(event, GraphWatchEvent::Change);
    assert!(watch.try_recv().is_none());
}

#[tokio::test]
async fn removing_node_removes_owned_state_and_inbound_links() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone()).await;
    let source = NodeId::new(kind.clone(), "57").unwrap();
    let target = NodeId::new(kind, "6").unwrap();
    let relation = RelationName::new("linked-to").unwrap();
    let key = PropertyKey::new("title").unwrap();

    graph.create_node(&source).await.unwrap();
    graph.create_node(&target).await.unwrap();
    graph
        .set_property(&target, &key, LocusValue::String("value".to_string()))
        .await
        .unwrap();
    graph.set_link(&source, &relation, &target).await.unwrap();

    graph.remove_node(&target).await.unwrap();

    assert!(!graph.contains_node(&target).await.unwrap());
    assert!(matches!(
        graph.property(&target, &key).await.unwrap_err(),
        GraphError::NotFound { .. }
    ));
    assert!(matches!(
        graph.targets(&source, &relation).await.unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[tokio::test]
async fn removing_node_removes_cross_provider_inbound_links() {
    let source_kind = NodeKind::new("window").unwrap();
    let target_kind = NodeKind::new("workspace").unwrap();
    let graph = DynamicGraph::new();
    register_in_memory_provider(&graph, source_kind.clone()).await;
    register_in_memory_provider(&graph, target_kind.clone()).await;

    let source = NodeId::new(source_kind, "57").unwrap();
    let target = NodeId::new(target_kind, "1").unwrap();
    let relation = RelationName::new("on-workspace").unwrap();

    graph.create_node(&source).await.unwrap();
    graph.create_node(&target).await.unwrap();
    graph.set_link(&source, &relation, &target).await.unwrap();

    graph.remove_node(&target).await.unwrap();

    assert!(matches!(
        graph.targets(&source, &relation).await.unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[tokio::test]
async fn providers_can_be_registered_as_separate_capabilities() {
    let kind = NodeKind::new("workspace").unwrap();
    let node = NodeId::new(kind.clone(), "1").unwrap();
    let key = PropertyKey::new("name").unwrap();
    let graph = DynamicGraph::new();

    graph
        .register_node_provider(StaticNodeProvider {
            kind: kind.clone(),
            node: node.clone(),
        })
        .await
        .unwrap();
    graph
        .register_property_provider(
            kind,
            StaticPropertyProvider {
                node: node.clone(),
                key: key.clone(),
                value: LocusValue::String("main".to_string()),
            },
        )
        .await
        .unwrap();

    assert_eq!(graph.nodes().await.unwrap(), vec![node.clone()]);
    assert_eq!(
        graph.property(&node, &key).await.unwrap(),
        LocusValue::String("main".to_string())
    );
    assert!(matches!(
        graph
            .set_property(&node, &key, LocusValue::String("other".to_string()))
            .await
            .unwrap_err(),
        GraphError::Unsupported { .. }
    ));
}

struct StaticNodeProvider {
    kind: NodeKind,
    node: NodeId,
}

#[async_trait]
impl NodeProvider for StaticNodeProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(node == &self.node)
    }

    async fn nodes(&self) -> Result<Vec<NodeId>> {
        Ok(vec![self.node.clone()])
    }
}

struct StaticPropertyProvider {
    node: NodeId,
    key: PropertyKey,
    value: LocusValue,
}

#[async_trait]
impl PropertyProvider for StaticPropertyProvider {
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        if subject == &self.node && key == &self.key {
            Ok(PropertySpec::new(key.clone(), ValueKind::String))
        } else {
            Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
        }
    }

    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        if subject == &self.node {
            Ok(vec![PropertySpec::new(self.key.clone(), self.value.kind())])
        } else {
            Err(GraphError::NotFound {
                kind: "node",
                name: subject.to_string(),
            })
        }
    }

    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.property_spec(subject, key).await?;
        Ok(self.value.clone())
    }
}

async fn in_memory_graph(kind: NodeKind) -> DynamicGraph {
    let graph = DynamicGraph::new();
    register_in_memory_provider(&graph, kind).await;
    graph
}

async fn register_in_memory_provider(graph: &DynamicGraph, kind: NodeKind) {
    let provider = InMemoryProvider::new(kind.clone());
    graph
        .register_node_provider(provider.clone())
        .await
        .unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_property_mutation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .await
        .unwrap();
    graph
        .register_relation_mutation_provider(kind, provider)
        .await
        .unwrap();
}
