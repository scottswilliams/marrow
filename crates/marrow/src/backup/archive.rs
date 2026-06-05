//! The on-disk backup framing: a magic header, a JSON manifest, and the
//! length-prefixed cell stream, plus the checksum over that stream. The framing
//! is deterministic, so equal data produces a byte-identical backup.

use std::io::{Read, Write};

use serde_json::{Value, json};

use super::{
    BackupError, BackupManifest, CommitDescriptor, DefaultCountDescriptor, EngineDescriptor,
    FORMAT_VERSION, RetireCountDescriptor,
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

/// Fold one cell's framed bytes into the running checksum, exactly as they are
/// written, so the write and read sides agree.
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
        return Err(BackupError::FormatVersion(
            "not a Marrow backup file".to_string(),
        ));
    }
    let version = read_u32(input).map_err(format_version_io)?;
    if version != FORMAT_VERSION {
        return Err(BackupError::FormatVersion(format!(
            "backup format version {version} is unsupported (this build writes {FORMAT_VERSION})"
        )));
    }
    let manifest_len = read_u32(input).map_err(format_version_io)?;
    if manifest_len > MAX_MANIFEST_BYTES {
        return Err(BackupError::FormatVersion(
            "backup manifest is implausibly large".to_string(),
        ));
    }
    let mut bytes = vec![0u8; manifest_len as usize];
    input.read_exact(&mut bytes).map_err(format_version_io)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        BackupError::FormatVersion(format!("backup manifest is not valid: {error}"))
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
    BackupError::FormatVersion(format!("backup header is truncated: {error}"))
}

fn read_cell_error(error: TreeBackupCellReadError) -> BackupError {
    match error {
        TreeBackupCellReadError::EndedEarly => {
            BackupError::corrupt("backup cell stream ended early")
        }
        TreeBackupCellReadError::CellTooLarge => {
            BackupError::corrupt("backup cell is implausibly large")
        }
        TreeBackupCellReadError::MalformedCell => BackupError::corrupt("backup cell is malformed"),
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
            "profile_digest": hex(&manifest.engine.profile_digest),
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
        "engine_profile_digest": hex(&commit.engine_profile_digest),
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
    let engine = value.get("engine").ok_or_else(missing("engine"))?;
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
            Some(commit) => Some(commit_from_json(commit)?),
        },
        record_count: u64_field(value, "record_count")?,
        data_checksum: u64_field(value, "data_checksum")?,
    })
}

fn commit_from_json(value: &Value) -> Result<CommitDescriptor, BackupError> {
    Ok(CommitDescriptor {
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
    })
}

fn missing(field: &'static str) -> impl Fn() -> BackupError {
    move || BackupError::FormatVersion(format!("backup manifest is missing `{field}`"))
}

fn u32_field(value: &Value, field: &'static str) -> Result<u32, BackupError> {
    let number = u64_field(value, field)?;
    u32::try_from(number)
        .map_err(|_| BackupError::FormatVersion(format!("`{field}` is out of range")))
}

fn u8_field(value: &Value, field: &'static str) -> Result<u8, BackupError> {
    let number = u64_field(value, field)?;
    u8::try_from(number)
        .map_err(|_| BackupError::FormatVersion(format!("`{field}` is out of range")))
}

fn u64_field(value: &Value, field: &'static str) -> Result<u64, BackupError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(missing(field))
}

fn opt_u64_field(value: &Value, field: &'static str) -> Result<Option<u64>, BackupError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(_)) => Ok(Some(u64_field(value, field)?)),
        Some(_) => Err(missing(field)()),
    }
}

fn str_field<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, BackupError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(missing(field))
}

fn opt_str_field(value: &Value, field: &'static str) -> Result<Option<String>, BackupError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(missing(field)()),
    }
}

fn str_array_field(value: &Value, field: &'static str) -> Result<Vec<String>, BackupError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(missing(field))?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(missing(field))
        })
        .collect()
}

fn default_counts_field(
    value: &Value,
    field: &'static str,
) -> Result<Vec<DefaultCountDescriptor>, BackupError> {
    match value.get(field).ok_or_else(missing(field))? {
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                Ok(DefaultCountDescriptor {
                    catalog_id: str_field(entry, "catalog_id")?.to_string(),
                    records_backfilled: u64_field(entry, "records_backfilled")?,
                    target_records: u64_field(entry, "target_records")?,
                    evidence_digest: str_field(entry, "evidence_digest")?.to_string(),
                })
            })
            .collect(),
        _ => Err(missing(field)()),
    }
}

fn retire_counts_field(
    value: &Value,
    field: &'static str,
) -> Result<Vec<RetireCountDescriptor>, BackupError> {
    match value.get(field).ok_or_else(missing(field))? {
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                Ok(RetireCountDescriptor {
                    catalog_id: str_field(entry, "catalog_id")?.to_string(),
                    records: u64_field(entry, "records")?,
                })
            })
            .collect(),
        _ => Err(missing(field)()),
    }
}

fn digest_field(value: &Value, field: &'static str) -> Result<EngineProfileDigest, BackupError> {
    let text = str_field(value, field)?;
    let mut digest = EngineProfileDigest::default();
    if text.len() != digest.len() * 2 {
        return Err(BackupError::FormatVersion(format!(
            "`{field}` is not an 8-byte digest"
        )));
    }
    for (index, byte) in digest.iter_mut().enumerate() {
        let pair = &text[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(pair, 16)
            .map_err(|_| BackupError::FormatVersion(format!("`{field}` is not hexadecimal")))?;
    }
    Ok(digest)
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    crate::push_hex(&mut text, bytes);
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{CommitDescriptor, DefaultCountDescriptor};
    use super::{commit_from_json, commit_to_json};

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
        assert!(error.to_string().contains("evidence_digest"), "{error}");
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
