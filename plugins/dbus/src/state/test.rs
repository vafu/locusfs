use std::collections::BTreeMap;

use locusfs_graph::{
    GraphPathDirectory, GraphPathEntry, GraphWatchTarget, LocusValue, NodeId, NodeKind,
    PropertyKey, RelationName,
};

use super::{
    BusKind, DBUS_METHOD_KIND, DBUS_OBJECT_KIND, DBUS_SERVICE_KIND, DbusMethodSnapshot,
    DbusPropertySnapshot, DbusState, ServiceConfig, object_snapshot, service_node,
};

#[test]
fn exposes_configured_bus_node() {
    let state = test_state();
    let kind = NodeKind::new(DBUS_SERVICE_KIND).unwrap();
    let service = service_node("system").unwrap();

    assert_eq!(state.nodes(&kind).unwrap(), vec![service.clone()]);
    assert!(state.contains_node(&service).unwrap());
}

#[test]
fn inactive_bus_properties_list_services_without_active_services() {
    let state = test_state();
    let node = service_node("system").unwrap();

    assert_eq!(
        state
            .property(&node, &PropertyKey::new("services").unwrap())
            .unwrap(),
        LocusValue::String("power".to_string())
    );
    assert_eq!(
        state
            .property(&node, &PropertyKey::new("active-services").unwrap())
            .unwrap(),
        LocusValue::String(String::new())
    );
    assert!(
        state
            .property(&node, &PropertyKey::new("owner").unwrap())
            .is_err()
    );
}

#[test]
fn service_snapshot_exposes_objects_without_public_relations() {
    let mut state = test_state();
    let object = object_snapshot(
        "power",
        "/org/example/Power/devices/Battery0",
        BTreeMap::from([(
            "org.example.Power.Device".to_string(),
            BTreeMap::from([
                ("Percentage".to_string(), property(LocusValue::F64(82.5))),
                (
                    "NativePath".to_string(),
                    property(LocusValue::String("BAT0".to_string())),
                ),
            ]),
        )]),
    );
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object.clone())]),
        )
        .expect("snapshot update succeeds");

    let service = service_node("system").unwrap();
    let object_node = NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND).unwrap(),
        "power:/org/example/Power/devices/Battery0",
    )
    .unwrap();

    assert_eq!(
        state
            .property(&service, &PropertyKey::new("active-services").unwrap())
            .unwrap(),
        LocusValue::String("power".to_string())
    );
    assert!(state.relations(&service).unwrap().is_empty());
    assert!(
        state
            .targets(&service, &RelationName::new("object").unwrap())
            .is_err()
    );
    assert!(state.relations(&object_node).unwrap().is_empty());
    assert!(
        state
            .targets(&object_node, &RelationName::new("dbus").unwrap())
            .is_err()
    );
    assert_eq!(
        state
            .property(&object_node, &PropertyKey::new("Percentage").unwrap())
            .unwrap(),
        LocusValue::F64(82.5)
    );
    assert_eq!(
        state
            .property(&object_node, &PropertyKey::new("service-name").unwrap())
            .unwrap(),
        LocusValue::String("org.example.Power".to_string())
    );
    assert_eq!(
        state
            .property(&object_node, &PropertyKey::new("path").unwrap())
            .unwrap(),
        LocusValue::String("/org/example/Power/devices/Battery0".to_string())
    );
    assert_eq!(
        state
            .property(
                &object_node,
                &PropertyKey::new("org.example.Power.Device.Percentage").unwrap()
            )
            .unwrap(),
        LocusValue::F64(82.5)
    );
}

