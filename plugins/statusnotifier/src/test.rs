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
