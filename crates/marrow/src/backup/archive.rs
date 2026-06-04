//! The on-disk backup framing: a magic header, a JSON manifest, and the
//! length-prefixed cell stream, plus the checksum over that stream. The framing
//! is deterministic, so equal data produces a byte-identical backup.

use std::io::{Read, Write};

use serde_json::{Value, json};

use super::{
    BackupError, BackupManifest, CommitDescriptor, DefaultBackfillCellDescriptor,
    DefaultCountDescriptor, EngineDescriptor, FORMAT_VERSION, KeyDescriptor, KeyKind,
    PathSegmentDescriptor, PathSegmentKind, RetireCountDescriptor,
};
use marrow_store::tree::EngineProfileDigest;

/// Identifies a Marrow backup file. A file that does not begin with it is not a
/// backup this build can restore.
const MAGIC: &[u8; 8] = b"MARROWBK";

/// Upper bounds that keep a truncated or foreign file from forcing a huge
/// allocation before the framing is validated.
const MAX_MANIFEST_BYTES: u32 = 16 * 1024 * 1024;
const MAX_CELL_BYTES: u32 = 256 * 1024 * 1024;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Fold one cell's framed bytes into the running checksum, exactly as they are
/// written, so the write and read sides agree.
pub(super) fn checksum_cell(hash: u64, key: &[u8], value: &[u8]) -> u64 {
    let mut hash = fold(hash, &(key.len() as u32).to_be_bytes());
    hash = fold(hash, key);
    hash = fold(hash, &(value.len() as u32).to_be_bytes());
    fold(hash, value)
}

pub(super) const CHECKSUM_SEED: u64 = FNV_OFFSET;

fn fold(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

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

pub(super) fn write_cell(out: &mut impl Write, key: &[u8], value: &[u8]) -> std::io::Result<()> {
    out.write_all(&(key.len() as u32).to_be_bytes())?;
    out.write_all(key)?;
    out.write_all(&(value.len() as u32).to_be_bytes())?;
    out.write_all(value)
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
pub(super) fn read_cell(input: &mut impl Read) -> Result<(Vec<u8>, Vec<u8>), BackupError> {
    let key = read_chunk(input)?;
    let value = read_chunk(input)?;
    Ok((key, value))
}

fn read_chunk(input: &mut impl Read) -> Result<Vec<u8>, BackupError> {
    let len =
        read_u32(input).map_err(|_| BackupError::corrupt("backup cell stream ended early"))?;
    if len > MAX_CELL_BYTES {
        return Err(BackupError::corrupt("backup cell is implausibly large"));
    }
    let mut bytes = vec![0u8; len as usize];
    input
        .read_exact(&mut bytes)
        .map_err(|_| BackupError::corrupt("backup cell stream ended early"))?;
    Ok(bytes)
}

fn read_u32(input: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn format_version_io(error: std::io::Error) -> BackupError {
    BackupError::FormatVersion(format!("backup header is truncated: {error}"))
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
        "activation_proposal_catalog_json": commit.activation_proposal_catalog_json,
        "activation_records_backfilled": commit.activation_records_backfilled,
        "activation_default_records_by_id": commit.activation_default_records_by_id.iter().map(|count| json!({
            "catalog_id": &count.catalog_id,
            "records_backfilled": count.records_backfilled,
            "target_records": count.target_records,
        })).collect::<Vec<_>>(),
        "activation_default_backfill_cells": commit.activation_default_backfill_cells.iter().map(|cell| json!({
            "catalog_id": &cell.catalog_id,
            "store_id": &cell.store_id,
            "identity": cell.identity.iter().map(key_to_json).collect::<Vec<_>>(),
            "path": cell.path.iter().map(path_segment_to_json).collect::<Vec<_>>(),
            "backfilled": cell.backfilled,
        })).collect::<Vec<_>>(),
        "activation_indexes_rebuilt": commit.activation_indexes_rebuilt,
        "activation_records_retired": commit.activation_records_retired,
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
        activation_proposal_catalog_json: opt_str_field(value, "activation_proposal_catalog_json")?
            .filter(|catalog| !catalog.is_empty()),
        activation_records_backfilled: u64_field(value, "activation_records_backfilled")?,
        activation_default_records_by_id: default_counts_field(
            value,
            "activation_default_records_by_id",
        )?,
        activation_default_backfill_cells: default_backfill_cells_field(
            value,
            "activation_default_backfill_cells",
        )?,
        activation_indexes_rebuilt: u64_field(value, "activation_indexes_rebuilt")?,
        activation_records_retired: u64_field(value, "activation_records_retired")?,
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

fn bool_field(value: &Value, field: &'static str) -> Result<bool, BackupError> {
    value
        .get(field)
        .and_then(Value::as_bool)
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
                })
            })
            .collect(),
        _ => Err(missing(field)()),
    }
}

