use crate::{GraphError, NodeId, NodeKind, PropertyKey};

#[test]
fn identifiers_reject_empty_values() {
    let error = NodeKind::new("").unwrap_err();
    assert!(matches!(error, GraphError::InvalidIdentifier { .. }));
}

#[test]
fn identifiers_reject_reserved_path_segments() {
    assert!(PropertyKey::new(".").is_err());
    assert!(PropertyKey::new("..").is_err());
}

#[test]
fn identifiers_allow_shell_hostile_but_encodable_text() {
    let id = NodeId::new(
        NodeKind::new("node").unwrap(),
        "57/active title".to_string(),
    )
    .unwrap();
    assert_eq!(id.kind().as_str(), "node");
    assert_eq!(id.local(), "57/active title");
}

#[test]
fn node_ids_parse_kind_and_local_id() {
    let id = NodeId::parse("workspace:1").unwrap();
    assert_eq!(id.kind().as_str(), "workspace");
    assert_eq!(id.local(), "1");
    assert_eq!(id.to_string(), "workspace:1");
}
