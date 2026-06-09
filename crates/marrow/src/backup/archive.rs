//! The on-disk backup framing: a magic header, a JSON manifest, and the
//! length-prefixed cell stream, plus the checksum over that stream. The framing
//! is deterministic, so equal data produces a byte-identical backup.

use std::io::{Read, Write};

use serde_json::{Value, json};

use super::{
    BackupCorruptProblem, BackupError, BackupFormatProblem, BackupManifest, CommitDescriptor,
    DefaultCountDescriptor, EngineDescriptor, FORMAT_VERSION, RetireCountDescriptor,
};
use marrow_store::tree::{
    EngineProfileDigest, TreeBackupCell, TreeBackupCellBuf, TreeBackupCellReadError,
};

/// Identifies a Marrow backup file. A file that does not begin with it is not a
/// backup this build can restore.
const MAGIC: &[u8; 8] = b"MARROWBK";

/// Upper bounds that keep a truncated or foreign file from forcing a huge
/// allocation before the framing is validated.
const MAX_MANIFEST_BYTES: u32 = 16 * 1024 * 1024;
const MAX_CELL_BYTES: u32 = 256 * 1024 * 1024;

const CHECKSUM_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold one cell into the running checksum over its framed bytes; write and read
/// sides must agree on exactly these bytes.
pub(super) fn checksum_cell(hash: u64, cell: TreeBackupCell<'_>) -> u64 {
    cell.fold_checksum(hash)
}

pub(super) const CHECKSUM_SEED: u64 = CHECKSUM_OFFSET;

pub(super) fn write_header(
    out: &mut impl Write,
    manifest: &BackupManifest,
) -> Result<(), BackupError> {
    let manifest = serde_json::to_vec(&manifest_to_json(manifest)).expect("a manifest serializes");
    out.write_all(MAGIC)?;
    out.write_all(&FORMAT_VERSION.to_be_bytes())?;
    out.write_all(&(manifest.len() as u32).to_be_bytes())?;
    out.write_all(&manifest)?;
    Ok(())
}

pub(super) fn write_cell(out: &mut impl Write, cell: TreeBackupCell<'_>) -> std::io::Result<()> {
    cell.write_framed(out)
}

pub(super) fn read_header(input: &mut impl Read) -> Result<BackupManifest, BackupError> {
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic).map_err(format_version_io)?;
    if &magic != MAGIC {
        return Err(BackupError::format_version(
            BackupFormatProblem::NotBackupFile,
            "not a Marrow backup file".to_string(),
        ));
    }
    let version = read_u32(input).map_err(format_version_io)?;
    if version != FORMAT_VERSION {
        return Err(BackupError::format_version(
            BackupFormatProblem::UnsupportedVersion {
                found: version,
                expected: FORMAT_VERSION,
            },
            format!(
                "backup format version {version} is unsupported (this build writes {FORMAT_VERSION})"
            ),
        ));
    }
    let manifest_len = read_u32(input).map_err(format_version_io)?;
    if manifest_len > MAX_MANIFEST_BYTES {
        return Err(BackupError::format_version(
            BackupFormatProblem::ManifestTooLarge,
            "backup manifest is implausibly large".to_string(),
        ));
    }
    let mut bytes = vec![0u8; manifest_len as usize];
    input.read_exact(&mut bytes).map_err(format_version_io)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        BackupError::format_version(
            BackupFormatProblem::ManifestInvalid,
            format!("backup manifest is not valid: {error}"),
        )
    })?;
    manifest_from_json(&value)
}

/// Read one framed cell, or a checksum/framing error if the stream is short.
pub(super) fn read_cell(input: &mut impl Read) -> Result<TreeBackupCellBuf, BackupError> {
    TreeBackupCellBuf::read_framed(input, MAX_CELL_BYTES).map_err(read_cell_error)
}

