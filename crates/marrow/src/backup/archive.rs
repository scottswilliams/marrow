//! The on-disk backup framing: a magic header, a JSON manifest, the
//! length-prefixed catalog section, and the length-prefixed cell stream, plus one
//! integrity checksum over all three. The framing is deterministic, so equal data
//! produces a byte-identical backup.

use std::io::{Read, Write};

use marrow_catalog::CatalogMetadata;
use serde_json::{Value, json};

use super::{
    BackupCorruptProblem, BackupError, BackupFormatProblem, BackupManifest, CommitDescriptor,
    EngineDescriptor, require_sha256_digest,
};
use marrow_store::tree::{
    EngineProfileDigest, TREE_BACKUP_MAX_CATALOG_SECTION_BYTES, TREE_BACKUP_MAX_CELL_BYTES,
    TREE_BACKUP_MAX_MANIFEST_BYTES, TreeBackupArchiveReadError, TreeBackupCell, TreeBackupCellBuf,
    TreeBackupCellReadError, fold_checksum_bytes, read_tree_backup_archive_chunk,
    read_tree_backup_archive_header, write_tree_backup_archive_chunk,
    write_tree_backup_archive_header,
};

const CHECKSUM_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Fold one cell into the running checksum over its framed bytes; write and read
/// sides must agree on exactly these bytes.
pub(super) fn checksum_cell(hash: u64, cell: TreeBackupCell<'_>) -> Result<u64, BackupError> {
    cell.fold_checksum(hash)
        .map_err(BackupError::cell_frame_too_large)
}

pub(super) const CHECKSUM_SEED: u64 = CHECKSUM_OFFSET;

/// Fold a length-prefixed byte run into the running checksum, the same framing the
/// header and catalog section are written with.
fn fold_chunk(hash: u64, bytes: &[u8]) -> u64 {
    let hash = fold_checksum_bytes(hash, &(bytes.len() as u32).to_be_bytes());
    fold_checksum_bytes(hash, bytes)
}

/// The integrity-checksum contribution of the manifest: its canonical bytes with
/// `archive_checksum` zeroed, so the field that records the checksum is excluded
/// from the bytes it covers. The read side recomputes this from the parsed
/// manifest, so a tampered manifest field changes the recomputed checksum.
pub(super) fn checksum_manifest(hash: u64, manifest: &BackupManifest) -> Result<u64, BackupError> {
    Ok(fold_chunk(hash, &manifest_checksum_bytes(manifest)?))
}

/// The integrity-checksum contribution of the catalog section: its framed bytes,
/// so a tampered catalog row changes the recomputed checksum.
pub(super) fn checksum_catalog_section(hash: u64, section: &[u8]) -> u64 {
    fold_chunk(hash, section)
}

fn manifest_checksum_bytes(manifest: &BackupManifest) -> Result<Vec<u8>, BackupError> {
    let mut value = manifest_to_json(manifest);
    value["archive_checksum"] = json!(0u64);
    manifest_json_bytes(&value)
}

fn manifest_json_bytes(value: &Value) -> Result<Vec<u8>, BackupError> {
    serde_json::to_vec(value).map_err(BackupError::ManifestSerialization)
}

/// The catalog section bytes for `snapshot`: the empty section when there is no
/// accepted catalog, else the canonical catalog JSON. The section is self-bracketing
/// so the read side decodes it without consulting the manifest.
pub(super) fn catalog_section_bytes(
    snapshot: Option<&CatalogMetadata>,
) -> Result<Vec<u8>, BackupError> {
    match snapshot {
        None => Ok(Vec::new()),
        Some(snapshot) => snapshot
            .to_json_pretty()
            .map(|json| json.into_bytes())
            .map_err(BackupError::CatalogSerialization),
    }
}

pub(super) fn write_header(
    out: &mut impl Write,
    manifest: &BackupManifest,
) -> Result<(), BackupError> {
    let manifest = manifest_json_bytes(&manifest_to_json(manifest))?;
    write_tree_backup_archive_header(out)?;
    write_tree_backup_archive_chunk(out, &manifest)?;
    Ok(())
}

/// Write the length-prefixed catalog section. An absent catalog writes a zero-length
/// section, so every backup carries the frame and the read side never branches on its
/// presence.
pub(super) fn write_catalog_section(out: &mut impl Write, section: &[u8]) -> std::io::Result<()> {
    write_tree_backup_archive_chunk(out, section)
}

pub(super) fn write_cell(out: &mut impl Write, cell: TreeBackupCell<'_>) -> std::io::Result<()> {
    cell.write_framed(out)
}

pub(super) fn read_header(input: &mut impl Read) -> Result<BackupManifest, BackupError> {
    read_tree_backup_archive_header(input).map_err(format_header_error)?;
    let bytes = read_tree_backup_archive_chunk(input, TREE_BACKUP_MAX_MANIFEST_BYTES, "manifest")
        .map_err(format_manifest_error)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        BackupError::format_version(
            BackupFormatProblem::ManifestInvalid,
            format!("backup manifest is not valid: {error}"),
        )
    })?;
    manifest_from_json(&value)
}

