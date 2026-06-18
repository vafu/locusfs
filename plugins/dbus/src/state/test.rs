use std::collections::BTreeMap;

use locusfs_graph::{LocusValue, NodeId, NodeKind, PropertyKey, RelationName};

use super::{
    DBUS_OBJECT_KIND, DBUS_SERVICE_KIND, DbusState, ServiceConfig, object_snapshot, service_node,
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
                ("Percentage".to_string(), LocusValue::F64(82.5)),
                (
                    "NativePath".to_string(),
                    LocusValue::String("BAT0".to_string()),
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
                LocusValue::String("outside".to_string()),
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
