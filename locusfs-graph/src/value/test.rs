use super::*;
use crate::PropertyKey;

#[test]
fn values_report_their_kind() {
    assert_eq!(LocusValue::Bool(false).kind(), ValueKind::Bool);
    assert_eq!(LocusValue::U32(7).kind(), ValueKind::U32);
}

#[test]
fn property_specs_record_capabilities() {
    let key = PropertyKey::new("switch-to").unwrap();
    let spec = PropertySpec::write_only(key.clone(), ValueKind::String);
    assert_eq!(spec.key(), &key);
    assert_eq!(spec.kind(), ValueKind::String);
    assert!(!spec.is_readable());
    assert!(spec.is_writable());
}
