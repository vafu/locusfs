use crate::state::registered_item_name;

#[test]
fn registered_item_name_omits_default_path() {
    assert_eq!(
        registered_item_name(":1.42", "/StatusNotifierItem"),
        ":1.42"
    );
}

#[test]
fn registered_item_name_preserves_custom_path() {
    assert_eq!(
        registered_item_name(":1.62", "/org/ayatana/NotificationItem/nm_applet"),
        ":1.62/org/ayatana/NotificationItem/nm_applet"
    );
}

use locusfs_graph::{LocusValue, NodeId, NodeKind, PropertyKey};

use crate::{
    STATUS_NOTIFIER_ITEM_KIND,
    state::{StatusNotifierItem, StatusNotifierPixmap, StatusNotifierState},
};

#[test]
fn item_properties_include_pixmap_when_present() {
    let mut state = StatusNotifierState::default();
    state
        .upsert_item(StatusNotifierItem {
            id: "item".to_owned(),
            service_name: ":1.42".to_owned(),
            path: "/StatusNotifierItem".to_owned(),
            category: String::new(),
            title: String::new(),
            status: "Active".to_owned(),
            icon_name: String::new(),
            attention_icon_name: String::new(),
            overlay_icon_name: String::new(),
            menu_path: String::new(),
            item_is_menu: false,
            icon_pixmap: Some(StatusNotifierPixmap {
                width: 2,
                height: 1,
                argb32_hex: "00000000ffffffff".to_owned(),
            }),
        })
        .unwrap();
    let node = NodeId::new(NodeKind::new(STATUS_NOTIFIER_ITEM_KIND).unwrap(), "item").unwrap();

    assert_eq!(
        state
            .property(&node, &PropertyKey::new("icon-pixmap-width").unwrap())
            .unwrap(),
        LocusValue::U32(2)
    );
    assert_eq!(
        state
            .property(&node, &PropertyKey::new("icon-pixmap-height").unwrap())
            .unwrap(),
        LocusValue::U32(1)
    );
    assert_eq!(
        state
            .property(&node, &PropertyKey::new("icon-pixmap-argb32").unwrap())
            .unwrap(),
        LocusValue::String("00000000ffffffff".to_owned())
    );
}