#[test]
fn object_node_ids_use_full_dbus_paths() {
    let mut state = test_state();
    let object = object_snapshot(
        "power",
        "/org/other/Device0",
        BTreeMap::from([(
            "org.example.Device".to_string(),
            BTreeMap::from([(
                "Name".to_string(),
                property(LocusValue::String("outside".to_string())),
            )]),
        )]),
    );
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object.clone())]),
        )
        .expect("snapshot update succeeds");

    let object_node = NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND).unwrap(),
        "power:/org/other/Device0",
    )
    .unwrap();

    assert_eq!(
        state
            .nodes(&NodeKind::new(DBUS_OBJECT_KIND).unwrap())
            .unwrap(),
        vec![object_node.clone()]
    );
    assert!(state.contains_node(&object_node).unwrap());
    assert_eq!(
        state
            .property(&object_node, &PropertyKey::new("path").unwrap())
            .unwrap(),
        LocusValue::String("/org/other/Device0".to_string())
    );
    assert_eq!(
        state
            .property(&object_node, &PropertyKey::new("Name").unwrap())
            .unwrap(),
        LocusValue::String("outside".to_string())
    );
}

#[test]
fn writable_object_properties_are_marked_and_update_cache() {
    let mut state = test_state();
    let object = object_snapshot(
        "power",
        "/org/example/Power",
        BTreeMap::from([(
            "org.example.Power".to_string(),
            BTreeMap::from([(
                "ActiveProfile".to_string(),
                writable_property(LocusValue::String("balanced".to_string())),
            )]),
        )]),
    );
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let object_node = NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND).unwrap(),
        "power:/org/example/Power",
    )
    .unwrap();
    let key = PropertyKey::new("ActiveProfile").unwrap();

    assert!(
        state
            .property_spec(&object_node, &key)
            .unwrap()
            .is_writable()
    );
    assert_eq!(
        state
            .writable_property(&object_node, &key)
            .unwrap()
            .interface,
        "org.example.Power"
    );

    state
        .update_cached_property(
            &object_node,
            &key,
            LocusValue::String("performance".to_string()),
        )
        .unwrap();
    assert_eq!(
        state.property(&object_node, &key).unwrap(),
        LocusValue::String("performance".to_string())
    );
}

#[test]
fn object_methods_are_exposed_as_write_only_call_nodes() {
    let mut state = test_state();
    let mut object = object_snapshot(
        "power",
        "/org/example/Power/devices/Keyboard0",
        BTreeMap::from([(
            "org.example.Power.Device".to_string(),
            BTreeMap::from([(
                "Name".to_string(),
                property(LocusValue::String("Keyboard".to_string())),
            )]),
        )]),
    );
    object.methods = BTreeMap::from([(
        "org.example.Power.Device".to_string(),
        BTreeMap::from([(
            "Connect".to_string(),
            DbusMethodSnapshot {
                input_signature: Vec::new(),
            },
        )]),
    )]);
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let object_node = NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND).unwrap(),
        "power:/org/example/Power/devices/Keyboard0",
    )
    .unwrap();
    let method_node = NodeId::new(
        NodeKind::new(DBUS_METHOD_KIND).unwrap(),
        "power:/org/example/Power/devices/Keyboard0:Connect",
    )
    .unwrap();
    let call_key = PropertyKey::new("call").unwrap();

    assert!(state.relations(&object_node).unwrap().is_empty());
    assert!(
        state
            .targets(&object_node, &RelationName::new("methods").unwrap())
            .is_err()
    );
    assert!(state.contains_node(&method_node).unwrap());
    assert_eq!(
        state
            .property(&method_node, &PropertyKey::new("method").unwrap())
            .unwrap(),
        LocusValue::String("Connect".to_string())
    );
    assert_eq!(
        state
            .property(&method_node, &PropertyKey::new("interface").unwrap())
            .unwrap(),
        LocusValue::String("org.example.Power.Device".to_string())
    );
    let spec = state.property_spec(&method_node, &call_key).unwrap();
    assert!(spec.is_writable());
    assert!(!spec.is_readable());
    assert_eq!(
        state
            .callable_method(&method_node, &call_key)
            .unwrap()
            .method,
        "Connect"
    );
}

