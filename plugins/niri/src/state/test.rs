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
fn workspace_properties_do_not_expose_idx_alias() {
    let mut state = NiriState::new(HashMap::new());
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();

    assert!(
        state
            .property(&node("workspace", "42"), &property("idx"))
            .is_err()
    );
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
fn active_window_then_focus_event_does_not_emit_duplicate_selected_changes() {
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

    let active_window_changes = state
        .apply_event(Event::WorkspaceActiveWindowChanged {
            workspace_id: 42,
            active_window_id: Some(8),
        })
        .unwrap();
    assert!(
        active_window_changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("selected"),
        })
    );
    assert!(
        active_window_changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "8"),
            key: property("selected"),
        })
    );

    let focus_changes = state
        .apply_event(Event::WindowFocusChanged { id: Some(8) })
        .unwrap();
    assert!(
        !focus_changes.iter().any(|change| matches!(
            change,
            locusfs_graph::GraphChange::PropertyChanged { node, key }
                if node.kind().as_str() == "window" && key.as_str() == "selected"
        )),
        "focus event should not repeat selected property changes after active-window event"
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

#[test]
fn window_opened_emits_node_added() {
    let mut state = state_with_output("DP-1");
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1")],
        })
        .unwrap();

    let changes = state
        .apply_event(Event::WindowOpenedOrChanged {
            window: window(7, 42, "Terminal"),
        })
        .unwrap();

    assert!(changes.contains(&locusfs_graph::GraphChange::NodeAdded {
        node: node("window", "7"),
    }));
}

#[test]
fn windows_changed_emits_node_removed_for_missing_old_window() {
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
        .apply_event(Event::WindowsChanged {
            windows: vec![window(8, 42, "Browser")],
        })
        .unwrap();

    assert!(changes.contains(&locusfs_graph::GraphChange::NodeRemoved {
        node: node("window", "7"),
    }));
}

#[test]
fn windows_changed_emits_property_changes_for_reordered_window() {
    let mut state = state_with_output("DP-1");
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![workspace(42, "DP-1"), workspace(43, "DP-1")],
        })
        .unwrap();
    state
        .apply_event(Event::WindowsChanged {
            windows: vec![window(7, 42, "Terminal")],
        })
        .unwrap();

    let mut moved = window(7, 43, "Terminal");
    moved.layout.pos_in_scrolling_layout = Some((3, 4));
    let changes = state
        .apply_event(Event::WindowsChanged {
            windows: vec![moved],
        })
        .unwrap();

    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("workspace-id"),
        })
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("column"),
        })
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("row"),
        })
    );
}

#[test]
fn window_layouts_changed_emits_position_property_changes() {
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

    let mut layout = window(7, 42, "Terminal").layout;
    layout.pos_in_scrolling_layout = Some((5, 6));
    let changes = state
        .apply_event(Event::WindowLayoutsChanged {
            changes: vec![(7, layout)],
        })
        .unwrap();

    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("column"),
        })
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("window", "7"),
            key: property("row"),
        })
    );
}

#[test]
fn workspaces_changed_emits_property_changes_for_reordered_workspace() {
    let mut state = state_with_output("DP-1");
    let mut first = workspace(42, "DP-1");
    first.idx = 1;
    first.name = None;
    state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![first.clone()],
        })
        .unwrap();

    let mut reordered = first;
    reordered.idx = 3;
    let changes = state
        .apply_event(Event::WorkspacesChanged {
            workspaces: vec![reordered],
        })
        .unwrap();

    assert_eq!(
        state
            .property(&node("workspace", "42"), &property("index"))
            .unwrap(),
        LocusValue::U32(3)
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("workspace", "42"),
            key: property("index"),
        })
    );
    assert!(
        changes.contains(&locusfs_graph::GraphChange::PropertyChanged {
            node: node("workspace", "42"),
            key: property("name"),
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
