use super::{WatchAction, WatchChange, WatchEvent, WatchState, WatchValue};

#[test]
fn encodes_and_decodes_unset_state() {
    let event = WatchEvent::State(WatchState::Unset);

    let encoded = event.encode_text();
    let decoded = WatchEvent::decode_text(&encoded).unwrap();

    assert_eq!(encoded, b"unset\n");
    assert_eq!(decoded, event);
}

#[test]
fn decodes_absolute_set_payload_as_path() {
    let decoded = WatchEvent::decode_text(b"set /workspace/3\n").unwrap();

    assert_eq!(
        decoded,
        WatchEvent::State(WatchState::Set(WatchValue::Path(
            "/workspace/3".to_string()
        )))
    );
}

#[test]
fn decodes_non_absolute_set_payload_as_property() {
    let decoded = WatchEvent::decode_text(b"set true\n").unwrap();

    assert_eq!(
        decoded,
        WatchEvent::State(WatchState::Set(WatchValue::Property("true".to_string())))
    );
}

#[test]
fn decodes_empty_set_payload_as_property() {
    let decoded = WatchEvent::decode_text(b"set \n").unwrap();

    assert_eq!(
        decoded,
        WatchEvent::State(WatchState::Set(WatchValue::Property(String::new())))
    );
}

#[test]
fn encodes_and_decodes_subject_relative_property_change() {
    let event = WatchEvent::Change(WatchChange::Property {
        action: WatchAction::Changed,
        node: None,
        key: "selected".to_string(),
    });

    let encoded = event.encode_text();
    let decoded = WatchEvent::decode_text(&encoded).unwrap();

    assert_eq!(encoded, b"property changed selected\n");
    assert_eq!(decoded, event);
}

#[test]
fn encodes_and_decodes_absolute_relation_change() {
    let event = WatchEvent::Change(WatchChange::Relation {
        action: WatchAction::Removed,
        node: Some("context:selected".to_string()),
        relation: "workspace".to_string(),
    });

    let encoded = event.encode_text();
    let decoded = WatchEvent::decode_text(&encoded).unwrap();

    assert_eq!(encoded, b"relation removed context:selected workspace\n");
    assert_eq!(decoded, event);
}