/// A catalog section read from the archive: its decoded snapshot (`None` when the
/// section is empty) and the exact framed bytes, which restore folds into the
/// integrity checksum so a tampered row is rejected.
pub(super) struct CatalogSection {
    pub(super) snapshot: Option<CatalogMetadata>,
    pub(super) bytes: Vec<u8>,
}

/// Read the length-prefixed catalog section. A zero-length section decodes to no
/// catalog; otherwise the bytes must be a valid catalog whose stored digest matches
/// its entries, so a tampered row fails closed here before any data is replayed.
pub(super) fn read_catalog_section(input: &mut impl Read) -> Result<CatalogSection, BackupError> {
    let bytes = read_tree_backup_archive_chunk(
        input,
        TREE_BACKUP_MAX_CATALOG_SECTION_BYTES,
        "catalog section",
    )
    .map_err(catalog_section_archive_error)?;
    let snapshot = if bytes.is_empty() {
        None
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| catalog_section_invalid())?;
        Some(CatalogMetadata::from_json(text).map_err(|_| catalog_section_invalid())?)
    };
    Ok(CatalogSection { snapshot, bytes })
}

fn catalog_section_invalid() -> BackupError {
    BackupError::corrupt(
        BackupCorruptProblem::CatalogSectionInvalid,
        "backup catalog section is not a valid accepted catalog",
    )
}

/// Read one framed cell, or a checksum/framing error if the stream is short.
pub(super) fn read_cell(input: &mut impl Read) -> Result<TreeBackupCellBuf, BackupError> {
    TreeBackupCellBuf::read_framed(input, TREE_BACKUP_MAX_CELL_BYTES).map_err(read_cell_error)
}

fn format_header_error(error: TreeBackupArchiveReadError) -> BackupError {
    match error {
        TreeBackupArchiveReadError::NotBackupFile => BackupError::format_version(
            BackupFormatProblem::NotBackupFile,
            "not a Marrow backup file".to_string(),
        ),
        TreeBackupArchiveReadError::UnsupportedVersion { found, expected } => {
            BackupError::format_version(
                BackupFormatProblem::UnsupportedVersion { found, expected },
                format!(
                    "backup format version {found} is unsupported (this build writes {expected})"
                ),
            )
        }
        TreeBackupArchiveReadError::HeaderTruncated
        | TreeBackupArchiveReadError::ChunkTruncated { .. }
        | TreeBackupArchiveReadError::ChunkTooLarge { .. } => {
            BackupError::format_version(BackupFormatProblem::HeaderTruncated, error.to_string())
        }
    }
}

fn format_manifest_error(error: TreeBackupArchiveReadError) -> BackupError {
    match error {
        TreeBackupArchiveReadError::ChunkTooLarge { .. } => BackupError::format_version(
            BackupFormatProblem::ManifestTooLarge,
            "backup manifest is implausibly large".to_string(),
        ),
        _ => BackupError::format_version(BackupFormatProblem::HeaderTruncated, error.to_string()),
    }
}

fn catalog_section_archive_error(error: TreeBackupArchiveReadError) -> BackupError {
    match error {
        TreeBackupArchiveReadError::ChunkTooLarge { .. } => BackupError::corrupt(
            BackupCorruptProblem::CatalogSectionTooLarge,
            "backup catalog section is implausibly large",
        ),
        _ => BackupError::corrupt(
            BackupCorruptProblem::CatalogSectionInvalid,
            format!("backup catalog section is truncated: {error}"),
        ),
    }
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
        "catalog_digest": manifest.catalog_digest,
        "state_digest": manifest.state_digest,
        "store_uid": manifest.store_uid,
        "parent_snapshot_digest": manifest.parent_snapshot_digest,
        "engine": {
            "name": manifest.engine.name,
            "layout_epoch": manifest.engine.layout_epoch,
            "key_profile_version": manifest.engine.key_profile_version,
            "value_codec_version": manifest.engine.value_codec_version,
            "profile_digest": crate::hex_string(&manifest.engine.profile_digest),
        },
        "commit": manifest.commit.as_ref().map(commit_to_json),
        "record_count": manifest.record_count,
        "archive_checksum": manifest.archive_checksum,
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
    })
}

fn manifest_from_json(value: &Value) -> Result<BackupManifest, BackupError> {
    let engine = object_field(value, "engine")?;
    let manifest = BackupManifest {
        format_version: u32_field(value, "format_version")?,
        source_digest: str_field(value, "source_digest")?.to_string(),
        catalog_epoch: opt_u64_field(value, "catalog_epoch")?,
        catalog_digest: opt_str_field(value, "catalog_digest")?,
        state_digest: str_field(value, "state_digest")?.to_string(),
        store_uid: store_uid_field(value, "store_uid")?,
        parent_snapshot_digest: parent_snapshot_digest_field(value)?,
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
        archive_checksum: u64_field(value, "archive_checksum")?,
    };
    require_sha256_digest("source_digest", &manifest.source_digest)?;
    if let Some(digest) = &manifest.catalog_digest {
        require_sha256_digest("catalog_digest", digest)?;
    }
    require_sha256_digest("state_digest", &manifest.state_digest)?;
    Ok(manifest)
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
    };
    commit.validate_digest_shapes()?;
    Ok(commit)
}

