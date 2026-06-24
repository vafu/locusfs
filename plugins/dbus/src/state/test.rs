use std::collections::BTreeMap;

use locusfs_graph::{
    GraphPathDirectory, GraphPathEntry, LocusValue, NodeId, NodeKind, PropertyKey, RelationName,
};

use super::{
    BusKind, DBUS_METHOD_KIND, DBUS_OBJECT_KIND, DBUS_SERVICE_KIND, DbusMethodSnapshot,
    DbusPropertySnapshot, DbusState, ServiceConfig, object_snapshot, service_node,
};

#[test]
fn exposes_configured_service_node() {
    let state = test_state();
    let kind = NodeKind::new(DBUS_SERVICE_KIND).unwrap();
    let service = service_node("power").unwrap();

    assert_eq!(state.nodes(&kind).unwrap(), vec![service.clone()]);
    assert!(state.contains_node(&service).unwrap());
}

#[test]
fn inactive_service_properties_omit_owner() {
    let state = test_state();
    let node = service_node("power").unwrap();

    assert_eq!(
        state
            .property(&node, &PropertyKey::new("active").unwrap())
            .unwrap(),
        LocusValue::Bool(false)
    );
    assert!(
        state
            .property(&node, &PropertyKey::new("owner").unwrap())
            .is_err()
    );
}

#[test]
fn service_snapshot_exposes_objects_and_relations() {
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

    let service = service_node("power").unwrap();
    let object_node = NodeId::new(
        NodeKind::new(DBUS_OBJECT_KIND).unwrap(),
        "power:devices/Battery0",
    )
    .unwrap();

    assert_eq!(
        state
            .property(&service, &PropertyKey::new("active").unwrap())
            .unwrap(),
        LocusValue::Bool(true)
    );
    assert_eq!(
        state
            .targets(&service, &RelationName::new("object").unwrap())
            .unwrap(),
        vec![object_node.clone()]
    );
    assert_eq!(
        state
            .targets(&object_node, &RelationName::new("dbus-service").unwrap())
            .unwrap(),
        vec![service]
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
fn object_node_ids_round_trip_for_paths_outside_object_manager() {
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

    let object_node = NodeId::new(NodeKind::new(DBUS_OBJECT_KIND).unwrap(), "power:@").unwrap();
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
        "power:devices/Keyboard0",
    )
    .unwrap();
    let method_node = NodeId::new(
        NodeKind::new(DBUS_METHOD_KIND).unwrap(),
        "power:devices/Keyboard0:Connect",
    )
    .unwrap();
    let call_key = PropertyKey::new("call").unwrap();

    assert_eq!(
        state
            .targets(&object_node, &RelationName::new("methods").unwrap())
            .unwrap(),
        vec![method_node.clone()]
    );
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
fn service_path_exposes_object_tree_properties_and_methods() {
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

    let service = service_node("power").unwrap();
    let root = GraphPathDirectory::Node(service);
    let object_dir = lookup_dir(&state, &root, "object");
    let devices_dir = lookup_dir(&state, &object_dir, "devices");
    let keyboard_dir = lookup_dir(&state, &devices_dir, "Keyboard0");
    let properties_dir = lookup_dir(&state, &keyboard_dir, "@properties");
    let methods_dir = lookup_dir(&state, &keyboard_dir, "@methods");

    assert!(matches!(
        state
            .path_lookup_child(&properties_dir, &"Name".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
    let method_dir = lookup_dir(&state, &methods_dir, "Connect");
    assert!(matches!(
        state
            .path_lookup_child(&method_dir, &"call".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
}

#[test]
fn outside_object_manager_paths_are_namespaced_under_absolute() {
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

    let service = service_node("power").unwrap();
    let root = GraphPathDirectory::Node(service);
    let object_dir = lookup_dir(&state, &root, "object");
    let absolute_dir = lookup_dir(&state, &object_dir, "@absolute");
    let org_dir = lookup_dir(&state, &absolute_dir, "org");
    let other_dir = lookup_dir(&state, &org_dir, "other");
    let device_dir = lookup_dir(&state, &other_dir, "Device0");
    let properties_dir = lookup_dir(&state, &device_dir, "@properties");

    assert!(matches!(
        state
            .path_lookup_child(&properties_dir, &"Name".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
}

#[test]
fn root_object_manager_paths_are_exposed_relative_to_object_root() {
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

    let service = service_node("bluez").unwrap();
    let root = GraphPathDirectory::Node(service);
    let object_dir = lookup_dir(&state, &root, "object");
    let org_dir = lookup_dir(&state, &object_dir, "org");
    let bluez_dir = lookup_dir(&state, &org_dir, "bluez");
    let adapter_dir = lookup_dir(&state, &bluez_dir, "hci0");
    let properties_dir = lookup_dir(&state, &adapter_dir, "@properties");

    assert!(matches!(
        state
            .path_lookup_child(&properties_dir, &"Powered".parse().unwrap())
            .unwrap(),
        Some(GraphPathEntry::Property { .. })
    ));
}

#[test]
fn rejects_unconfigured_service_nodes() {
    let state = test_state();
    let node = NodeId::new(
        NodeKind::new(DBUS_SERVICE_KIND).unwrap(),
        "org.freedesktop.Notifications",
    )
    .unwrap();

    assert!(!state.contains_node(&node).unwrap());
    assert!(
        state
            .property(&node, &PropertyKey::new("active").unwrap())
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

fn writable_property(value: LocusValue) -> DbusPropertySnapshot {
    DbusPropertySnapshot {
        value,
        writable: true,
    }
}
