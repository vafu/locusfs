use super::*;

#[test]
fn string_parsing_strips_one_echo_newline() {
    assert_eq!(
        LocusValue::parse_shell(ValueKind::String, "Display Name\n").unwrap(),
        LocusValue::String("Display Name".to_string())
    );
}

#[test]
fn string_parsing_preserves_inner_and_extra_newlines() {
    assert_eq!(
        LocusValue::parse_shell(ValueKind::String, "one\ntwo\n\n").unwrap(),
        LocusValue::String("one\ntwo\n".to_string())
    );
}

#[test]
fn scalar_values_parse_from_shell_text() {
    assert_eq!(
        LocusValue::parse_shell(ValueKind::Bool, "true\n").unwrap(),
        LocusValue::Bool(true)
    );
    assert_eq!(
        LocusValue::parse_shell(ValueKind::U32, "7\n").unwrap(),
        LocusValue::U32(7)
    );
    assert_eq!(
        LocusValue::parse_shell(ValueKind::I32, "-7\n").unwrap(),
        LocusValue::I32(-7)
    );
}

#[test]
fn floats_must_be_finite() {
    assert!(LocusValue::parse_shell(ValueKind::F64, "NaN").is_err());
}

#[test]
fn file_serialization_appends_newline() {
    assert_eq!(LocusValue::Bool(false).to_file_string(), "false\n");
}
