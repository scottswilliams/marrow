//! Typed portable backup of a project's saved data.
//!
//! A backup is a Marrow artifact, not a raw engine-file copy: a small header, a
//! typed manifest, the accepted-catalog section, and the canonical ordered
//! data-cell stream. The manifest binds the data to the program that wrote it —
//! its source digest, accepted catalog epoch and digest, engine profile,
//! value-codec version, and one integrity checksum — so a restore refuses data
//! outside this binary's checked contract. The catalog section carries the
//! engine-resident accepted catalog rows so a restored store is self-contained
//! and runs immediately; the data stream carries the store's data cells only,
//! never catalog rows. Generated indexes are derived, so a restore rebuilds them
//! rather than replaying them.
//!
//! The integrity checksum is one bounded streaming fold over the manifest bytes,
//! the catalog section, and the data cells, so a tampered manifest or catalog row
//! is rejected on restore just as a tampered data cell is.
//!
//! The cell stream is backend-independent and stores typed cell targets derived
//! from catalog stable IDs. Stable IDs are random opaque values that freeze when
//! accepted. Backups are deterministic and portable across conforming backends at
//! the same layout and codec, but byte identity requires matching accepted
//! catalog facts, engine profile, value codec, and stored data.
//!
//! [`create`] writes a backup over a stable read snapshot; [`restore`] validates a
//! backup against the project and replays its catalog rows and data cells into an
//! empty store in one transaction. The on-disk framing and the manifest live
//! here; the two operations live in their own modules.

mod archive;
mod create;
mod restore;

pub(crate) use create::create_backup;
pub(crate) use restore::{BackupPrologue, read_backup_prologue, restore_backup_with_prologue};

use marrow_store::tree::{CommitMetadata, EngineProfile, EngineProfileDigest};

/// The on-disk format version. It advances only on an incompatible change to the
/// header, manifest, or cell framing.
pub(crate) const FORMAT_VERSION: u32 = 4;

/// A short name identifying the engine family a backup was taken from. v0.1 has
/// one; the layout, key-profile, and value-codec versions distinguish revisions.
pub(crate) const ENGINE_NAME: &str = "marrow-tree-cell";

/// The typed header binding a backup's data to the program and engine that wrote
/// it. Restore validates every field before replaying a single cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupManifest {
    pub(crate) format_version: u32,
    /// The schema-bearing source digest of the program that wrote the data.
    pub(crate) source_digest: String,
    /// `None` for an unstamped store.
    pub(crate) catalog_epoch: Option<u64>,
    /// The accepted catalog's digest, fingerprinting the rows the catalog section
    /// carries (the digest is taken over the epoch and every entry, so it also pins
    /// their count). `None` for a store with no accepted catalog.
    pub(crate) catalog_digest: Option<String>,
    /// The engine profile the data was written under, so restore refuses a layout
    /// or codec it cannot reproduce (reporting an engine recompile is required).
    pub(crate) engine: EngineDescriptor,
    /// Replayed so the restored store fences exactly like the original. `None` for
    /// an unstamped store.
    pub(crate) commit: Option<CommitDescriptor>,
    /// How many tree cells the data stream carries.
    pub(crate) record_count: u64,
    /// One bounded streaming fold over the manifest bytes (with this field zeroed),
    /// the catalog section, and the data cells. A tampered manifest, catalog row, or
    /// data cell fails this check before restore commits.
    pub(crate) archive_checksum: u64,
}

/// The engine identity a restore must match to replay bytes verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EngineDescriptor {
    pub(crate) name: String,
    pub(crate) layout_epoch: u64,
    pub(crate) key_profile_version: u8,
    pub(crate) value_codec_version: u32,
    pub(crate) profile_digest: EngineProfileDigest,
}

impl EngineDescriptor {
    /// The engine identity the running binary writes and restores into. The layout
    /// epoch and profile digest come from the running profile.
    pub(crate) fn current(profile: &EngineProfile) -> Self {
        Self::build(profile, profile.layout_epoch(), profile.digest_bytes())
    }

