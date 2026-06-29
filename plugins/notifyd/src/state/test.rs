use locusfs_graph::{
    GraphPathDirectory, GraphPathEntry, LocusValue, NodeKind, PathName, PropertyKey,
};

use crate::state::{
    COMMANDS_NODE, NOTIFICATIONS_NODE, NotificationRecord, NotificationUrgency,
    NotifydCommandTarget, NotifydState, action_node, make_action, notification_node, notifyd_node,
};
use crate::{NOTIFICATION_ACTION_KIND, NOTIFICATION_KIND};

#[test]
fn notifications_path_lists_notifications() {
    let mut state = test_state();
    state
        .upsert_notification(notification("1", "Build done"), 16)
        .unwrap();

    let notifications = notifyd_node(NOTIFICATIONS_NODE).unwrap();
    let children = state
        .path_children(&GraphPathDirectory::Node(notifications))
        .unwrap()
        .unwrap();

    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name.as_str(), "1");
    assert_eq!(
        children[0].entry,
        GraphPathEntry::Directory(GraphPathDirectory::Node(notification_node("1").unwrap()))
    );
}

#[test]
fn notification_properties_match_record() {
    let mut state = test_state();
    state
        .upsert_notification(notification("7", "Hello"), 16)
        .unwrap();
    let node = notification_node("7").unwrap();

    assert_eq!(
        state
            .property(&node, &PropertyKey::new("summary").unwrap())
            .unwrap(),
        LocusValue::String("Hello".to_owned())
    );
    assert_eq!(
        state
            .property(&node, &PropertyKey::new("urgency").unwrap())
            .unwrap(),
        LocusValue::String("critical".to_owned())
    );
    assert!(
        state
            .property_spec(&node, &PropertyKey::new("discard").unwrap())
            .unwrap()
            .is_writable()
    );
}

#[test]
fn discard_removes_notification_from_notifications_path() {
    let mut state = test_state();
    state
        .upsert_notification(notification("10", "Discarded"), 16)
        .unwrap();

    state.discard_notification("10").unwrap();

    let notifications = notifyd_node(NOTIFICATIONS_NODE).unwrap();
    let children = state
        .path_children(&GraphPathDirectory::Node(notifications))
        .unwrap()
        .unwrap();
    assert!(children.is_empty());
}

#[test]
fn command_target_validates_discard_writes() {
    let mut state = test_state();
    state
        .upsert_notification(notification("11", "Discardable"), 16)
        .unwrap();

    assert_eq!(
        state
            .command_target(
                &notification_node("11").unwrap(),
                &PropertyKey::new("discard").unwrap(),
                &LocusValue::String("discarded".to_owned()),
            )
            .unwrap(),
        NotifydCommandTarget::Discard {
            notification_id: "11".to_owned()
        }
    );
}

#[test]
fn command_target_validates_discard_all_writes() {
    let state = test_state();
    let commands = notifyd_node(COMMANDS_NODE).unwrap();

    assert_eq!(
        state
            .command_target(
                &commands,
                &PropertyKey::new("discard-all").unwrap(),
                &LocusValue::String("discarded".to_owned()),
            )
            .unwrap(),
        NotifydCommandTarget::DiscardAll
    );
}

#[test]
fn notification_actions_are_exposed_under_actions_directory() {
    let mut state = test_state();
    let mut record = notification("3", "Actionable");
    record.actions = vec![make_action("3", "default".to_owned(), "Open".to_owned())];
    state.upsert_notification(record, 16).unwrap();
    let node = notification_node("3").unwrap();

    let actions = state
        .path_lookup_child(
            &GraphPathDirectory::Node(node.clone()),
            &PathName::new("actions").unwrap(),
        )
        .unwrap()
        .unwrap();
    let GraphPathEntry::Directory(actions_dir) = actions else {
        panic!("actions should be a directory");
    };
    let children = state.path_children(&actions_dir).unwrap().unwrap();

    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name.as_str(), "default");
    let action = action_node("3-default").unwrap();
    assert_eq!(
        state
            .property(&action, &PropertyKey::new("label").unwrap())
            .unwrap(),
        LocusValue::String("Open".to_owned())
    );
    assert!(
        state
            .property_spec(&action, &PropertyKey::new("invoke").unwrap())
            .unwrap()
            .is_writable()
    );
}

#[test]
fn command_target_validates_invoke_writes() {
    let mut state = test_state();
    let mut record = notification("4", "Actionable");
    record.actions = vec![make_action("4", "default".to_owned(), "Open".to_owned())];
    state.upsert_notification(record, 16).unwrap();

    assert_eq!(
        state
            .command_target(
                &action_node("4-default").unwrap(),
                &PropertyKey::new("invoke").unwrap(),
                &LocusValue::String("default".to_owned()),
            )
            .unwrap(),
        NotifydCommandTarget::InvokeAction {
            notification_id: "4".to_owned(),
            action_key: "default".to_owned(),
        }
    );
}

#[test]
fn command_dnd_is_read_write() {
    let state = test_state();
    let commands = notifyd_node(COMMANDS_NODE).unwrap();
    let key = PropertyKey::new("dnd-enabled").unwrap();

    let spec = state.property_spec(&commands, &key).unwrap();
    assert!(spec.is_readable());
    assert!(spec.is_writable());
    assert_eq!(
        state
            .command_target(&commands, &key, &LocusValue::Bool(true))
            .unwrap(),
        NotifydCommandTarget::SetDnd(true)
    );
}

#[test]
fn node_lists_include_notifications_and_actions() {
    let mut state = test_state();
    let mut record = notification("8", "Actionable");
    record.actions = vec![make_action("8", "default".to_owned(), "Open".to_owned())];
    state.upsert_notification(record, 16).unwrap();

    assert_eq!(
        state
            .nodes(&NodeKind::new(NOTIFICATION_KIND).unwrap())
            .unwrap(),
        vec![notification_node("8").unwrap()]
    );
    assert_eq!(
        state
            .nodes(&NodeKind::new(NOTIFICATION_ACTION_KIND).unwrap())
            .unwrap(),
        vec![action_node("8-default").unwrap()]
    );
}

fn test_state() -> NotifydState {
    NotifydState::new("Locus Notifyd".to_owned(), false)
}

fn notification(id: &str, summary: &str) -> NotificationRecord {
    NotificationRecord {
        local_id: id.to_owned(),
        dbus_id: id.parse().unwrap_or(1),
        created_at_unix_ms: 1000,
        updated_at_unix_ms: 1000,
        expire_timeout_ms: 5000,
        app_name: "Tests".to_owned(),
        desktop_entry: "tests".to_owned(),
        app_icon: "dialog-information".to_owned(),
        summary: summary.to_owned(),
        body: "Body".to_owned(),
        body_markup: None,
        category: "status".to_owned(),
        urgency: NotificationUrgency::Critical,
        progress: Some(40),
        resident: false,
        transient: false,
        suppress_sound: false,
        icon_name: "dialog-information".to_owned(),
        image_path: Some("/tmp/image.png".to_owned()),
        image_source: "image-path".to_owned(),
        image_width: Some(64),
        image_height: Some(64),
        stack_key: Some("tests".to_owned()),
        actions: Vec::new(),
    }
}
