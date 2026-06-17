use std::collections::HashMap;

use locusfs_graph::{LocusValue, NodeId, NodeKind, PropertyKey, RelationName};
use niri_ipc::{Event, LogicalOutput, Output, Transform, Window, WindowLayout, Workspace};

use super::NiriState;

#[test]
fn projects_live_niri_state_into_nodes_properties_and_relations() {
    let mut state = state_with_output("DP-1");
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();
    state
        .apply_event(Event::WindowsChanged {
            windows: vec![window(7, 42, "Terminal")],
        })
        .unwrap();

    let window = node("window", "7");
    let workspace = node("workspace", "42");
    let output = node("output", "DP-1");
    let selected = node("context", "selected");

    assert!(state.contains_node(&window).unwrap());
    assert_eq!(
        state.property(&window, &property("title")).unwrap(),
        LocusValue::String("Terminal".to_string())
    );
    assert_eq!(
        state.property(&window, &property("selected")).unwrap(),
        LocusValue::Bool(true)
    );
    assert_eq!(
        state.targets(&window, &relation("workspace")).unwrap(),
        vec![workspace.clone()]
    );
    assert_eq!(
        state.targets(&workspace, &relation("output")).unwrap(),
        vec![output]
    );
    assert_eq!(
        state.property(&workspace, &property("selected")).unwrap(),
        LocusValue::Bool(true)
    );
    assert_eq!(
        state.targets(&selected, &relation("workspace")).unwrap(),
        vec![workspace]
    );
    assert_eq!(
        state.targets(&selected, &relation("window")).unwrap(),
        vec![window]
    );
}

#[test]
fn exposes_properties_as_read_only_specs() {
    let mut state = NiriState::new(HashMap::new());
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();

    let spec = state
        .property_spec(&node("workspace", "42"), &property("focused"))
        .unwrap();

    assert_eq!(spec.key(), &property("focused"));
    assert!(spec.is_readable());
    assert!(!spec.is_writable());
}

#[test]
fn empty_registered_kind_lists_as_empty_nodes() {
    let mut state = NiriState::new(HashMap::new());
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();

    assert!(
        state
            .nodes(&NodeKind::new("window").unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn focus_events_update_selected_window_relation_in_place() {
    let mut state = state_with_output("DP-1");
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();
    state
        .apply_event(Event::WindowsChanged {
            windows: vec![window(7, 42, "Terminal"), window(8, 42, "Browser")],
        })
        .unwrap();
    state
        .apply_event(Event::WindowFocusChanged { id: Some(8) })
        .unwrap();

    assert_eq!(
        state
            .targets(&node("context", "selected"), &relation("window"))
            .unwrap(),
        vec![node("window", "8")]
    );
    assert_eq!(
        state
            .property(&node("window", "7"), &property("selected"))
            .unwrap(),
        LocusValue::Bool(false)
    );
    assert_eq!(
        state
            .property(&node("window", "8"), &property("selected"))
            .unwrap(),
        LocusValue::Bool(true)
    );
}

#[test]
fn focus_events_emit_selected_window_property_changes() {
    let mut state = state_with_output("DP-1");
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();
    state
        .apply_event(Event::WindowsChanged {
            windows: vec![window(7, 42, "Terminal"), window(8, 42, "Browser")],
        })
        .unwrap();

    let changes = state
        .apply_event(Event::WindowFocusChanged { id: Some(8) })
        .unwrap();

    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("selected"),
        })
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "8"),
            key: property("selected"),
        })
    );
}

#[test]
fn workspace_activation_updates_selected_workspace_property() {
    let mut state = state_with_output("DP-1");
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![
                workspace(42, "DP-1"),
                workspace_with_focus(43, "DP-1", false),
            ],
        })
        .unwrap();

    let changes = state
        .apply_event(Event::WorkspaceActivated {
            id: 43,
            focused: true,
        })
        .unwrap();

    assert_eq!(
        state
            .property(&node("workspace", "42"), &property("selected"))
            .unwrap(),
        LocusValue::Bool(false)
    );
    assert_eq!(
        state
            .property(&node("workspace", "43"), &property("selected"))
            .unwrap(),
        LocusValue::Bool(true)
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("workspace", "42"),
            key: property("selected"),
        })
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("workspace", "43"),
            key: property("selected"),
        })
    );
}

fn state_with_output(name: &str) -> NiriState {
    NiriState::new(HashMap::from([(name.to_string(), output(name))]))
}

fn output(name: &str) -> Output {
    Output {
        name: name.to_string(),
        make: "Acme".to_string(),
        model: "Panel".to_string(),
        serial: Some("serial".to_string()),
        physical_size: Some((600, 340)),
        modes: Vec::new(),
        current_mode: None,
        is_custom_mode: false,
        vrr_supported: true,
        vrr_enabled: false,
        logical: Some(LogicalOutput {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            transform: Transform::Normal,
        }),
    }
}

fn workspace(id: u64, output: &str) -> Workspace {
    workspace_with_focus(id, output, true)
}

fn workspace_with_focus(id: u64, output: &str, focused: bool) -> Workspace {
    Workspace {
        id,
        idx: 1,
        name: Some("web".to_string()),
        output: Some(output.to_string()),
        is_urgent: false,
        is_active: true,
        is_focused: focused,
        active_window_id: Some(7),
    }
}

fn window(id: u64, workspace_id: u64, title: &str) -> Window {
    Window {
        id,
        title: Some(title.to_string()),
        app_id: Some("foot".to_string()),
        pid: Some(123),
        workspace_id: Some(workspace_id),
        is_focused: id == 7,
        is_floating: false,
        is_urgent: false,
        layout: WindowLayout {
            pos_in_scrolling_layout: Some((1, 2)),
            tile_size: (800.0, 600.0),
            window_size: (800, 580),
            tile_pos_in_workspace_view: None,
            window_offset_in_tile: (0.0, 20.0),
        },
        focus_timestamp: None,
    }
}

fn node(kind: &str, local: &str) -> NodeId {
    NodeId::new(NodeKind::new(kind).unwrap(), local).unwrap()
}

fn property(key: &str) -> PropertyKey {
    PropertyKey::new(key).unwrap()
}

fn relation(name: &str) -> RelationName {
    RelationName::new(name).unwrap()
}