    /// The engine identity a backup records: the running binary's name, key-profile,
    /// and value-codec versions, but the store's recorded layout epoch and profile
    /// digest when present so a backup describes the engine its data was written
    /// under. An unstamped store falls back to the running profile's values.
    pub(crate) fn recorded(
        profile: &EngineProfile,
        recorded_layout_epoch: Option<u64>,
        recorded_profile_digest: Option<EngineProfileDigest>,
    ) -> Self {
        Self::build(
            profile,
            recorded_layout_epoch.unwrap_or_else(|| profile.layout_epoch()),
            recorded_profile_digest.unwrap_or_else(|| profile.digest_bytes()),
        )
    }

    fn build(
        profile: &EngineProfile,
        layout_epoch: u64,
        profile_digest: EngineProfileDigest,
    ) -> Self {
        Self {
            name: ENGINE_NAME.to_string(),
            layout_epoch,
            key_profile_version: profile.key_profile_version(),
            value_codec_version: marrow_store::value::VALUE_CODEC_VERSION,
            profile_digest,
        }
    }
}

/// The commit metadata a backup records and a restore restamps, mirroring
/// [`CommitMetadata`] with catalog ids carried as their opaque text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitDescriptor {
    pub(crate) commit_id: u64,
    pub(crate) catalog_epoch: u64,
    pub(crate) layout_epoch: u64,
    pub(crate) source_digest: String,
    pub(crate) engine_profile_digest: EngineProfileDigest,
    pub(crate) changed_root_catalog_ids: Vec<String>,
    pub(crate) changed_index_catalog_ids: Vec<String>,
    pub(crate) activation_evolution_digest: String,
    pub(crate) activation_proposal_catalog_digest: Option<String>,
    pub(crate) activation_proposal_new_catalog_ids: Vec<String>,
    pub(crate) activation_records_backfilled: u64,
    pub(crate) activation_default_records_by_id: Vec<DefaultCountDescriptor>,
    pub(crate) activation_indexes_rebuilt: u64,
    pub(crate) activation_records_retired: u64,
    pub(crate) activation_retire_evidence_digest: String,
    pub(crate) activation_records_retired_by_id: Vec<RetireCountDescriptor>,
    pub(crate) activation_records_transformed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultCountDescriptor {
    pub(crate) catalog_id: String,
    pub(crate) records_backfilled: u64,
    pub(crate) target_records: u64,
    pub(crate) evidence_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetireCountDescriptor {
    pub(crate) catalog_id: String,
    pub(crate) records: u64,
}

fn owned_ids(ids: &[marrow_store::cell::CatalogId]) -> Vec<String> {
    ids.iter().map(|id| id.as_str().to_string()).collect()
}

impl CommitDescriptor {
    pub(crate) fn from_metadata(metadata: &CommitMetadata) -> Self {
        Self {
            commit_id: metadata.commit_id,
            catalog_epoch: metadata.catalog_epoch,
            layout_epoch: metadata.layout_epoch,
            source_digest: metadata.source_digest.clone(),
            engine_profile_digest: metadata.engine_profile_digest,
            changed_root_catalog_ids: owned_ids(&metadata.changed_root_catalog_ids),
            changed_index_catalog_ids: owned_ids(&metadata.changed_index_catalog_ids),
            activation_evolution_digest: metadata.activation_evolution_digest.clone(),
            activation_proposal_catalog_digest: metadata.activation_proposal_catalog_digest.clone(),
            activation_proposal_new_catalog_ids: owned_ids(
                &metadata.activation_proposal_new_catalog_ids,
            ),
            activation_records_backfilled: metadata.activation_records_backfilled,
            activation_default_records_by_id: metadata
                .activation_default_records_by_id
                .iter()
                .map(|count| DefaultCountDescriptor {
                    catalog_id: count.catalog_id.as_str().to_string(),
                    records_backfilled: count.records_backfilled,
                    target_records: count.target_records,
                    evidence_digest: count.evidence_digest.clone(),
                })
                .collect(),
            activation_indexes_rebuilt: metadata.activation_indexes_rebuilt,
            activation_records_retired: metadata.activation_records_retired,
            activation_retire_evidence_digest: metadata.activation_retire_evidence_digest.clone(),
            activation_records_retired_by_id: metadata
                .activation_records_retired_by_id
                .iter()
                .map(|(catalog_id, records)| RetireCountDescriptor {
                    catalog_id: catalog_id.as_str().to_string(),
                    records: *records,
                })
                .collect(),
            activation_records_transformed: metadata.activation_records_transformed,
        }
    }

    pub(crate) fn validate_manifest_binding(
        &self,
        manifest: &BackupManifest,
    ) -> Result<(), BackupError> {
        self.validate_digest_shapes()?;
        if Some(self.catalog_epoch) != manifest.catalog_epoch
            || self.source_digest != manifest.source_digest
            || self.layout_epoch != manifest.engine.layout_epoch
            || self.engine_profile_digest != manifest.engine.profile_digest
        {
            return Err(BackupError::corrupt(
                BackupCorruptProblem::ManifestCommitBindingMismatch,
                "manifest commit metadata disagrees with the backup binding",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_digest_shapes(&self) -> Result<(), BackupError> {
        require_sha256_digest("source_digest", &self.source_digest)?;
        require_optional_sha256_digest(
            "activation_evolution_digest",
            &self.activation_evolution_digest,
        )?;
        if let Some(digest) = &self.activation_proposal_catalog_digest {
            require_sha256_digest("activation_proposal_catalog_digest", digest)?;
        }
        for count in &self.activation_default_records_by_id {
            require_sha256_digest("evidence_digest", &count.evidence_digest)?;
        }
        require_optional_sha256_digest(
            "activation_retire_evidence_digest",
            &self.activation_retire_evidence_digest,
        )?;
        Ok(())
    }

    /// Rebuild the engine-facing commit metadata, rejecting malformed ids or digest
    /// evidence as a corrupt manifest.
    pub(crate) fn to_metadata(&self) -> Result<CommitMetadata, BackupError> {
        self.validate_digest_shapes()?;
        Ok(CommitMetadata {
            commit_id: self.commit_id,
            catalog_epoch: self.catalog_epoch,
            layout_epoch: self.layout_epoch,
            source_digest: self.source_digest.clone(),
            engine_profile_digest: self.engine_profile_digest,
            changed_root_catalog_ids: catalog_ids(&self.changed_root_catalog_ids)?,
            changed_index_catalog_ids: catalog_ids(&self.changed_index_catalog_ids)?,
            activation_evolution_digest: self.activation_evolution_digest.clone(),
            activation_proposal_catalog_digest: self.activation_proposal_catalog_digest.clone(),
            activation_proposal_new_catalog_ids: catalog_ids(
                &self.activation_proposal_new_catalog_ids,
            )?,
            activation_records_backfilled: self.activation_records_backfilled,
            activation_default_records_by_id: default_counts(
                &self.activation_default_records_by_id,
            )?,
            activation_indexes_rebuilt: self.activation_indexes_rebuilt,
            activation_records_retired: self.activation_records_retired,
            activation_retire_evidence_digest: self.activation_retire_evidence_digest.clone(),
            activation_records_retired_by_id: retire_counts(
                &self.activation_records_retired_by_id,
            )?,
            activation_records_transformed: self.activation_records_transformed,
        })
    }
}

fn require_optional_sha256_digest(field: &'static str, digest: &str) -> Result<(), BackupError> {
    if digest.is_empty() {
        Ok(())
    } else {
        require_sha256_digest(field, digest)
    }
}

fn require_sha256_digest(field: &'static str, digest: &str) -> Result<(), BackupError> {
    if sha256_digest_spelling(digest) {
        Ok(())
    } else {
        Err(BackupError::format_version(
            BackupFormatProblem::DigestSpelling { field },
            format!("`{field}` must be a sha256 digest"),
        ))
    }
}

fn sha256_digest_spelling(digest: &str) -> bool {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The single construction site for `MalformedCatalogId`, so a bad manifest id is
/// rejected as corrupt at one point.
fn catalog_id(text: &str) -> Result<marrow_store::cell::CatalogId, BackupError> {
    marrow_store::cell::CatalogId::new(text.to_string()).map_err(|_| {
        BackupError::corrupt(
            BackupCorruptProblem::MalformedCatalogId,
            "manifest carries a malformed catalog id",
        )
    })
}

fn catalog_ids(ids: &[String]) -> Result<Vec<marrow_store::cell::CatalogId>, BackupError> {
    ids.iter().map(|id| catalog_id(id)).collect()
}

fn default_counts(
    counts: &[DefaultCountDescriptor],
) -> Result<Vec<marrow_store::tree::ActivationDefaultRecordCount>, BackupError> {
    counts
        .iter()
        .map(|count| {
            Ok(marrow_store::tree::ActivationDefaultRecordCount {
                catalog_id: catalog_id(&count.catalog_id)?,
                records_backfilled: count.records_backfilled,
                target_records: count.target_records,
                evidence_digest: count.evidence_digest.clone(),
            })
        })
        .collect()
}

fn retire_counts(
    counts: &[RetireCountDescriptor],
) -> Result<Vec<(marrow_store::cell::CatalogId, u64)>, BackupError> {
    counts
        .iter()
        .map(|count| Ok((catalog_id(&count.catalog_id)?, count.records)))
        .collect()
}

/// A backup or restore failure, carrying a stable dotted code for tools.
#[derive(Debug)]
pub(crate) enum BackupError {
    /// The backup file could not be read or written.
    Io(std::io::Error),
    /// A store read or write failed.
    Store(marrow_store::StoreError),
    /// The backup header or manifest is not a Marrow backup this build understands.
    /// Production reports only `code()` and `message`; the typed `problem` is a
    /// test-observable discriminator (tests assert the precise framing fault and its
    /// flagged field), not a production-consumed value, so it is never rendered.
    FormatVersion {
        problem: BackupFormatProblem,
        message: String,
    },
    /// A cell chunk failed its checksum or framing. As with `FormatVersion`, the typed
    /// `problem` is a test-observable discriminator only; production reports `code()`
    /// and `message`.
    CorruptChunk {
        problem: BackupCorruptProblem,
        message: String,
    },
    /// The restore target already holds saved data or an accepted catalog.
    NotEmpty,
    /// The backup was written under a different engine, layout, or value codec.
    EngineRecompileRequired(String),
    /// The backup's schema does not match the project being restored into.
    SourceMismatch(String),
    /// The backup's catalog epoch does not match the project's accepted catalog.
    CatalogMismatch(String),
    /// The restored data does not validate against the project schema.
    DataInvalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupFormatProblem {
    NotBackupFile,
    UnsupportedVersion {
        found: u32,
        expected: u32,
    },
    HeaderTruncated,
    ManifestTooLarge,
    ManifestInvalid,
    MissingField {
        field: &'static str,
    },
    FieldType {
        field: &'static str,
        expected: &'static str,
    },
    FieldOutOfRange {
        field: &'static str,
    },
    DigestLength {
        field: &'static str,
    },
    DigestHex {
        field: &'static str,
    },
    DigestSpelling {
        field: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupCorruptProblem {
    CellStreamEndedEarly,
    CellTooLarge,
    MalformedCell,
    ManifestCommitBindingMismatch,
    ManifestCatalogBindingMismatch,
    MalformedCatalogId,
    CatalogSectionTooLarge,
    CatalogSectionInvalid,
    CatalogDigestMismatch,
    ChecksumMismatch,
    TrailingBytes,
}

impl BackupError {
    fn format_version(problem: BackupFormatProblem, message: String) -> Self {
        Self::FormatVersion { problem, message }
    }

    fn corrupt(problem: BackupCorruptProblem, message: impl Into<String>) -> Self {
        Self::CorruptChunk {
            problem,
            message: message.into(),
        }
    }

    /// The stable dotted code a tool reports for this failure.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io.write",
            Self::Store(error) => error.code(),
            Self::FormatVersion { .. } => "restore.format_version",
            Self::CorruptChunk { .. } => "restore.corrupt_chunk",
            Self::NotEmpty => "restore.not_empty",
            Self::EngineRecompileRequired(_) => "restore.engine_recompile_required",
            Self::SourceMismatch(_) => "restore.source_mismatch",
            Self::CatalogMismatch(_) => "restore.catalog_mismatch",
            Self::DataInvalid(_) => "restore.data_invalid",
        }
    }
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "backup i/o failed: {error}"),
            Self::Store(error) => write!(f, "{error}"),
            Self::FormatVersion { problem, message } => {
                let _ = problem;
                write!(f, "{message}")
            }
            Self::CorruptChunk { problem, message } => {
                let _ = problem;
                write!(f, "{message}")
            }
            Self::EngineRecompileRequired(message)
            | Self::SourceMismatch(message)
            | Self::CatalogMismatch(message)
            | Self::DataInvalid(message) => write!(f, "{message}"),
            Self::NotEmpty => write!(
                f,
                "the restore target already holds saved data or an accepted catalog; restore writes into an empty store"
            ),
        }
    }
}

impl From<std::io::Error> for BackupError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<marrow_store::StoreError> for BackupError {
    fn from(error: marrow_store::StoreError) -> Self {
        Self::Store(error)
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use std::path::PathBuf;

    use marrow_check::{CheckedProgram, ProjectConfig, check_project, check_project_with_catalog};

    pub(super) const BOOK_SOURCE: &str =
        "module shelf\n\nresource Book at ^books(id: int)\n    required title: string\n";

    pub(super) fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create project root");
        root
    }

    fn config() -> ProjectConfig {
        ProjectConfig {
            source_roots: vec!["src".into()],
            default_entry: None,
            store: None,
            tests: Vec::new(),
        }
    }

    pub(super) fn committed_program(name: &str, source: &str) -> (PathBuf, CheckedProgram) {
        let root = temp_dir(name);
        let path = root.join("src/shelf.mw");
        std::fs::create_dir_all(path.parent().unwrap()).expect("create src");
        std::fs::write(&path, source).expect("write source");
        let (report, program) = check_project(&root, &config()).expect("check project");
        assert!(!report.has_errors(), "{:#?}", report.diagnostics);
        // Freeze the baseline into an engine-resident store and re-bind against the
        // accepted snapshot, the way a state-establishing run does.
        let store = marrow_store::tree::TreeStore::memory();
        marrow_run::evolution::commit_catalog_baseline(&store, &program)
            .expect("commit catalog baseline");
        let accepted = store
            .read_catalog_snapshot()
            .expect("read catalog snapshot");
        let (report, program) =
            check_project_with_catalog(&root, &config(), accepted.as_ref()).expect("re-check");
        assert!(!report.has_errors(), "{:#?}", report.diagnostics);
        (root, program)
    }
}

#[cfg(test)]
mod tests {
    use marrow_store::cell::CatalogId;
    use marrow_store::tree::{ActivationDefaultRecordCount, CommitMetadata};

    use super::CommitDescriptor;

    fn catalog(text: &str) -> CatalogId {
        CatalogId::new(text.to_string()).expect("valid catalog id")
    }

    #[test]
    fn commit_descriptor_preserves_bounded_default_evidence() {
        let metadata = CommitMetadata {
            commit_id: 12,
            catalog_epoch: 9,
            layout_epoch: 1,
            source_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000001"
                    .to_string(),
            engine_profile_digest: [1, 3, 5, 7, 9, 11, 13, 15],
            changed_root_catalog_ids: vec![catalog("cat_00000000000000000000000000000001")],
            changed_index_catalog_ids: Vec::new(),
            activation_evolution_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000002"
                    .to_string(),
            activation_proposal_catalog_digest: Some(
                "sha256:0000000000000000000000000000000000000000000000000000000000000003"
                    .to_string(),
            ),
            activation_proposal_new_catalog_ids: vec![catalog(
                "cat_00000000000000000000000000000005",
            )],
            activation_records_backfilled: 1,
            activation_default_records_by_id: vec![ActivationDefaultRecordCount {
                catalog_id: catalog("cat_00000000000000000000000000000005"),
                records_backfilled: 1,
                target_records: 2,
                evidence_digest:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000004"
                        .to_string(),
            }],
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest: String::new(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        };

        let descriptor = CommitDescriptor::from_metadata(&metadata);
        let restored = descriptor
            .to_metadata()
            .expect("descriptor restores metadata");

        assert_eq!(restored, metadata);
    }
}
