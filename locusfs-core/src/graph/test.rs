use std::path::PathBuf;

use super::*;

#[test]
fn properties_round_trip_through_graph_contract() {
    let graph = InMemoryGraph::new();
    let node = NodeId::new("window:57").unwrap();
    let key = PropertyKey::new("title").unwrap();

    graph
        .set_property(&node, &key, LocusValue::String("Ghostty".to_string()))
        .unwrap();

    assert_eq!(
        graph.property(&node, &key).unwrap(),
        Some(LocusValue::String("Ghostty".to_string()))
    );
}

#[test]
fn links_round_trip_through_graph_contract() {
    let graph = InMemoryGraph::new();
    let source = NodeId::new("window:57").unwrap();
    let relation = RelationName::new("workspace").unwrap();
    let target = NodeId::new("workspace:6").unwrap();

    graph.set_link(&source, &relation, &target).unwrap();
    assert_eq!(
        graph.targets(&source, &relation).unwrap(),
        vec![target.clone()]
    );

    graph.remove_link(&source, &relation, &target).unwrap();
    assert!(graph.targets(&source, &relation).unwrap().is_empty());
}

#[test]
fn project_symlink_operation_creates_project_node() {
    let graph = InMemoryGraph::new();
    let project = ProjectName::new("my-project").unwrap();

    graph
        .upsert_project_link(&project, PathBuf::from("/tmp/my-project"))
        .unwrap();

    let entry = graph.project(&project).unwrap().unwrap();
    assert_eq!(entry.node.as_str(), "project:my-project");
    assert_eq!(entry.root, PathBuf::from("/tmp/my-project"));

    let name = PropertyKey::new("name").unwrap();
    assert_eq!(
        graph.project_property(&project, &name).unwrap(),
        Some(LocusValue::String("my-project".to_string()))
    );
}

#[test]
fn project_name_write_updates_display_name() {
    let graph = InMemoryGraph::new();
    let project = ProjectName::new("my-project").unwrap();
    let name = PropertyKey::new("name").unwrap();

    graph
        .upsert_project_link(&project, "/tmp/my-project")
        .unwrap();
    graph
        .set_project_property(&project, &name, "Display Name\n")
        .unwrap();

    assert_eq!(
        graph.project_property(&project, &name).unwrap(),
        Some(LocusValue::String("Display Name".to_string()))
    );
}

#[test]
fn project_link_update_preserves_display_name() {
    let graph = InMemoryGraph::new();
    let project = ProjectName::new("my-project").unwrap();
    let name = PropertyKey::new("name").unwrap();

    graph.upsert_project_link(&project, "/tmp/old").unwrap();
    graph
        .set_project_property(&project, &name, "Display Name\n")
        .unwrap();
    graph.upsert_project_link(&project, "/tmp/new").unwrap();

    assert_eq!(
        graph.project(&project).unwrap().unwrap().root,
        PathBuf::from("/tmp/new")
    );
    assert_eq!(
        graph.project_property(&project, &name).unwrap(),
        Some(LocusValue::String("Display Name".to_string()))
    );
}