fn default_backfill_cells_field(
    value: &Value,
    field: &'static str,
) -> Result<Vec<DefaultBackfillCellDescriptor>, BackupError> {
    match value.get(field).ok_or_else(missing(field))? {
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                Ok(DefaultBackfillCellDescriptor {
                    catalog_id: str_field(entry, "catalog_id")?.to_string(),
                    store_id: str_field(entry, "store_id")?.to_string(),
                    identity: key_array_field(entry, "identity")?,
                    path: path_array_field(entry, "path")?,
                    backfilled: bool_field(entry, "backfilled")?,
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

fn key_array_field(value: &Value, field: &'static str) -> Result<Vec<KeyDescriptor>, BackupError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(missing(field))?
        .iter()
        .map(key_from_json)
        .collect()
}

fn path_array_field(
    value: &Value,
    field: &'static str,
) -> Result<Vec<PathSegmentDescriptor>, BackupError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(missing(field))?
        .iter()
        .map(path_segment_from_json)
        .collect()
}

fn key_to_json(key: &KeyDescriptor) -> Value {
    json!({
        "kind": key_kind_text(key.kind),
        "value": &key.value,
    })
}

fn key_from_json(value: &Value) -> Result<KeyDescriptor, BackupError> {
    Ok(KeyDescriptor {
        kind: key_kind(str_field(value, "kind")?)?,
        value: str_field(value, "value")?.to_string(),
    })
}

fn path_segment_to_json(segment: &PathSegmentDescriptor) -> Value {
    match segment.kind {
        PathSegmentKind::Member => json!({
            "kind": "member",
            "catalog_id": &segment.catalog_id,
        }),
        PathSegmentKind::Key => json!({
            "kind": "key",
            "key": segment.key.as_ref().map(key_to_json),
        }),
    }
}

fn path_segment_from_json(value: &Value) -> Result<PathSegmentDescriptor, BackupError> {
    match str_field(value, "kind")? {
        "member" => Ok(PathSegmentDescriptor {
            kind: PathSegmentKind::Member,
            catalog_id: Some(str_field(value, "catalog_id")?.to_string()),
            key: None,
        }),
        "key" => Ok(PathSegmentDescriptor {
            kind: PathSegmentKind::Key,
            catalog_id: None,
            key: Some(key_from_json(value.get("key").ok_or_else(missing("key"))?)?),
        }),
        _ => Err(missing("kind")()),
    }
}

fn key_kind_text(kind: KeyKind) -> &'static str {
    match kind {
        KeyKind::Int => "int",
        KeyKind::Bool => "bool",
        KeyKind::Str => "str",
        KeyKind::Date => "date",
        KeyKind::Duration => "duration",
        KeyKind::Instant => "instant",
        KeyKind::Bytes => "bytes",
    }
}

fn key_kind(text: &str) -> Result<KeyKind, BackupError> {
    match text {
        "int" => Ok(KeyKind::Int),
        "bool" => Ok(KeyKind::Bool),
        "str" => Ok(KeyKind::Str),
        "date" => Ok(KeyKind::Date),
        "duration" => Ok(KeyKind::Duration),
        "instant" => Ok(KeyKind::Instant),
        "bytes" => Ok(KeyKind::Bytes),
        _ => Err(missing("kind")()),
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

    use super::commit_from_json;

    #[test]
    fn commit_manifest_requires_default_backfill_cell_evidence_field() {
        let commit = json!({
            "commit_id": 1,
            "catalog_epoch": 2,
            "layout_epoch": 0,
            "source_digest": "fnv1a64:0000000000000001",
            "engine_profile_digest": "0102030405060708",
            "changed_root_catalog_ids": [],
            "changed_index_catalog_ids": [],
            "activation_evolution_digest": "fnv1a64:0000000000000002",
            "activation_proposal_catalog_digest": null,
            "activation_proposal_catalog_json": null,
            "activation_records_backfilled": 0,
            "activation_default_records_by_id": [],
            "activation_indexes_rebuilt": 0,
            "activation_records_retired": 0,
            "activation_records_retired_by_id": [],
            "activation_records_transformed": 0,
        });

        let error = commit_from_json(&commit).expect_err("missing evidence is corrupt");

        assert_eq!(error.code(), "restore.format_version");
        assert!(
            error
                .to_string()
                .contains("activation_default_backfill_cells"),
            "{error}"
        );
    }
}