fn read_u32(input: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn format_version_io(error: std::io::Error) -> BackupError {
    BackupError::format_version(
        BackupFormatProblem::HeaderTruncated,
        format!("backup header is truncated: {error}"),
    )
}

fn read_cell_error(error: TreeBackupCellReadError) -> BackupError {
    match error {
        TreeBackupCellReadError::EndedEarly => BackupError::corrupt(
            BackupCorruptProblem::CellStreamEndedEarly,
            "backup cell stream ended early",
        ),
        TreeBackupCellReadError::CellTooLarge => BackupError::corrupt(
            BackupCorruptProblem::CellTooLarge,
            "backup cell is implausibly large",
        ),
        TreeBackupCellReadError::MalformedCell => BackupError::corrupt(
            BackupCorruptProblem::MalformedCell,
            "backup cell is malformed",
        ),
    }
}

fn manifest_to_json(manifest: &BackupManifest) -> Value {
    json!({
        "format_version": manifest.format_version,
        "source_digest": manifest.source_digest,
        "catalog_epoch": manifest.catalog_epoch,
        "engine": {
            "name": manifest.engine.name,
            "layout_epoch": manifest.engine.layout_epoch,
            "key_profile_version": manifest.engine.key_profile_version,
            "value_codec_version": manifest.engine.value_codec_version,
            "profile_digest": crate::hex_string(&manifest.engine.profile_digest),
        },
        "commit": manifest.commit.as_ref().map(commit_to_json),
        "record_count": manifest.record_count,
        "data_checksum": manifest.data_checksum,
    })
}

fn commit_to_json(commit: &CommitDescriptor) -> Value {
    json!({
        "commit_id": commit.commit_id,
        "catalog_epoch": commit.catalog_epoch,
        "layout_epoch": commit.layout_epoch,
        "source_digest": commit.source_digest,
        "engine_profile_digest": crate::hex_string(&commit.engine_profile_digest),
        "changed_root_catalog_ids": commit.changed_root_catalog_ids,
        "changed_index_catalog_ids": commit.changed_index_catalog_ids,
        "activation_evolution_digest": commit.activation_evolution_digest,
        "activation_proposal_catalog_digest": commit.activation_proposal_catalog_digest,
        "activation_proposal_new_catalog_ids": commit.activation_proposal_new_catalog_ids,
        "activation_records_backfilled": commit.activation_records_backfilled,
        "activation_default_records_by_id": commit.activation_default_records_by_id.iter().map(|count| json!({
            "catalog_id": &count.catalog_id,
            "records_backfilled": count.records_backfilled,
            "target_records": count.target_records,
            "evidence_digest": &count.evidence_digest,
        })).collect::<Vec<_>>(),
        "activation_indexes_rebuilt": commit.activation_indexes_rebuilt,
        "activation_records_retired": commit.activation_records_retired,
        "activation_retire_evidence_digest": commit.activation_retire_evidence_digest,
        "activation_records_retired_by_id": commit.activation_records_retired_by_id.iter().map(|count| json!({
            "catalog_id": &count.catalog_id,
            "records": count.records,
        })).collect::<Vec<_>>(),
        "activation_records_transformed": commit.activation_records_transformed,
    })
}

fn manifest_from_json(value: &Value) -> Result<BackupManifest, BackupError> {
    let engine = object_field(value, "engine")?;
    Ok(BackupManifest {
        format_version: u32_field(value, "format_version")?,
        source_digest: str_field(value, "source_digest")?.to_string(),
        catalog_epoch: opt_u64_field(value, "catalog_epoch")?,
        engine: EngineDescriptor {
            name: str_field(engine, "name")?.to_string(),
            layout_epoch: u64_field(engine, "layout_epoch")?,
            key_profile_version: u8_field(engine, "key_profile_version")?,
            value_codec_version: u32_field(engine, "value_codec_version")?,
            profile_digest: digest_field(engine, "profile_digest")?,
        },
        commit: match value.get("commit") {
            None | Some(Value::Null) => None,
            Some(commit @ Value::Object(_)) => Some(commit_from_json(commit)?),
            Some(_) => return Err(wrong_type("commit", "object")),
        },
        record_count: u64_field(value, "record_count")?,
        data_checksum: u64_field(value, "data_checksum")?,
    })
}

fn commit_from_json(value: &Value) -> Result<CommitDescriptor, BackupError> {
    let commit = CommitDescriptor {
        commit_id: u64_field(value, "commit_id")?,
        catalog_epoch: u64_field(value, "catalog_epoch")?,
        layout_epoch: u64_field(value, "layout_epoch")?,
        source_digest: str_field(value, "source_digest")?.to_string(),
        engine_profile_digest: digest_field(value, "engine_profile_digest")?,
        changed_root_catalog_ids: str_array_field(value, "changed_root_catalog_ids")?,
        changed_index_catalog_ids: str_array_field(value, "changed_index_catalog_ids")?,
        activation_evolution_digest: str_field(value, "activation_evolution_digest")?.to_string(),
        activation_proposal_catalog_digest: opt_str_field(
            value,
            "activation_proposal_catalog_digest",
        )?
        .filter(|digest| !digest.is_empty()),
        activation_proposal_new_catalog_ids: str_array_field(
            value,
            "activation_proposal_new_catalog_ids",
        )?,
        activation_records_backfilled: u64_field(value, "activation_records_backfilled")?,
        activation_default_records_by_id: default_counts_field(
            value,
            "activation_default_records_by_id",
        )?,
        activation_indexes_rebuilt: u64_field(value, "activation_indexes_rebuilt")?,
        activation_records_retired: u64_field(value, "activation_records_retired")?,
        activation_retire_evidence_digest: str_field(value, "activation_retire_evidence_digest")?
            .to_string(),
        activation_records_retired_by_id: retire_counts_field(
            value,
            "activation_records_retired_by_id",
        )?,
        activation_records_transformed: u64_field(value, "activation_records_transformed")?,
    };
    commit.validate_digest_shapes()?;
    Ok(commit)
}

fn missing(field: &'static str) -> impl Fn() -> BackupError {
    move || {
        BackupError::format_version(
            BackupFormatProblem::MissingField { field },
            format!("backup manifest is missing `{field}`"),
        )
    }
}

fn wrong_type(field: &'static str, expected: &'static str) -> BackupError {
    BackupError::format_version(
        BackupFormatProblem::FieldType { field, expected },
        format!("backup manifest field `{field}` must be {expected}"),
    )
}

fn object_field<'a>(value: &'a Value, field: &'static str) -> Result<&'a Value, BackupError> {
    match value.get(field) {
        None => Err(missing(field)()),
        Some(object @ Value::Object(_)) => Ok(object),
        Some(_) => Err(wrong_type(field, "object")),
    }
}

fn u32_field(value: &Value, field: &'static str) -> Result<u32, BackupError> {
    let Some(number) = number_field(value, field, "u32")? else {
        return Err(missing(field)());
    };
    u32::try_from(number).map_err(|_| {
        BackupError::format_version(
            BackupFormatProblem::FieldOutOfRange { field },
            format!("`{field}` is out of range"),
        )
    })
}

fn u8_field(value: &Value, field: &'static str) -> Result<u8, BackupError> {
    let Some(number) = number_field(value, field, "u8")? else {
        return Err(missing(field)());
    };
    u8::try_from(number).map_err(|_| {
        BackupError::format_version(
            BackupFormatProblem::FieldOutOfRange { field },
            format!("`{field}` is out of range"),
        )
    })
}

fn u64_field(value: &Value, field: &'static str) -> Result<u64, BackupError> {
    match number_field(value, field, "u64")? {
        Some(number) => Ok(number),
        None => Err(missing(field)()),
    }
}

fn number_field(
    value: &Value,
    field: &'static str,
    expected: &'static str,
) -> Result<Option<u64>, BackupError> {
    match value.get(field) {
        None => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .ok_or_else(|| wrong_type(field, expected))
            .map(Some),
        Some(_) => Err(wrong_type(field, expected)),
    }
}

fn opt_u64_field(value: &Value, field: &'static str) -> Result<Option<u64>, BackupError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .ok_or_else(|| wrong_type(field, "u64"))
            .map(Some),
        Some(_) => Err(wrong_type(field, "u64")),
    }
}

