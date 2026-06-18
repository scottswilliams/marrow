//! Outbound JSON rendering for Marrow's current CLI-compatible surfaces.
//!
//! This crate preserves the existing `marrow run --format json` return-value
//! shape and the saved-key shape used in tooling reports. It is not a general
//! `Value` codec, and inbound or web-lossless JSON needs checked context that
//! this outbound renderer deliberately does not carry.

use marrow_run::Value;
use marrow_store::key::SavedKey;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryReturnJsonError {
    UnsupportedValue,
}

pub fn entry_return_to_json(value: &Value) -> Result<serde_json::Value, EntryReturnJsonError> {
    Ok(match value {
        Value::Int(value) => json!({ "kind": "int", "value": value }),
        Value::Bool(value) => json!({ "kind": "bool", "value": value }),
        Value::Str(value) => json!({ "kind": "string", "value": value }),
        Value::Decimal(value) => json!({ "kind": "decimal", "value": value.to_text() }),
        Value::Date(value) => json!({ "kind": "date", "value": value }),
        Value::Duration(value) => json!({ "kind": "duration", "value": value.to_string() }),
        Value::Instant(value) => json!({ "kind": "instant", "value": value.to_string() }),
        Value::Bytes(value) => {
            json!({ "kind": "bytes", "value_b64": marrow_run::base64::encode(value) })
        }
        Value::Enum(value) => json!({
            "kind": "enum",
            "enum_id": value.enum_id().0,
            "member_id": value.member_id().0,
        }),
        Value::Identity(identity) => json!({
            "kind": "identity",
            "root": identity.root(),
            "keys": identity
                .keys()
                .iter()
                .map(saved_key_to_json)
                .collect::<Vec<_>>(),
        }),
        Value::Sequence(items) => {
            let values = items
                .iter()
                .map(entry_return_to_json)
                .collect::<Result<Vec<_>, _>>()?;
            json!({ "kind": "sequence", "values": values })
        }
        Value::Resource(_) | Value::LocalTree(_) => {
            return Err(EntryReturnJsonError::UnsupportedValue);
        }
    })
}

pub fn saved_key_to_json(key: &SavedKey) -> serde_json::Value {
    match key {
        SavedKey::Int(value) => json!({ "type": "int", "value": value }),
        SavedKey::Bool(value) => json!({ "type": "bool", "value": value }),
        SavedKey::Str(value) => json!({ "type": "string", "value": value }),
        SavedKey::Date(value) => json!({ "type": "date", "days_since_epoch": value }),
        SavedKey::Duration(value) => json!({ "type": "duration", "nanos": value.to_string() }),
        SavedKey::Instant(value) => {
            json!({ "type": "instant", "nanos_since_epoch": value.to_string() })
        }
        SavedKey::Bytes(value) => {
            json!({ "type": "bytes", "value_b64": marrow_run::base64::encode(value) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryReturnJsonError, entry_return_to_json, saved_key_to_json};
    use marrow_run::Value;
    use marrow_store::Decimal;
    use marrow_store::key::SavedKey;
    use serde_json::json;

    #[test]
    fn saved_keys_render_with_stable_type_tags() {
        let cases = vec![
            (
                SavedKey::Int(i64::MIN),
                json!({ "type": "int", "value": i64::MIN }),
            ),
            (
                SavedKey::Bool(true),
                json!({ "type": "bool", "value": true }),
            ),
            (
                SavedKey::Str("book-1".into()),
                json!({ "type": "string", "value": "book-1" }),
            ),
            (
                SavedKey::Date(-2),
                json!({ "type": "date", "days_since_epoch": -2 }),
            ),
            (
                SavedKey::Duration(1_000_000_000_000_000_001),
                json!({ "type": "duration", "nanos": "1000000000000000001" }),
            ),
            (
                SavedKey::Instant(-1_000_000_000_000_000_001),
                json!({ "type": "instant", "nanos_since_epoch": "-1000000000000000001" }),
            ),
            (
                SavedKey::Bytes(vec![0, 255]),
                json!({ "type": "bytes", "value_b64": "AP8=" }),
            ),
        ];

        for (key, expected) in cases {
            assert_eq!(saved_key_to_json(&key), expected);
        }
    }

    #[test]
    fn values_render_the_run_json_surface() {
        let value = Value::Sequence(vec![
            Value::Int(i64::MAX),
            Value::Bool(false),
            Value::Str("title".into()),
            Value::Decimal(Decimal::parse("12.5").expect("decimal")),
            Value::Date(-3),
            Value::Duration(1_000_000_000_000_000_002),
            Value::Instant(-1_000_000_000_000_000_002),
            Value::Bytes(vec![1, 2, 3]),
            Value::Sequence(vec![Value::Str("nested".into())]),
        ]);

        assert_eq!(
            entry_return_to_json(&value),
            Ok(json!({
                "kind": "sequence",
                "values": [
                    { "kind": "int", "value": i64::MAX },
                    { "kind": "bool", "value": false },
                    { "kind": "string", "value": "title" },
                    { "kind": "decimal", "value": "12.5" },
                    { "kind": "date", "value": -3 },
                    { "kind": "duration", "value": "1000000000000000002" },
                    { "kind": "instant", "value": "-1000000000000000002" },
                    { "kind": "bytes", "value_b64": "AQID" },
                    {
                        "kind": "sequence",
                        "values": [{ "kind": "string", "value": "nested" }]
                    }
                ]
            }))
        );
    }

    #[test]
    fn values_reject_runtime_shapes_without_a_json_surface() {
        assert_eq!(
            entry_return_to_json(&Value::Resource(Vec::new())),
            Err(EntryReturnJsonError::UnsupportedValue)
        );
        assert_eq!(
            entry_return_to_json(&Value::LocalTree(Vec::new())),
            Err(EntryReturnJsonError::UnsupportedValue)
        );
    }
}
