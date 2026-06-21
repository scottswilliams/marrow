//! JSON DTOs for Marrow's current machine-readable surfaces.
//!
//! This crate owns the bounded run result/error DTOs, run facts, saved-key
//! shape, and data generation shape used in tooling reports, plus checked surface
//! ABI descriptor DTOs, read request/result DTOs, generated write
//! request/result DTOs, surface action request/result DTOs, operation
//! envelopes, and descriptor-derived surface route manifests. Surface read DTOs
//! can execute against a
//! `marrow_run::ProjectSurfaceReadSession`, and point/singleton
//! create/update/delete and action DTOs can execute against a
//! `marrow_run::ProjectSurfaceSession`, without exposing the backing store. The
//! zero-capability project operation helper uses `Host::new()`; callers that
//! need host capabilities use the explicit-host helper. It is not a general
//! `Value` codec, and it does not define HTTP serving or opaque cursor tokens.

use marrow_check::tooling::{DataCommitStamp, DataSnapshotStamp, DataTransactionStamp};
use marrow_run::Value;
use marrow_store::key::SavedKey;
use serde::Serialize;
use serde_json::json;

pub mod resource_schema;
pub mod run;
pub mod saved_data;
pub mod surface;

pub use run::{
    EntryRunFactsJson, RunAutoAppliedJson, RunEnvelopeJson, RunStoreStateJson,
    entry_run_facts_to_json, run_error_to_json, run_output_to_json, run_session_error_to_json,
};