fn str_field<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, BackupError> {
    match value.get(field) {
        None => Err(missing(field)()),
        Some(Value::String(text)) => Ok(text),
        Some(_) => Err(wrong_type(field, "string")),
    }
}

fn opt_str_field(value: &Value, field: &'static str) -> Result<Option<String>, BackupError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(wrong_type(field, "string")),
    }
}

fn str_array_field(value: &Value, field: &'static str) -> Result<Vec<String>, BackupError> {
    let entries = match value.get(field) {
        None => return Err(missing(field)()),
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err(wrong_type(field, "array")),
    };
    entries
        .iter()
        .map(|entry| match entry {
            Value::String(text) => Ok(text.clone()),
            _ => Err(wrong_type(field, "string")),
        })
        .collect()
}

/// Decode an array-of-object manifest field, owning the shared array/object-shape
/// validation; each caller supplies only the per-entry field extraction.
fn object_array_field<T>(
    value: &Value,
    field: &'static str,
    parse_entry: impl Fn(&Value) -> Result<T, BackupError>,
) -> Result<Vec<T>, BackupError> {
    match value.get(field).ok_or_else(missing(field))? {
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                if !entry.is_object() {
                    return Err(wrong_type(field, "object"));
                }
                parse_entry(entry)
            })
            .collect(),
        _ => Err(wrong_type(field, "array")),
    }
}

