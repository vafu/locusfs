use crate::{
    DynamicGraph, GraphChange, GraphError, InMemoryProvider, LocusValue, NodeId, NodeKind,
    NodeProvider, PropertyKey, PropertyProvider, PropertySpec, RelationName, Result, ValueKind,
};

#[test]
fn nodes_are_created_explicitly() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone());
    let node = NodeId::new(kind, "57").unwrap();

    graph.create_node(&node).unwrap();

    assert_eq!(graph.nodes().unwrap(), vec![node]);
}

#[test]
fn properties_round_trip_through_graph_contract() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone());
    let node = NodeId::new(kind, "57").unwrap();
    let key = PropertyKey::new("title").unwrap();

    graph.create_node(&node).unwrap();
    graph
        .set_property(&node, &key, LocusValue::String("value".to_string()))
        .unwrap();

    assert_eq!(
        graph.property(&node, &key).unwrap(),
        LocusValue::String("value".to_string())
    );

    assert_eq!(
        graph
            .properties(&node)
            .unwrap()
            .into_iter()
            .map(|spec| spec.into_key())
            .collect::<Vec<_>>(),
        vec![key.clone()]
    );

    graph.remove_property(&node, &key).unwrap();
    assert!(matches!(
        graph.property(&node, &key).unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[test]
fn links_round_trip_through_graph_contract() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone());
    let source = NodeId::new(kind.clone(), "57").unwrap();
    let relation = RelationName::new("linked-to").unwrap();
    let target = NodeId::new(kind, "6").unwrap();

    graph.create_node(&source).unwrap();
    graph.create_node(&target).unwrap();
    graph.set_link(&source, &relation, &target).unwrap();
    assert_eq!(
        graph.targets(&source, &relation).unwrap(),
        vec![target.clone()]
    );
    assert_eq!(graph.relations(&source).unwrap(), vec![relation.clone()]);

    graph.remove_link(&source, &relation, &target).unwrap();
    assert!(matches!(
        graph.targets(&source, &relation).unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[test]
fn graph_mutations_emit_semantic_changes() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone());
    let changes = graph.subscribe_changes().unwrap();
    let source = NodeId::new(kind.clone(), "57").unwrap();
    let target = NodeId::new(kind, "6").unwrap();
    let key = PropertyKey::new("title").unwrap();
    let relation = RelationName::new("linked-to").unwrap();

    graph.create_node(&source).unwrap();
    graph.create_node(&target).unwrap();
    graph
        .set_property(&source, &key, LocusValue::String("value".to_string()))
        .unwrap();
    graph.set_link(&source, &relation, &target).unwrap();

    let emitted = changes.try_iter().collect::<Vec<_>>();
    assert!(emitted.contains(&GraphChange::NodeChanged {
        node: source.clone()
    }));
    assert!(emitted.contains(&GraphChange::PropertyChanged {
        node: source.clone(),
        key
    }));
    assert!(emitted.contains(&GraphChange::RelationChanged { source, relation }));
}

#[test]
fn removing_node_removes_owned_state_and_inbound_links() {
    let kind = NodeKind::new("node").unwrap();
    let graph = in_memory_graph(kind.clone());
    let source = NodeId::new(kind.clone(), "57").unwrap();
    let target = NodeId::new(kind, "6").unwrap();
    let relation = RelationName::new("linked-to").unwrap();
    let key = PropertyKey::new("title").unwrap();

    graph.create_node(&source).unwrap();
    graph.create_node(&target).unwrap();
    graph
        .set_property(&target, &key, LocusValue::String("value".to_string()))
        .unwrap();
    graph.set_link(&source, &relation, &target).unwrap();

    graph.remove_node(&target).unwrap();

    assert!(!graph.contains_node(&target).unwrap());
    assert!(matches!(
        graph.property(&target, &key).unwrap_err(),
        GraphError::NotFound { .. }
    ));
    assert!(matches!(
        graph.targets(&source, &relation).unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[test]
fn removing_node_removes_cross_provider_inbound_links() {
    let source_kind = NodeKind::new("window").unwrap();
    let target_kind = NodeKind::new("workspace").unwrap();
    let graph = DynamicGraph::new();
    register_in_memory_provider(&graph, source_kind.clone());
    register_in_memory_provider(&graph, target_kind.clone());

    let source = NodeId::new(source_kind, "57").unwrap();
    let target = NodeId::new(target_kind, "1").unwrap();
    let relation = RelationName::new("on-workspace").unwrap();

    graph.create_node(&source).unwrap();
    graph.create_node(&target).unwrap();
    graph.set_link(&source, &relation, &target).unwrap();

    graph.remove_node(&target).unwrap();

    assert!(matches!(
        graph.targets(&source, &relation).unwrap_err(),
        GraphError::NotFound { .. }
    ));
}

#[test]
fn providers_can_be_registered_as_separate_capabilities() {
    let kind = NodeKind::new("workspace").unwrap();
    let node = NodeId::new(kind.clone(), "1").unwrap();
    let key = PropertyKey::new("name").unwrap();
    let graph = DynamicGraph::new();

    graph
        .register_node_provider(StaticNodeProvider {
            kind: kind.clone(),
            node: node.clone(),
        })
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
        .unwrap();

    assert_eq!(graph.nodes().unwrap(), vec![node.clone()]);
    assert_eq!(
        graph.property(&node, &key).unwrap(),
        LocusValue::String("main".to_string())
    );
    assert!(matches!(
        graph
            .set_property(&node, &key, LocusValue::String("other".to_string()))
            .unwrap_err(),
        GraphError::Unsupported { .. }
    ));
}

struct StaticNodeProvider {
    kind: NodeKind,
    node: NodeId,
}

impl NodeProvider for StaticNodeProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn contains_node(&self, node: &NodeId) -> Result<bool> {
        Ok(node == &self.node)
    }

    fn nodes(&self) -> Result<Vec<NodeId>> {
        Ok(vec![self.node.clone()])
    }
}

struct StaticPropertyProvider {
    node: NodeId,
    key: PropertyKey,
    value: LocusValue,
}

impl PropertyProvider for StaticPropertyProvider {
    fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        if subject == &self.node && key == &self.key {
            Ok(PropertySpec::new(key.clone(), ValueKind::String))
        } else {
            Err(GraphError::NotFound {
                kind: "property",
                name: format!("{subject}/{key}"),
            })
        }
    }

    fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        if subject == &self.node {
            Ok(vec![PropertySpec::new(self.key.clone(), self.value.kind())])
        } else {
            Err(GraphError::NotFound {
                kind: "node",
                name: subject.to_string(),
            })
        }
    }

    fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.property_spec(subject, key)?;
        Ok(self.value.clone())
    }
}

fn in_memory_graph(kind: NodeKind) -> DynamicGraph {
    let graph = DynamicGraph::new();
    register_in_memory_provider(&graph, kind);
    graph
}

fn register_in_memory_provider(graph: &DynamicGraph, kind: NodeKind) {
    let provider = InMemoryProvider::new(kind.clone());
    graph.register_node_provider(provider.clone()).unwrap();
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_property_mutation_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .unwrap();
    graph
        .register_relation_mutation_provider(kind, provider)
        .unwrap();
}
