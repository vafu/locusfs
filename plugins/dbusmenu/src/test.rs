use crate::runtime::registered_item_target;

#[test]
fn registered_item_target_uses_default_statusnotifier_path() {
    assert_eq!(
        registered_item_target(":1.42"),
        Some((":1.42".to_owned(), "/StatusNotifierItem".to_owned()))
    );
}

#[test]
fn registered_item_target_accepts_service_plus_custom_path() {
    assert_eq!(
        registered_item_target(":1.62/org/ayatana/NotificationItem/nm_applet"),
        Some((
            ":1.62".to_owned(),
            "/org/ayatana/NotificationItem/nm_applet".to_owned()
        ))
    );
}

#[test]
fn registered_item_target_rejects_unresolvable_path_only_item() {
    assert_eq!(registered_item_target("/org/example/Tray"), None);
}
