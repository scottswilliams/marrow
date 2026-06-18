//! Outbound JSON rendering for Marrow's current CLI-compatible surfaces.
//!
//! This crate preserves the existing `marrow run --format json` return-value
//! shape, saved-key shape, and data snapshot shape used in tooling reports. It
//! is not a general `Value` codec, and inbound or web-lossless JSON needs
//! checked context that this outbound renderer deliberately does not carry.

use marrow_check::tooling::{DataCommitStamp, DataSnapshotStamp};
use marrow_run::Value;
use marrow_store::key::SavedKey;
use serde::Serialize;
use serde_json::json;

const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryReturnJsonError {
    UnsupportedValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataSnapshotJson {
    pub store_uid: Option<String>,
    pub catalog_digest: Option<String>,
    pub commit: Option<DataCommitJson>,
    pub checked_source_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataCommitJson {
    pub commit_id: u64,
    pub catalog_epoch: u64,
    pub source_digest: String,
    pub layout_epoch: u64,
    pub engine_profile_digest: String,
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

pub fn data_snapshot_stamp_to_json(stamp: &DataSnapshotStamp) -> serde_json::Value {
    serde_json::to_value(DataSnapshotJson::from(stamp)).expect("data snapshot DTO serializes")
}

impl From<&DataSnapshotStamp> for DataSnapshotJson {
    fn from(stamp: &DataSnapshotStamp) -> Self {
        Self {
            store_uid: stamp.store_uid.as_ref().map(|uid| uid.as_str().to_string()),
            catalog_digest: stamp.store_catalog_digest.clone(),
            commit: stamp.store_commit.as_ref().map(DataCommitJson::from),
            checked_source_digest: stamp.checked_source_digest.clone(),
        }
    }
}

impl From<&DataCommitStamp> for DataCommitJson {
    fn from(stamp: &DataCommitStamp) -> Self {
        Self {
            commit_id: stamp.commit_id,
            catalog_epoch: stamp.catalog_epoch,
            source_digest: stamp.source_digest.clone(),
            layout_epoch: stamp.layout_epoch,
            engine_profile_digest: lower_hex(&stamp.engine_profile_digest),
        }
    }
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        DataSnapshotJson, EntryReturnJsonError, data_snapshot_stamp_to_json, entry_return_to_json,
        saved_key_to_json,
    };
    use marrow_check::tooling::{DataCommitStamp, DataSnapshotStamp};
    use marrow_run::Value;
    use marrow_store::Decimal;
    use marrow_store::key::SavedKey;
    use marrow_store::tree::StoreUid;
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
    fn data_snapshot_stamp_json_matches_cli_shape() {
        let stamp = DataSnapshotStamp {
            store_uid: Some(StoreUid::from_entropy_bytes([1; 16])),
            store_catalog_digest: Some("sha256:catalog".to_string()),
            store_commit: Some(DataCommitStamp {
                commit_id: 7,
                catalog_epoch: 3,
                source_digest: "sha256:source".to_string(),
                layout_epoch: 2,
                engine_profile_digest: [0x77, 0x94, 0x4e, 0xb8, 0x6c, 0x08, 0xb6, 0x65],
            }),
            checked_source_digest: "sha256:checked".to_string(),
        };

        assert_eq!(
            serde_json::to_value(DataSnapshotJson::from(&stamp)).unwrap(),
            json!({
                "store_uid": "store_01010101010101010101010101010101",
                "catalog_digest": "sha256:catalog",
                "commit": {
                    "commit_id": 7,
                    "catalog_epoch": 3,
                    "source_digest": "sha256:source",
                    "layout_epoch": 2,
                    "engine_profile_digest": "77944eb86c08b665",
                },
                "checked_source_digest": "sha256:checked",
            })
        );
    }

    #[test]
    fn data_snapshot_stamp_json_preserves_null_metadata_fields() {
        let stamp = DataSnapshotStamp {
            store_uid: None,
            store_catalog_digest: None,
            store_commit: None,
            checked_source_digest: "sha256:checked".to_string(),
        };

        assert_eq!(
            data_snapshot_stamp_to_json(&stamp),
            json!({
                "store_uid": null,
                "catalog_digest": null,
                "commit": null,
                "checked_source_digest": "sha256:checked",
            })
        );
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