const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const ENTRY_RETURN_JSON_NODE_CAP: usize = 256;
const ENTRY_RETURN_JSON_STRING_CAP: usize = 8 * 1024;
const ENTRY_RETURN_JSON_BYTES_CAP: usize = (ENTRY_RETURN_JSON_STRING_CAP / 4) * 3;
const ENTRY_RETURN_JSON_KEY_COUNT_CAP: usize = 32;
pub const DATA_GENERATION_PROFILE_VERSION: &str = "data.generation.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryReturnJsonError {
    UnsupportedValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataGenerationJson {
    pub profile_version: String,
    pub store_uid: Option<String>,
    pub catalog_digest: Option<String>,
    pub commit: Option<DataCommitJson>,
    pub open_transaction: Option<DataTransactionJson>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataTransactionJson {
    pub depth: usize,
}

fn entry_return_to_json_bounded(
    value: &Value,
) -> Result<run::EntryReturnJson, EntryReturnJsonError> {
    let mut budget = EntryReturnBudget::new(ENTRY_RETURN_JSON_NODE_CAP);
    bounded_entry_return_to_json(value, &mut budget)
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

struct EntryReturnBudget {
    remaining: usize,
}

impl EntryReturnBudget {
    fn new(limit: usize) -> Self {
        Self { remaining: limit }
    }

    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }
}

fn bounded_entry_return_to_json(
    value: &Value,
    budget: &mut EntryReturnBudget,
) -> Result<run::EntryReturnJson, EntryReturnJsonError> {
    if !budget.take() {
        if matches!(value, Value::Resource(_) | Value::LocalTree(_)) {
            return Err(EntryReturnJsonError::UnsupportedValue);
        }
        return Ok(run::EntryReturnJson::Truncated);
    }
    Ok(match value {
        Value::Int(value) => run::EntryReturnJson::Int { value: *value },
        Value::Bool(value) => run::EntryReturnJson::Bool { value: *value },
        Value::Str(value) => {
            let value = bounded_entry_return_string(value);
            run::EntryReturnJson::String {
                value: value.value,
                truncated: value.truncated,
                original_bytes: value.original_bytes,
            }
        }
        Value::Decimal(value) => run::EntryReturnJson::Decimal {
            value: value.to_text(),
        },
        Value::Date(value) => run::EntryReturnJson::Date { value: *value },
        Value::Duration(value) => run::EntryReturnJson::Duration {
            value: value.to_string(),
        },
        Value::Instant(value) => run::EntryReturnJson::Instant {
            value: value.to_string(),
        },
        Value::Bytes(value) => {
            let value = bounded_entry_return_bytes(value);
            run::EntryReturnJson::Bytes {
                value_b64: value.value_b64,
                truncated: value.truncated,
                original_bytes: value.original_bytes,
            }
        }
        Value::Enum(value) => run::EntryReturnJson::Enum {
            member: value.render_name().to_string(),
        },
        Value::Identity(identity) => {
            let (keys, keys_truncated) = bounded_saved_keys_to_json(identity.keys());
            run::EntryReturnJson::Identity {
                root: identity.root().to_string(),
                keys,
                keys_truncated,
            }
        }
        Value::Sequence(items) => {
            let mut truncated = false;
            let mut values = Vec::new();
            for item in items.values() {
                if budget.remaining == 0 {
                    truncated = true;
                    break;
                }
                values.push(bounded_entry_return_to_json(item, budget)?);
            }
            run::EntryReturnJson::Sequence { values, truncated }
        }
        Value::Resource(_) | Value::LocalTree(_) => {
            return Err(EntryReturnJsonError::UnsupportedValue);
        }
    })
}

struct BoundedEntryReturnString {
    value: String,
    truncated: bool,
    original_bytes: usize,
}

fn bounded_entry_return_string(value: &str) -> BoundedEntryReturnString {
    let original_bytes = value.len();
    if original_bytes <= ENTRY_RETURN_JSON_STRING_CAP {
        return BoundedEntryReturnString {
            value: value.to_string(),
            truncated: false,
            original_bytes,
        };
    }
    let mut end = ENTRY_RETURN_JSON_STRING_CAP;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut value = value[..end].to_string();
    value.push('\u{2026}');
    BoundedEntryReturnString {
        value,
        truncated: true,
        original_bytes,
    }
}

struct BoundedEntryReturnBytes {
    value_b64: String,
    truncated: bool,
    original_bytes: usize,
}

fn bounded_entry_return_bytes(bytes: &[u8]) -> BoundedEntryReturnBytes {
    let original_bytes = bytes.len();
    let end = bytes.len().min(ENTRY_RETURN_JSON_BYTES_CAP);
    BoundedEntryReturnBytes {
        value_b64: marrow_run::base64::encode(&bytes[..end]),
        truncated: end < bytes.len(),
        original_bytes,
    }
}

fn bounded_saved_keys_to_json(keys: &[SavedKey]) -> (Vec<run::EntryReturnSavedKeyJson>, bool) {
    let truncated = keys.len() > ENTRY_RETURN_JSON_KEY_COUNT_CAP;
    let keys = keys
        .iter()
        .take(ENTRY_RETURN_JSON_KEY_COUNT_CAP)
        .map(bounded_saved_key_to_json)
        .collect();
    (keys, truncated)
}

fn bounded_saved_key_to_json(key: &SavedKey) -> run::EntryReturnSavedKeyJson {
    match key {
        SavedKey::Int(value) => run::EntryReturnSavedKeyJson::Int { value: *value },
        SavedKey::Bool(value) => run::EntryReturnSavedKeyJson::Bool { value: *value },
        SavedKey::Str(value) => {
            let value = bounded_entry_return_string(value);
            run::EntryReturnSavedKeyJson::String {
                value: value.value,
                truncated: value.truncated,
                original_bytes: value.original_bytes,
            }
        }
        SavedKey::Date(value) => run::EntryReturnSavedKeyJson::Date {
            days_since_epoch: *value,
        },
        SavedKey::Duration(value) => run::EntryReturnSavedKeyJson::Duration {
            nanos: value.to_string(),
        },
        SavedKey::Instant(value) => run::EntryReturnSavedKeyJson::Instant {
            nanos_since_epoch: value.to_string(),
        },
        SavedKey::Bytes(value) => {
            let value = bounded_entry_return_bytes(value);
            run::EntryReturnSavedKeyJson::Bytes {
                value_b64: value.value_b64,
                truncated: value.truncated,
                original_bytes: value.original_bytes,
            }
        }
    }
}

pub fn data_generation_stamp_to_json(stamp: &DataSnapshotStamp) -> serde_json::Value {
    serde_json::to_value(DataGenerationJson::from(stamp)).expect("data generation DTO serializes")
}

impl From<&DataSnapshotStamp> for DataGenerationJson {
    fn from(stamp: &DataSnapshotStamp) -> Self {
        Self {
            profile_version: DATA_GENERATION_PROFILE_VERSION.to_string(),
            store_uid: stamp.store_uid.as_ref().map(|uid| uid.as_str().to_string()),
            catalog_digest: stamp.store_catalog_digest.clone(),
            commit: stamp.store_commit.as_ref().map(DataCommitJson::from),
            open_transaction: stamp
                .open_transaction
                .as_ref()
                .map(DataTransactionJson::from),
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

impl From<&DataTransactionStamp> for DataTransactionJson {
    fn from(stamp: &DataTransactionStamp) -> Self {
        Self {
            depth: stamp.depth.get(),
        }
    }
}

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
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
        DATA_GENERATION_PROFILE_VERSION, DataGenerationJson, data_generation_stamp_to_json,
        entry_run_facts_to_json, run_error_to_json, run_output_to_json, run_session_error_to_json,
        saved_key_to_json,
    };
    use std::fs;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use marrow_check::tooling::{DataCommitStamp, DataSnapshotStamp, DataTransactionStamp};
    use marrow_run::{
        Host, ProjectInvokeError, ProjectOpen, ProjectSession, ProjectSessionError, RunOutput,
        RuntimeError, Sequence, SessionEntry, Value,
    };
    use marrow_store::Decimal;
    use marrow_store::key::SavedKey;
    use marrow_store::tree::StoreUid;
    use serde::Serialize;
    use serde_json::json;

    fn dto_json(value: impl Serialize) -> serde_json::Value {
        serde_json::to_value(value).expect("DTO serializes")
    }

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
    fn data_generation_json_matches_cli_shape() {
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
            open_transaction: None,
            checked_source_digest: "sha256:checked".to_string(),
        };

        assert_eq!(
            serde_json::to_value(DataGenerationJson::from(&stamp)).unwrap(),
            json!({
                "profile_version": DATA_GENERATION_PROFILE_VERSION,
                "store_uid": "store_01010101010101010101010101010101",
                "catalog_digest": "sha256:catalog",
                "commit": {
                    "commit_id": 7,
                    "catalog_epoch": 3,
                    "source_digest": "sha256:source",
                    "layout_epoch": 2,
                    "engine_profile_digest": "77944eb86c08b665",
                },
                "open_transaction": null,
                "checked_source_digest": "sha256:checked",
            })
        );
    }

    #[test]
    fn data_generation_json_preserves_null_metadata_fields() {
        let stamp = DataSnapshotStamp {
            store_uid: None,
            store_catalog_digest: None,
            store_commit: None,
            open_transaction: None,
            checked_source_digest: "sha256:checked".to_string(),
        };

        assert_eq!(
            data_generation_stamp_to_json(&stamp),
            json!({
                "profile_version": DATA_GENERATION_PROFILE_VERSION,
                "store_uid": null,
                "catalog_digest": null,
                "commit": null,
                "open_transaction": null,
                "checked_source_digest": "sha256:checked",
            })
        );
    }

    #[test]
    fn data_generation_json_renders_open_transaction() {
        let stamp = DataSnapshotStamp {
            store_uid: None,
            store_catalog_digest: None,
            store_commit: None,
            open_transaction: Some(DataTransactionStamp {
                depth: NonZeroUsize::new(2).expect("nonzero depth"),
            }),
            checked_source_digest: "sha256:checked".to_string(),
        };

        assert_eq!(
            data_generation_stamp_to_json(&stamp),
            json!({
                "profile_version": DATA_GENERATION_PROFILE_VERSION,
                "store_uid": null,
                "catalog_digest": null,
                "commit": null,
                "open_transaction": { "depth": 2 },
                "checked_source_digest": "sha256:checked",
            })
        );
    }

    #[test]
    fn run_output_json_renders_each_supported_result_value_kind() {
        let value = Value::Sequence(Sequence::dense(vec![
            Value::Int(i64::MAX),
            Value::Bool(false),
            Value::Str("title".into()),
            Value::Decimal(Decimal::parse("12.5").expect("decimal")),
            Value::Date(-3),
            Value::Duration(1_000_000_000_000_000_002),
            Value::Instant(-1_000_000_000_000_000_002),
            Value::Bytes(vec![1, 2, 3]),
            Value::Sequence(Sequence::dense(vec![Value::Str("nested".into())])),
        ]));

        let rendered = dto_json(
            run_output_to_json(&RunOutput { value: Some(value) }, String::new()).expect("json"),
        );

        assert_eq!(
            rendered["result"],
            json!({
                "kind": "value",
                "value": {
                "kind": "sequence",
                "values": [
                    { "kind": "int", "value": i64::MAX },
                    { "kind": "bool", "value": false },
                    {
                        "kind": "string",
                        "value": "title",
                        "truncated": false,
                        "originalBytes": 5
                    },
                    { "kind": "decimal", "value": "12.5" },
                    { "kind": "date", "value": -3 },
                    { "kind": "duration", "value": "1000000000000000002" },
                    { "kind": "instant", "value": "-1000000000000000002" },
                    {
                        "kind": "bytes",
                        "value_b64": "AQID",
                        "truncated": false,
                        "originalBytes": 3
                    },
                    {
                        "kind": "sequence",
                        "values": [{
                            "kind": "string",
                            "value": "nested",
                            "truncated": false,
                            "originalBytes": 6
                        }],
                        "truncated": false
                    }
                ],
                "truncated": false
                }
            })
        );
    }

    #[test]
    fn run_output_json_rejects_runtime_shapes_without_a_result_surface() {
        let resource_error = run_output_to_json(
            &RunOutput {
                value: Some(Value::Resource(Vec::new())),
            },
            String::new(),
        )
        .expect_err("resource values stay outside the run JSON result surface");
        let local_tree_error = run_output_to_json(
            &RunOutput {
                value: Some(Value::LocalTree(Vec::new())),
            },
            String::new(),
        )
        .expect_err("local trees stay outside the run JSON result surface");

        assert_eq!(resource_error.code(), marrow_run::RUN_ENTRY_SURFACE);
        assert_eq!(local_tree_error.code(), marrow_run::RUN_ENTRY_SURFACE);
    }

    #[test]
    fn run_output_json_renders_bounded_success_envelope() {
        let value = Value::Sequence(Sequence::dense((0..300).map(Value::Int).collect()));
        let rendered = dto_json(
            run_output_to_json(&RunOutput { value: Some(value) }, "ok".to_string()).expect("json"),
        );

        assert_eq!(rendered["diagnostics"], json!([]), "{rendered}");
        assert_eq!(rendered["output"], "ok", "{rendered}");
        assert_eq!(rendered["result"]["kind"], "value", "{rendered}");
        assert_eq!(
            rendered["result"]["value"]["kind"], "sequence",
            "{rendered}"
        );
        let values = rendered["result"]["value"]["values"]
            .as_array()
            .unwrap_or_else(|| panic!("large sequences render as a bounded DTO: {rendered}"));
        assert!(values.len() < 300, "{rendered}");
        assert_eq!(rendered["result"]["value"]["truncated"], true, "{rendered}");
    }

    #[test]
    fn run_output_json_caps_output_on_char_boundaries() {
        let rendered = dto_json(
            run_output_to_json(&RunOutput { value: None }, "\u{20ac}".repeat(8192)).expect("json"),
        );

        assert_eq!(rendered["result"], json!({ "kind": "none" }), "{rendered}");
        assert!(
            rendered["output"]
                .as_str()
                .is_some_and(|output| output.ends_with("\u{2026}output truncated\u{2026}")),
            "{rendered}"
        );
    }

    #[test]
    fn run_output_json_rejects_unsupported_return_values() {
        let error = run_output_to_json(
            &RunOutput {
                value: Some(Value::Resource(Vec::new())),
            },
            String::new(),
        )
        .expect_err("resource values stay outside the run JSON result surface");

        assert_eq!(error.code(), marrow_run::RUN_ENTRY_SURFACE);
    }

    #[test]
    fn run_output_json_truncates_without_scanning_omitted_values() {
        let mut values = (0..255).map(Value::Int).collect::<Vec<_>>();
        values.push(Value::Resource(Vec::new()));

        let rendered = dto_json(
            run_output_to_json(
                &RunOutput {
                    value: Some(Value::Sequence(Sequence::dense(values))),
                },
                String::new(),
            )
            .expect("omitted values are represented only by the truncation marker"),
        );

        assert_eq!(
            rendered["result"]["value"]["kind"], "sequence",
            "{rendered}"
        );
        assert_eq!(rendered["result"]["value"]["truncated"], true, "{rendered}");
        assert_eq!(
            rendered["result"]["value"]["values"]
                .as_array()
                .expect("rendered values")
                .len(),
            255,
            "{rendered}"
        );
    }

    #[test]
    fn run_error_json_renders_runtime_fault_diagnostics() {
        let root = temp_project(
            "run-error-json",
            "module m\npub fn main()\n    const ok = 1\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "m::main" } }"#,
        );
        let session = ProjectSession::open(&root, ProjectOpen::run().with_fresh_memory_store())
            .expect("fresh-memory session opens");
        let error = RuntimeError::entry_surface("x".repeat(20_000));
        let rendered = dto_json(run_error_to_json(
            session.runtime_program(),
            &ProjectInvokeError::Runtime(error),
            "before fault".to_string(),
        ));

        assert_eq!(rendered["output"], "before fault", "{rendered}");
        assert_eq!(
            rendered["diagnostics"][0]["code"],
            marrow_run::RUN_ENTRY_SURFACE
        );
        assert!(
            rendered["diagnostics"][0]["message"]
                .as_str()
                .is_some_and(|message| message.len() < 20_000),
            "{rendered}"
        );
        assert_eq!(rendered["diagnostics"][0]["source_span"]["line"], 0);
        assert_eq!(rendered["diagnostics"][0]["source_span"]["column"], 0);
        assert!(rendered["diagnostics"][0].get("character").is_none());
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn run_session_error_json_renders_run_diagnostic_envelope() {
        let rendered = dto_json(run_session_error_to_json(
            &ProjectSessionError::NoEntry,
            String::new(),
        ));

        assert_eq!(rendered["output"], "", "{rendered}");
        assert_eq!(rendered["diagnostics"][0]["code"], "run.no_entry");
        assert!(rendered.get("code").is_none(), "{rendered}");
    }

    #[test]
    fn run_missing_entry_json_uses_marrow_entry_contract() {
        let root = temp_project(
            "run-descriptor-error-json",
            "module m\npub fn main()\n    const ok = 1\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "m::main" } }"#,
        );
        let session = ProjectSession::open(&root, ProjectOpen::run().with_fresh_memory_store())
            .expect("fresh-memory session opens");
        let requested = "secret".repeat(5000);
        let mut output = String::new();
        let error = session
            .invoke(SessionEntry::new(&requested, &Host::new(), &mut output))
            .expect_err("unknown entry fails through the production invocation path");
        let rendered = dto_json(run_error_to_json(
            session.runtime_program(),
            &error,
            String::new(),
        ));

        assert_eq!(rendered["output"], "", "{rendered}");
        assert_eq!(
            rendered["diagnostics"][0]["code"],
            marrow_run::RUN_UNKNOWN_FUNCTION
        );
        assert!(
            rendered["diagnostics"][0]["message"]
                .as_str()
                .is_some_and(|message| message.len() < requested.len()),
            "{rendered}"
        );
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn run_output_json_bounds_identity_keys() {
        let huge_key = "x".repeat(20_000);
        let source = format!(
            "module m\n\
            resource Book\n    \
            required title: string\n\
            store ^books(id: string): Book\n\
            pub fn make(): Id(^books)\n    \
            return Id(^books, \"{huge_key}\")\n"
        );
        let root = temp_project(
            "run-identity-key-json",
            &source,
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        let session = ProjectSession::open(
            &root,
            ProjectOpen::run()
                .with_fresh_memory_store()
                .with_entry_override("m::make"),
        )
        .expect("fresh-memory session opens");
        let mut output = String::new();
        let result = session
            .invoke(SessionEntry::new("m::make", &Host::new(), &mut output))
            .expect("entry runs");

        let rendered = dto_json(run_output_to_json(&result, output).expect("json"));

        assert_eq!(rendered["diagnostics"], json!([]), "{rendered}");
        let value = &rendered["result"]["value"];
        let key = &value["keys"][0];
        assert_eq!(value["kind"], "identity", "{rendered}");
        assert_eq!(key["type"], "string", "{rendered}");
        assert_eq!(key["truncated"], true, "{rendered}");
        assert!(
            key["value"].as_str().unwrap().len() < huge_key.len(),
            "{rendered}"
        );
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn project_run_facts_json_projects_marrow_entry_facts() {
        let root = temp_project(
            "run-facts-json",
            "module m\n\
            resource Book\n    \
            required title: string\n\
            store ^books(id: int): Book\n\
            pub fn title(id: int): string\n    \
            return ^books(id).title ?? \"\"\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        let session = ProjectSession::open(
            &root,
            ProjectOpen::run()
                .with_fresh_memory_store()
                .with_entry_override("m::title"),
        )
        .expect("fresh-memory session opens");

        let rendered = run_facts_json_from_session(&session).expect("run facts render");

        assert!(
            rendered["analysis"]["sourceIdentity"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{rendered}"
        );
        assert_eq!(rendered["entry"]["canonicalName"], "m::title", "{rendered}");
        assert_eq!(rendered["storeOpenMode"], "read_only", "{rendered}");
        assert_eq!(
            rendered["footprint"]["workShape"], "read_only",
            "{rendered}"
        );
        assert_eq!(
            rendered["costShape"]["workShape"], "read_only",
            "{rendered}"
        );
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn project_run_facts_json_renders_analysis_generation() {
        let root = temp_project(
            "run-facts-generation-json",
            "module m\n\
            resource Book\n    \
            required title: string\n\
            store ^books(id: int): Book\n\
            pub fn title(id: int): string\n    \
            return ^books(id).title ?? \"\"\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        let session = ProjectSession::open(
            &root,
            ProjectOpen::run()
                .with_fresh_memory_store()
                .with_entry_override("m::title"),
        )
        .expect("fresh-memory session opens");

        let rendered = run_facts_json_from_session(&session).expect("run facts render");
        let generation = session.source_analysis_snapshot().generation();
        let accepted = generation
            .accepted_catalog
            .as_ref()
            .expect("fresh-memory run binds proposed catalog identity");

        assert_eq!(
            rendered["analysis"]["profileVersion"], "analysis.generation.v1",
            "{rendered}"
        );
        assert_eq!(
            rendered["analysis"]["sourceIdentity"],
            generation.content_identity.as_str(),
            "{rendered}"
        );
        assert_eq!(
            rendered["analysis"]["configDigest"],
            generation.config_digest.as_str(),
            "{rendered}"
        );
        assert_eq!(
            rendered["analysis"]["checkedSourceDigest"], generation.checked_source_digest,
            "{rendered}"
        );
        assert_eq!(
            rendered["analysis"]["readOnlyContextDigest"], generation.read_only_context_digest,
            "{rendered}"
        );
        assert_eq!(
            rendered["analysis"]["acceptedCatalog"],
            json!({
                "epoch": accepted.epoch,
                "digest": accepted.digest.as_deref().expect("accepted catalog digest"),
            }),
            "{rendered}"
        );
        assert_eq!(rendered["analysis"]["proposalCatalog"], json!(null));
        assert!(
            rendered["analysis"]
                .as_object()
                .is_some_and(|analysis| analysis.len() > 1),
            "{rendered}"
        );
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn project_run_facts_json_is_bound_to_run_sessions() {
        let root = temp_project(
            "run-facts-json-test-session",
            "module m\npub fn main()\n    const ok = 1\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        let session = ProjectSession::open(&root, ProjectOpen::test()).expect("test session opens");

        assert!(run_facts_json_from_session(&session).is_none());
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn project_run_facts_json_uses_the_session_entry_override() {
        let root = temp_project(
            "run-facts-json-entry-override",
            "module m\n\
            resource Book\n    \
            required title: string\n\
            store ^books(id: int): Book\n\
            pub fn read(): string\n    \
            return ^books(1).title ?? \"b\"\n\n\
            pub fn pure(): string\n    \
            return \"a\"\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        let session = ProjectSession::open(
            &root,
            ProjectOpen::run()
                .with_fresh_memory_store()
                .with_entry_override("m::read"),
        )
        .expect("fresh-memory session opens");

        let rendered = run_facts_json_from_session(&session).expect("run facts render");

        assert_eq!(rendered["entry"]["canonicalName"], "m::read", "{rendered}");
        assert_eq!(
            rendered["footprint"]["workShape"], "read_only",
            "{rendered}"
        );
        fs::remove_dir_all(&root).expect("remove temp project");
    }

    #[test]
    fn entry_run_facts_json_derives_source_identity_from_the_session() {
        let root_a = temp_project(
            "run-facts-json-source-a",
            "module m\npub fn main(): int\n    return 1\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "m::main" } }"#,
        );
        let root_b = temp_project(
            "run-facts-json-source-b",
            "module m\npub fn main(): int\n    return 2\n",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "m::main" } }"#,
        );
        let session_a = ProjectSession::open(&root_a, ProjectOpen::run().with_fresh_memory_store())
            .expect("first fresh-memory session opens");
        let session_b = ProjectSession::open(&root_b, ProjectOpen::run().with_fresh_memory_store())
            .expect("second fresh-memory session opens");

        let rendered_a =
            dto_json(entry_run_facts_to_json(&session_a).expect("first session facts render"));
        let rendered_b =
            dto_json(entry_run_facts_to_json(&session_b).expect("second session facts render"));
        assert_eq!(
            rendered_a["analysis"]["sourceIdentity"],
            session_a.source_analysis_identity().as_str(),
            "{rendered_a}"
        );
        assert_eq!(
            rendered_b["analysis"]["sourceIdentity"],
            session_b.source_analysis_identity().as_str(),
            "{rendered_b}"
        );
        assert_ne!(
            rendered_a["analysis"]["sourceIdentity"],
            rendered_b["analysis"]["sourceIdentity"]
        );
        assert_eq!(rendered_a["entry"], rendered_b["entry"]);
        fs::remove_dir_all(&root_a).expect("remove first temp project");
        fs::remove_dir_all(&root_b).expect("remove second temp project");
    }

    #[test]
    fn entry_run_facts_json_does_not_accept_external_identity_inputs() {
        let source = include_str!("run.rs");
        let start = source
            .find("pub fn entry_run_facts_to_json")
            .expect("public run-fact projector exists");
        let end = source[start..]
            .find(") -> Option<EntryRunFactsJson>")
            .expect("public run-fact projector signature is intact")
            + start;
        let signature = &source[start..end];
        assert!(signature.contains("session: &ProjectSession"));
        assert!(!signature.contains("source_identity"));
        assert!(!signature.contains("identity"));
    }

    fn run_facts_json_from_session(session: &ProjectSession) -> Option<serde_json::Value> {
        entry_run_facts_to_json(session).map(dto_json)
    }

    fn temp_project(name: &str, source: &str, config: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-json-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(root.join("src")).expect("create source root");
        fs::write(root.join("src/m.mw"), source).expect("write source");
        fs::write(root.join("marrow.json"), config).expect("write config");
        root
    }
}
