use locusfs_graph::{LocusValue, NodeId, NodeKind, PropertyKey};

use super::{DbusState, upower_node};
use crate::DBUS_SERVICE_KIND;

#[test]
fn exposes_hardcoded_upower_node() {
    let state = DbusState::default();
    let kind = NodeKind::new(DBUS_SERVICE_KIND).unwrap();

    assert_eq!(state.nodes(&kind).unwrap(), vec![upower_node().unwrap()]);
    assert!(state.contains_node(&upower_node().unwrap()).unwrap());
}

#[test]
fn inactive_upower_properties_omit_owner() {
    let state = DbusState::default();
    let node = upower_node().unwrap();

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
fn active_upower_properties_include_owner() {
    let mut state = DbusState::default();
    state
        .set_upower_owner(Some(":1.42".to_string()))
        .expect("owner update succeeds");
    let node = upower_node().unwrap();

    assert_eq!(
        state
            .property(&node, &PropertyKey::new("active").unwrap())
            .unwrap(),
        LocusValue::Bool(true)
    );
    assert_eq!(
        state
            .property(&node, &PropertyKey::new("owner").unwrap())
            .unwrap(),
        LocusValue::String(":1.42".to_string())
    );
}

#[test]
fn rejects_other_dbus_service_nodes() {
    let state = DbusState::default();
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