fn store_uid_field(value: &Value, field: &'static str) -> Result<String, BackupError> {
    let uid = str_field(value, field)?;
    marrow_store::tree::StoreUid::new(uid.to_string()).map_err(|_| {
        BackupError::format_version(
            BackupFormatProblem::FieldType {
                field,
                expected: "store uid",
            },
            format!("backup manifest field `{field}` must be a store uid"),
        )
    })?;
    Ok(uid.to_string())
}

fn parent_snapshot_digest_field(value: &Value) -> Result<Option<String>, BackupError> {
    match opt_str_field(value, "parent_snapshot_digest")? {
        None => Ok(None),
        Some(digest) if digest.is_empty() => Ok(None),
        Some(_) => Err(BackupError::format_version(
            BackupFormatProblem::ReservedFieldNonEmpty {
                field: "parent_snapshot_digest",
            },
            "backup parent_snapshot_digest is reserved for a later format".to_string(),
        )),
    }
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
        BackupError, BackupFormatProblem, BackupManifest, CommitDescriptor, EngineDescriptor,
        FORMAT_VERSION,
    };
    use super::{commit_to_json, manifest_from_json, manifest_to_json};

    fn minimal_manifest() -> serde_json::Value {
        json!({
            "format_version": FORMAT_VERSION,
            "source_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000001",
            "catalog_epoch": null,
            "catalog_digest": null,
            "state_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "store_uid": "store_00000000000000000000000000000001",
            "parent_snapshot_digest": null,
            "engine": {
                "name": super::super::ENGINE_NAME,
                "layout_epoch": 0,
                "key_profile_version": 0,
                "value_codec_version": marrow_store::value::VALUE_CODEC_VERSION,
                "profile_digest": "0102030405060708",
            },
            "commit": null,
            "record_count": 0,
            "archive_checksum": 0,
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
            format_version: FORMAT_VERSION,
            source_digest: digest(1),
            catalog_epoch: Some(7),
            catalog_digest: Some(digest(6)),
            state_digest: digest(7),
            store_uid: "store_00000000000000000000000000000001".to_string(),
            parent_snapshot_digest: None,
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
            }),
            record_count: 42,
            archive_checksum: 0xdead_beef,
        };

        let restored =
            manifest_from_json(&manifest_to_json(&manifest)).expect("manifest round-trips");

        assert_eq!(restored, manifest);
    }

    #[test]
    fn manifest_json_uses_the_incompatible_shape() {
        let manifest = minimal_manifest();

        assert_eq!(
            manifest["format_version"],
            json!(6),
            "this incompatible manifest shape changes the manifest and commit descriptor byte surface"
        );
        for field in [
            "source_digest",
            "catalog_epoch",
            "catalog_digest",
            "state_digest",
            "store_uid",
            "parent_snapshot_digest",
            "engine",
            "commit",
            "record_count",
            "archive_checksum",
        ] {
            assert!(manifest.get(field).is_some(), "manifest missing {field}");
        }
    }

    #[test]
    fn commit_json_omits_activation_receipt_payloads() {
        let digest = |seed: u8| format!("sha256:{}", format!("{seed:02x}").repeat(32));
        let commit = CommitDescriptor {
            commit_id: 11,
            catalog_epoch: 7,
            layout_epoch: 3,
            source_digest: digest(1),
            engine_profile_digest: [9, 8, 7, 6, 5, 4, 3, 2],
            changed_root_catalog_ids: vec!["cat_00000000000000000000000000000001".to_string()],
            changed_index_catalog_ids: vec!["cat_00000000000000000000000000000002".to_string()],
        };
        let json = commit_to_json(&commit);
        let object = json.as_object().expect("commit json object");

        for field in [
            activation_field("_evolution_digest"),
            activation_field("_proposal_catalog_digest"),
            activation_field("_proposal_new_catalog_ids"),
            activation_field("_records_backfilled"),
            activation_field("_default_records_by_id"),
            activation_field("_indexes_rebuilt"),
            activation_field("_records_retired"),
            activation_field("_retire_evidence_digest"),
            activation_field("_records_retired_by_id"),
            activation_field("_records_transformed"),
        ] {
            assert!(
                !object.contains_key(field.as_str()),
                "backup commit descriptor must not persist {field}"
            );
        }
    }

    fn activation_field(suffix: &str) -> String {
        ["activation", suffix].concat()
    }

    #[test]
    fn manifest_rejects_non_empty_parent_snapshot_digest() {
        let mut manifest = minimal_manifest();
        manifest["parent_snapshot_digest"] =
            json!("sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");

        let error = manifest_from_json(&manifest)
            .expect_err("parent snapshot chains are reserved for a later format");

        assert_eq!(error.code(), "restore.format_version");
        assert!(matches!(
            error,
            BackupError::FormatVersion {
                problem: BackupFormatProblem::ReservedFieldNonEmpty {
                    field: "parent_snapshot_digest"
                },
                ..
            }
        ));
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
}
