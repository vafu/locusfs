use super::*;

#[test]
fn identifiers_reject_empty_values() {
    let error = NodeId::new("").unwrap_err();
    assert!(matches!(error, LocusFsError::InvalidIdentifier { .. }));
}

#[test]
fn identifiers_reject_reserved_path_segments() {
    assert!(PropertyKey::new(".").is_err());
    assert!(PropertyKey::new("..").is_err());
}

#[test]
fn identifiers_allow_shell_hostile_but_encodable_text() {
    let id = NodeId::new("window:57/active title").unwrap();
    assert_eq!(id.as_str(), "window:57/active title");
}