#[test]
fn bus_path_exposes_object_tree_properties_and_method_call_files() {
    let mut state = test_state();
    let mut object = object_snapshot(
        "power",
        "/org/example/Power/devices/Keyboard0",
        BTreeMap::from([(
            "org.example.Power.Device".to_string(),
            BTreeMap::from([(
                "Name".to_string(),
                property(LocusValue::String("Keyboard".to_string())),
            )]),
        )]),
    );
    object.methods = BTreeMap::from([(
        "org.example.Power.Device".to_string(),
        BTreeMap::from([(
            "Connect".to_string(),
            DbusMethodSnapshot {
                input_signature: Vec::new(),
            },
        )]),
    )]);
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let service = service_node("system").unwrap();
    let root = GraphPathDirectory::Node(service);
    assert_eq!(child_names(&state, &root), vec!["org".to_string()]);

    let org_dir = lookup_dir(&state, &root, "org");
    let example_dir = lookup_dir(&state, &org_dir, "example");
    let power_dir = lookup_dir(&state, &example_dir, "Power");
    let devices_dir = lookup_dir(&state, &power_dir, "devices");
    let keyboard_dir = lookup_dir(&state, &devices_dir, "Keyboard0");
    let listed_children = child_names(&state, &keyboard_dir);
    assert!(listed_children.contains(&"Name".to_string()));
    assert!(listed_children.contains(&"org.example.Power.Device.Name".to_string()));
    assert!(listed_children.contains(&"Connect.call".to_string()));
    assert!(listed_children.contains(&"org.example.Power.Device.Connect.call".to_string()));

    assert!(matches!(
        state
            .path_lookup_child(&keyboard_dir, &"Name".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
    assert!(matches!(
        state
            .path_lookup_child(&keyboard_dir, &"Connect.call".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
}

#[test]
fn bus_path_preserves_full_object_paths_without_absolute_namespace() {
    let mut state = test_state();
    let object = object_snapshot(
        "power",
        "/org/other/Device0",
        BTreeMap::from([(
            "org.example.Device".to_string(),
            BTreeMap::from([(
                "Name".to_string(),
                property(LocusValue::String("outside".to_string())),
            )]),
        )]),
    );
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let service = service_node("system").unwrap();
    let root = GraphPathDirectory::Node(service);
    assert!(!child_names(&state, &root).contains(&"_absolute".to_string()));
    let org_dir = lookup_dir(&state, &root, "org");
    let other_dir = lookup_dir(&state, &org_dir, "other");
    let device_dir = lookup_dir(&state, &other_dir, "Device0");

    assert!(matches!(
        state
            .path_lookup_child(&device_dir, &"Name".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
}

#[test]
fn root_object_manager_paths_are_exposed_as_full_dbus_paths() {
    let mut state = DbusState::new(vec![ServiceConfig {
        local_id: "bluez".to_string(),
        bus: BusKind::System,
        name: "org.bluez".to_string(),
        object_manager_path: "/".to_string(),
    }]);
    let object = object_snapshot(
        "bluez",
        "/org/bluez/hci0",
        BTreeMap::from([(
            "org.bluez.Adapter1".to_string(),
            BTreeMap::from([("Powered".to_string(), property(LocusValue::Bool(true)))]),
        )]),
    );
    state
        .set_service_snapshot(
            "bluez",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let service = service_node("system").unwrap();
    let root = GraphPathDirectory::Node(service);
    let org_dir = lookup_dir(&state, &root, "org");
    let bluez_dir = lookup_dir(&state, &org_dir, "bluez");
    let adapter_dir = lookup_dir(&state, &bluez_dir, "hci0");

    assert!(matches!(
        state
            .path_lookup_child(&adapter_dir, &"Powered".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
}

#[test]
fn session_bus_paths_are_exposed_under_session() {
    let mut state = DbusState::new(vec![ServiceConfig {
        local_id: "agentdbus".to_string(),
        bus: BusKind::Session,
        name: "io.github.AgentDBus".to_string(),
        object_manager_path: "/io/github/AgentDBus".to_string(),
    }]);
    let object = object_snapshot(
        "agentdbus",
        "/io/github/AgentDBus/sessions/codex",
        BTreeMap::from([(
            "io.github.AgentDBus.Session".to_string(),
            BTreeMap::from([(
                "Status".to_string(),
                property(LocusValue::String("running".to_string())),
            )]),
        )]),
    );
    state
        .set_service_snapshot(
            "agentdbus",
            Some(":1.77".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let session = service_node("session").unwrap();
    assert_eq!(
        state
            .nodes(&NodeKind::new(DBUS_SERVICE_KIND).unwrap())
            .unwrap(),
        vec![session.clone()]
    );
    let root = GraphPathDirectory::Node(session);
    let io_dir = lookup_dir(&state, &root, "io");
    let github_dir = lookup_dir(&state, &io_dir, "github");
    let agent_dir = lookup_dir(&state, &github_dir, "AgentDBus");
    let sessions_dir = lookup_dir(&state, &agent_dir, "sessions");
    let codex_dir = lookup_dir(&state, &sessions_dir, "codex");

    assert!(matches!(
        state
            .path_lookup_child(&codex_dir, &"Status".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
    assert_eq!(
        state.path_watch_target(&codex_dir).unwrap(),
        Some(GraphWatchTarget::Kind(
            NodeKind::new(DBUS_OBJECT_KIND).unwrap()
        ))
    );
}

#[test]
fn ambiguous_methods_require_canonical_call_names() {
    let mut state = test_state();
    let mut object = object_snapshot(
        "power",
        "/org/example/Power/devices/Keyboard0",
        BTreeMap::new(),
    );
    object.methods = BTreeMap::from([
        (
            "org.example.Keyboard".to_string(),
            BTreeMap::from([(
                "Connect".to_string(),
                DbusMethodSnapshot {
                    input_signature: Vec::new(),
                },
            )]),
        ),
        (
            "org.example.Power.Device".to_string(),
            BTreeMap::from([(
                "Connect".to_string(),
                DbusMethodSnapshot {
                    input_signature: Vec::new(),
                },
            )]),
        ),
    ]);
    state
        .set_service_snapshot(
            "power",
            Some(":1.42".to_string()),
            BTreeMap::from([(object.path.clone(), object)]),
        )
        .expect("snapshot update succeeds");

    let root = GraphPathDirectory::Node(service_node("system").unwrap());
    let keyboard_dir = lookup_dir(
        &state,
        &lookup_dir(
            &state,
            &lookup_dir(
                &state,
                &lookup_dir(&state, &lookup_dir(&state, &root, "org"), "example"),
                "Power",
            ),
            "devices",
        ),
        "Keyboard0",
    );
    let listed_children = child_names(&state, &keyboard_dir);

    assert!(!listed_children.contains(&"Connect.call".to_string()));
    assert!(listed_children.contains(&"org.example.Keyboard.Connect.call".to_string()));
    assert!(listed_children.contains(&"org.example.Power.Device.Connect.call".to_string()));
}

#[test]
fn rejects_unconfigured_service_nodes() {
    let state = test_state();
    let node = service_node("upower").unwrap();

    assert!(!state.contains_node(&node).unwrap());
    assert!(
        state
            .property(&node, &PropertyKey::new("services").unwrap())
            .is_err()
    );
}

fn test_state() -> DbusState {
    DbusState::new(vec![ServiceConfig::system("org.example.Power")])
}

fn property(value: LocusValue) -> DbusPropertySnapshot {
    DbusPropertySnapshot {
        value,
        writable: false,
    }
}

fn lookup_dir(state: &DbusState, parent: &GraphPathDirectory, name: &str) -> GraphPathDirectory {
    match state
        .path_lookup_child(parent, &name.parse().unwrap())
        .unwrap()
    {
        Some(GraphPathEntry::Directory(directory)) => directory,
        other => panic!("expected directory for {name}, got {other:?}"),
    }
}

fn child_names(state: &DbusState, parent: &GraphPathDirectory) -> Vec<String> {
    state
        .path_children(parent)
        .unwrap()
        .unwrap_or_default()
        .into_iter()
        .map(|child| child.name.as_str().to_owned())
        .collect()
}

fn writable_property(value: LocusValue) -> DbusPropertySnapshot {
    DbusPropertySnapshot {
        value,
        writable: true,
    }
}
