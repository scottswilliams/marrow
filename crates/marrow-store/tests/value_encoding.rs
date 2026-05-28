//! Saved values round-trip through their canonical byte form, and non-canonical
//! bytes are rejected.

use marrow_store::value::{SavedValue, ValueType, decode_value, encode_value};

fn round_trips(value: SavedValue, ty: ValueType) {
    let bytes = encode_value(&value);
    assert_eq!(decode_value(&bytes, ty), Some(value), "round-trip failed");
}

#[test]
fn values_round_trip_through_canonical_bytes() {
    round_trips(SavedValue::Bool(true), ValueType::Bool);
    round_trips(SavedValue::Bool(false), ValueType::Bool);
    round_trips(SavedValue::Int(0), ValueType::Int);
    round_trips(SavedValue::Int(-42), ValueType::Int);
    round_trips(SavedValue::Int(i64::MIN), ValueType::Int);
    round_trips(SavedValue::Int(i64::MAX), ValueType::Int);
    round_trips(SavedValue::Str("Dune".into()), ValueType::Str);
    round_trips(SavedValue::Str(String::new()), ValueType::Str);
    round_trips(
        SavedValue::ErrorCode("store.limit_exceeded".into()),
        ValueType::ErrorCode,
    );
    round_trips(SavedValue::Bytes(vec![0x00, 0xFF, 0x01]), ValueType::Bytes);
}

#[test]
fn canonical_forms_match_the_docs() {
    assert_eq!(encode_value(&SavedValue::Bool(true)), b"1");
    assert_eq!(encode_value(&SavedValue::Bool(false)), b"0");
    assert_eq!(encode_value(&SavedValue::Int(42)), b"42");
    assert_eq!(encode_value(&SavedValue::Int(-5)), b"-5");
    assert_eq!(encode_value(&SavedValue::Str("hi".into())), b"hi");
}

#[test]
fn non_canonical_integers_are_rejected() {
    assert_eq!(decode_value(b"+1", ValueType::Int), None);
    assert_eq!(decode_value(b"01", ValueType::Int), None);
    assert_eq!(decode_value(b"-0", ValueType::Int), None);
    assert_eq!(decode_value(b" 1", ValueType::Int), None);
    assert_eq!(decode_value(b"x", ValueType::Int), None);
}

#[test]
fn non_canonical_booleans_are_rejected() {
    assert_eq!(decode_value(b"2", ValueType::Bool), None);
    assert_eq!(decode_value(b"true", ValueType::Bool), None);
    assert_eq!(decode_value(b"", ValueType::Bool), None);
}

#[test]
fn invalid_utf8_is_rejected_for_text_but_kept_for_bytes() {
    assert_eq!(decode_value(&[0xFF], ValueType::Str), None);
    assert_eq!(
        decode_value(&[0xFF], ValueType::Bytes),
        Some(SavedValue::Bytes(vec![0xFF]))
    );
}