fn default_counts_field(
    value: &Value,
    field: &'static str,
) -> Result<Vec<DefaultCountDescriptor>, BackupError> {
    object_array_field(value, field, |entry| {
        Ok(DefaultCountDescriptor {
            catalog_id: str_field(entry, "catalog_id")?.to_string(),
            records_backfilled: u64_field(entry, "records_backfilled")?,
            target_records: u64_field(entry, "target_records")?,
            evidence_digest: str_field(entry, "evidence_digest")?.to_string(),
        })
    })
}

fn retire_counts_field(
    value: &Value,
    field: &'static str,
) -> Result<Vec<RetireCountDescriptor>, BackupError> {
    object_array_field(value, field, |entry| {
        Ok(RetireCountDescriptor {
            catalog_id: str_field(entry, "catalog_id")?.to_string(),
            records: u64_field(entry, "records")?,
        })
    })
}

fn digest_field(value: &Value, field: &'static str) -> Result<EngineProfileDigest, BackupError> {
    let text = str_field(value, field)?;
    let mut digest = EngineProfileDigest::default();
    if text.len() != digest.len() * 2 {
        return Err(BackupError::format_version(
            BackupFormatProblem::DigestLength { field },
            format!("`{field}` is not an 8-byte digest"),
        ));
    }
    for (index, byte) in digest.iter_mut().enumerate() {
        let pair = &text[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(pair, 16).map_err(|_| {
            BackupError::format_version(
                BackupFormatProblem::DigestHex { field },
                format!("`{field}` is not hexadecimal"),
            )
        })?;
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{
        BackupError, BackupFormatProblem, BackupManifest, CommitDescriptor, DefaultCountDescriptor,
        EngineDescriptor, RetireCountDescriptor,
    };
    use super::{commit_from_json, commit_to_json, manifest_from_json, manifest_to_json};

    fn minimal_manifest() -> serde_json::Value {
        json!({
            "format_version": super::FORMAT_VERSION,
            "source_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000001",
            "catalog_epoch": null,
            "engine": {
                "name": super::super::ENGINE_NAME,
                "layout_epoch": 0,
                "key_profile_version": 0,
                "value_codec_version": marrow_store::value::VALUE_CODEC_VERSION,
                "profile_digest": "0102030405060708",
            },
            "commit": null,
            "record_count": 0,
            "data_checksum": 0,
        })
    }

    /// A fully populated manifest round-trips through `manifest_to_json` and
    /// `manifest_from_json` unchanged. The hand-written JSON mapping is a second owner
    /// of the manifest shape; this binds the two so a struct field added without a
    /// matching mapping (or vice versa) breaks the round-trip rather than silently
    /// dropping data from a backup.
    #[test]
    fn manifest_json_round_trips_every_field() {
        let digest = |seed: u8| format!("sha256:{}", format!("{seed:02x}").repeat(32));
        let manifest = BackupManifest {
            format_version: super::FORMAT_VERSION,
            source_digest: digest(1),
            catalog_epoch: Some(7),
            engine: EngineDescriptor {
                name: super::super::ENGINE_NAME.to_string(),
                layout_epoch: 3,
                key_profile_version: 2,
                value_codec_version: marrow_store::value::VALUE_CODEC_VERSION,
                profile_digest: [9, 8, 7, 6, 5, 4, 3, 2],
            },
            commit: Some(CommitDescriptor {
                commit_id: 11,
                catalog_epoch: 7,
                layout_epoch: 3,
                source_digest: digest(1),
                engine_profile_digest: [9, 8, 7, 6, 5, 4, 3, 2],
                changed_root_catalog_ids: vec!["cat_00000000000000000000000000000001".to_string()],
                changed_index_catalog_ids: vec!["cat_00000000000000000000000000000002".to_string()],
                activation_evolution_digest: digest(2),
                activation_proposal_catalog_digest: Some(digest(3)),
                activation_proposal_new_catalog_ids: vec![
                    "cat_00000000000000000000000000000003".to_string(),
                ],
                activation_records_backfilled: 5,
                activation_default_records_by_id: vec![DefaultCountDescriptor {
                    catalog_id: "cat_00000000000000000000000000000003".to_string(),
                    records_backfilled: 5,
                    target_records: 6,
                    evidence_digest: digest(4),
                }],
                activation_indexes_rebuilt: 1,
                activation_records_retired: 2,
                activation_retire_evidence_digest: digest(5),
                activation_records_retired_by_id: vec![RetireCountDescriptor {
                    catalog_id: "cat_00000000000000000000000000000004".to_string(),
                    records: 2,
                }],
                activation_records_transformed: 4,
            }),
            record_count: 42,
            data_checksum: 0xdead_beef,
        };

        let restored =
            manifest_from_json(&manifest_to_json(&manifest)).expect("manifest round-trips");

        assert_eq!(restored, manifest);
    }

    #[test]
    fn manifest_reports_present_wrong_type_fields() {
        let mut manifest = minimal_manifest();
        manifest["record_count"] = json!("1");

        let error = manifest_from_json(&manifest).expect_err("wrong type is rejected");

        assert_eq!(error.code(), "restore.format_version");
        assert!(matches!(
            error,
            BackupError::FormatVersion {
                problem: BackupFormatProblem::FieldType {
                    field: "record_count",
                    expected: "u64"
                },
                ..
            }
        ));
    }

    #[test]
    fn commit_manifest_requires_default_count_evidence_digest() {
        let commit = json!({
            "commit_id": 1,
            "catalog_epoch": 2,
            "layout_epoch": 0,
            "source_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000001",
            "engine_profile_digest": "0102030405060708",
            "changed_root_catalog_ids": [],
            "changed_index_catalog_ids": [],
            "activation_evolution_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000002",
            "activation_proposal_catalog_digest": null,
            "activation_proposal_new_catalog_ids": [],
            "activation_records_backfilled": 0,
            "activation_default_records_by_id": [{
                "catalog_id": "cat_00000000000000000000000000000001",
                "records_backfilled": 1,
                "target_records": 1
            }],
            "activation_indexes_rebuilt": 0,
            "activation_records_retired": 0,
            "activation_retire_evidence_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "activation_records_retired_by_id": [],
            "activation_records_transformed": 0,
        });

        let error = commit_from_json(&commit).expect_err("missing evidence is corrupt");

        assert_eq!(error.code(), "restore.format_version");
        assert!(matches!(
            error,
            BackupError::FormatVersion {
                problem: BackupFormatProblem::MissingField {
                    field: "evidence_digest"
                },
                ..
            }
        ));
    }

    #[test]
    fn commit_manifest_rejects_legacy_activation_digest_strings() {
        let legacy = |hex: &str| ["fn", "v1a64:", hex].concat();
        let mut commit = json!({
            "commit_id": 1,
            "catalog_epoch": 2,
            "layout_epoch": 0,
            "source_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000001",
            "engine_profile_digest": "0102030405060708",
            "changed_root_catalog_ids": [],
            "changed_index_catalog_ids": [],
            "activation_evolution_digest": legacy("0000000000000002"),
            "activation_proposal_catalog_digest": null,
            "activation_proposal_new_catalog_ids": [],
            "activation_records_backfilled": 0,
            "activation_default_records_by_id": [{
                "catalog_id": "cat_00000000000000000000000000000001",
                "records_backfilled": 1,
                "target_records": 1,
                "evidence_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000005"
            }],
            "activation_indexes_rebuilt": 0,
            "activation_records_retired": 0,
            "activation_retire_evidence_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "activation_records_retired_by_id": [],
            "activation_records_transformed": 0,
        });

        let error = commit_from_json(&commit).expect_err("legacy evolution digest is corrupt");
        assert_eq!(error.code(), "restore.format_version");
        assert!(matches!(
            error,
            BackupError::FormatVersion {
                problem: BackupFormatProblem::DigestSpelling {
                    field: "activation_evolution_digest"
                },
                ..
            }
        ));

        commit["activation_evolution_digest"] = json!("");
        commit["activation_default_records_by_id"][0]["evidence_digest"] =
            json!(legacy("0000000000000005"));
        let error = commit_from_json(&commit).expect_err("legacy default evidence is corrupt");
        assert_eq!(error.code(), "restore.format_version");
        assert!(matches!(
            error,
            BackupError::FormatVersion {
                problem: BackupFormatProblem::DigestSpelling {
                    field: "evidence_digest"
                },
                ..
            }
        ));
    }

    #[test]
    fn commit_manifest_serializes_activation_evidence_only() {
        let proposal_body_field = ["activation", "_proposal", "_catalog", "_json"].concat();
        let default_detail_field = ["activation", "_default", "_backfill", "_cells"].concat();
        let commit = CommitDescriptor {
            commit_id: 9,
            catalog_epoch: 7,
            layout_epoch: 1,
            source_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000001"
                    .to_string(),
            engine_profile_digest: [1, 2, 3, 4, 5, 6, 7, 8],
            changed_root_catalog_ids: vec!["cat_00000000000000000000000000000001".to_string()],
            changed_index_catalog_ids: Vec::new(),
            activation_evolution_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000002"
                    .to_string(),
            activation_proposal_catalog_digest: Some(
                "sha256:0000000000000000000000000000000000000000000000000000000000000003"
                    .to_string(),
            ),
            activation_proposal_new_catalog_ids: vec![
                "cat_00000000000000000000000000000007".to_string(),
            ],
            activation_records_backfilled: 128,
            activation_default_records_by_id: vec![DefaultCountDescriptor {
                catalog_id: "cat_00000000000000000000000000000004".to_string(),
                records_backfilled: 128,
                target_records: 128,
                evidence_digest:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000005"
                        .to_string(),
            }],
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000006"
                    .to_string(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        };

        let value = commit_to_json(&commit);
        let text = serde_json::to_string(&value).expect("serialize manifest commit");
        let counts = value["activation_default_records_by_id"]
            .as_array()
            .expect("default counts array");

        assert!(value.get(&proposal_body_field).is_none());
        assert!(value.get(&default_detail_field).is_none());
        assert!(!text.contains(&proposal_body_field));
        assert!(!text.contains(&default_detail_field));
        assert_eq!(counts.len(), 1);
        assert_eq!(
            counts[0]["evidence_digest"],
            json!("sha256:0000000000000000000000000000000000000000000000000000000000000005")
        );
        assert_eq!(
            commit_from_json(&value).expect("parse evidence-only commit"),
            commit
        );
    }
}
