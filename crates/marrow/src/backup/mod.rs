//! Typed portable backup of a project's saved data.
//!
//! A backup is a Marrow artifact, not a raw engine-file copy: a small header, a
//! typed manifest, the accepted-catalog section, and the canonical ordered
//! data-cell stream. The manifest binds the data to the program that wrote it —
//! its source digest, accepted catalog epoch and digest, engine profile,
//! value-codec version, and one integrity checksum — so a restore refuses data
//! outside this binary's checked contract. The catalog section carries the
//! committed accepted catalog rows so a restored store is self-contained and
//! runs immediately; the data stream carries the store's data cells only, never
//! catalog rows. Generated indexes are derived, so a restore rebuilds them rather
//! than replaying them.
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
mod artifact;
mod create;
mod restore;

pub(crate) use artifact::create_backup_artifact;
pub(crate) use create::create_backup;
pub(crate) use restore::{
    BackupPrologue, RestoreReceipt, RestoreReport, RestoreTargetMode,
    mount_backup_for_evolution_preview, read_backup_prologue, restore_backup_with_prologue,
    validate_backup_archive,
};

use marrow_run::Nondeterminism;
use marrow_store::tree::{CommitMetadata, EngineProfile, EngineProfileDigest, StoreUid, TreeStore};

/// The on-disk format version. It advances only on an incompatible change to the
/// header, manifest, or cell framing.
pub(crate) const FORMAT_VERSION: u32 = marrow_store::tree::TREE_BACKUP_ARCHIVE_FORMAT_VERSION;

/// A short name identifying the engine family a backup was taken from. v0.1 has
/// one; the layout, key-profile, and value-codec versions distinguish revisions.
pub(crate) const ENGINE_NAME: &str = "marrow-tree-cell";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CatalogFingerprintRef<'a> {
    pub(crate) epoch: Option<u64>,
    pub(crate) digest: Option<&'a str>,
}

impl<'a> CatalogFingerprintRef<'a> {
    pub(crate) fn from_catalog(catalog: Option<&'a marrow_catalog::CatalogMetadata>) -> Self {
        catalog.map_or(
            Self {
                epoch: None,
                digest: None,
            },
            |catalog| Self {
                epoch: Some(catalog.epoch),
                digest: Some(catalog.digest.as_str()),
            },
        )
    }

    pub(crate) fn from_parts(epoch: Option<u64>, digest: Option<&'a str>) -> Self {
        Self { epoch, digest }
    }
}

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
    /// Digest of the canonical ordered data-cell stream.
    pub(crate) state_digest: String,
    /// The physical store identity the backup was taken from.
    pub(crate) store_uid: String,
    /// Reserved for chained backups. v0.1 writes and accepts only the empty sentinel.
    pub(crate) parent_snapshot_digest: Option<String>,
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
    /// and value-codec versions, with layout and profile digest supplied by the
    /// store's commit metadata when the store is stamped. An unstamped store falls
    /// back to the running profile's values.
    pub(crate) fn recorded(profile: &EngineProfile, commit: Option<&CommitMetadata>) -> Self {
        Self::build(
            profile,
            commit.map_or_else(|| profile.layout_epoch(), |commit| commit.layout_epoch),
            commit.map_or_else(
                || profile.digest_bytes(),
                |commit| commit.engine_profile_digest,
            ),
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
                BackupCorruptProblem::ManifestCommitDescriptorMismatch,
                "manifest fields disagree with the embedded commit descriptor",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_digest_shapes(&self) -> Result<(), BackupError> {
        require_sha256_digest("source_digest", &self.source_digest)?;
        Ok(())
    }

    /// Rebuild the engine-facing commit metadata, rejecting malformed ids or
    /// digest spelling as a corrupt manifest.
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
        })
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

pub(crate) fn ensure_store_uid(
    store: &TreeStore,
    nondeterminism: &mut impl Nondeterminism,
) -> Result<StoreUid, BackupError> {
    if let Some(uid) = store.read_store_uid()? {
        return Ok(uid);
    }
    let uid = mint_store_uid(nondeterminism)?;
    store.write_store_uid(&uid)?;
    Ok(uid)
}

pub(crate) fn require_store_uid(store: &TreeStore) -> Result<StoreUid, BackupError> {
    store.read_store_uid()?.ok_or(BackupError::StoreUidMissing)
}

pub(crate) fn mint_store_uid(
    nondeterminism: &mut impl Nondeterminism,
) -> Result<StoreUid, BackupError> {
    let entropy = nondeterminism.entropy_u128()?;
    Ok(StoreUid::from_entropy_bytes(entropy.to_be_bytes()))
}

/// A backup or restore failure, carrying a stable dotted code for tools.
#[derive(Debug)]
pub(crate) enum BackupError {
    /// The backup file could not be read or written.
    Io(std::io::Error),
    /// A store read or write failed.
    Store(marrow_store::StoreError),
    /// The accepted catalog section could not be serialized for backup.
    CatalogSerialization(marrow_catalog::CatalogError),
    /// The backup manifest could not be serialized for backup.
    ManifestSerialization(serde_json::Error),
    /// A live store cell cannot be represented in the backup frame.
    CellFrameTooLarge,
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
    NotEmpty(String),
    /// Backup requires an already-stamped physical store identity.
    StoreUidMissing,
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
    ReservedFieldNonEmpty {
        field: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupCorruptProblem {
    CellStreamEndedEarly,
    CellTooLarge,
    MalformedCell,
    ManifestCommitDescriptorMismatch,
    ManifestCatalogBindingMismatch,
    MalformedCatalogId,
    CatalogSectionTooLarge,
    CatalogSectionInvalid,
    CatalogDigestMismatch,
    StateDigestMismatch,
    ChecksumMismatch,
    TrailingBytes,
}

impl BackupError {
    pub(crate) fn source_mismatch(backup_source_digest: &str, project_source_digest: &str) -> Self {
        Self::SourceMismatch(format!(
            "backup source digest {backup_source_digest}; project source digest {project_source_digest}"
        ))
    }

    pub(crate) fn catalog_mismatch(
        backup: CatalogFingerprintRef<'_>,
        project: CatalogFingerprintRef<'_>,
    ) -> Self {
        Self::CatalogMismatch(format!(
            "backup catalog epoch {}; backup catalog digest {}; project catalog epoch {}; project catalog digest {}",
            display_epoch(backup.epoch),
            backup.digest.unwrap_or("none"),
            display_epoch(project.epoch),
            project.digest.unwrap_or("none"),
        ))
    }

    pub(crate) fn target_not_empty() -> Self {
        Self::NotEmpty(
            "the restore target already holds saved data or an accepted catalog; restore writes into an empty store unless --replace --count confirms the live record count"
                .to_string(),
        )
    }

    pub(crate) fn replace_count_mismatch(expected: u64, found: u64) -> Self {
        Self::NotEmpty(format!(
            "restore --replace expected {expected} live record(s), found {found}; target was not changed"
        ))
    }

    fn format_version(problem: BackupFormatProblem, message: String) -> Self {
        Self::FormatVersion { problem, message }
    }

    fn corrupt(problem: BackupCorruptProblem, message: impl Into<String>) -> Self {
        Self::CorruptChunk {
            problem,
            message: message.into(),
        }
    }

    fn cell_frame_too_large(_: marrow_store::tree::TreeBackupCellFrameError) -> Self {
        Self::CellFrameTooLarge
    }

    /// The stable dotted code a tool reports for this failure.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io.write",
            Self::Store(error) => error.code(),
            Self::CatalogSerialization(_) => "backup.catalog_serialization",
            Self::ManifestSerialization(_) => "backup.manifest_serialization",
            Self::CellFrameTooLarge => "backup.cell_too_large",
            Self::FormatVersion { .. } => "restore.format_version",
            Self::CorruptChunk { .. } => "restore.corrupt_chunk",
            Self::NotEmpty(_) => "restore.not_empty",
            Self::StoreUidMissing => "backup.store_uid_missing",
            Self::EngineRecompileRequired(_) => "restore.engine_recompile_required",
            Self::SourceMismatch(_) => "restore.source_mismatch",
            Self::CatalogMismatch(_) => "restore.catalog_mismatch",
            Self::DataInvalid(_) => "restore.data_invalid",
        }
    }
}

fn display_epoch(epoch: Option<u64>) -> String {
    epoch.map_or_else(|| "none".to_string(), |epoch| epoch.to_string())
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "backup i/o failed: {error}"),
            Self::Store(error) => write!(f, "{error}"),
            Self::CatalogSerialization(error) => {
                write!(f, "backup catalog serialization failed: {error}")
            }
            Self::ManifestSerialization(error) => {
                write!(f, "backup manifest serialization failed: {error}")
            }
            Self::CellFrameTooLarge => write!(f, "backup cell is too large to frame"),
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
            | Self::DataInvalid(message)
            | Self::NotEmpty(message) => write!(f, "{message}"),
            Self::StoreUidMissing => write!(
                f,
                "the store has no physical store UID; run or evolve apply must stamp the store before backup"
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
    use marrow_project::{StoreBackend, StoreConfig};

    pub(super) const BOOK_SOURCE: &str =
        "module shelf\n\nresource Book\n    required title: string\nstore ^books(id: int): Book\n";

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
            store: StoreConfig {
                backend: StoreBackend::Memory,
                data_dir: None,
            },
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
        // Commit the baseline through the store transaction and re-bind against the
        // accepted snapshot, the way a state-establishing run does before rendering the
        // catalog file.
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
    use std::io;

    use marrow_run::{FixedNondeterminism, Nondeterminism};
    use marrow_store::cell::CatalogId;
    use marrow_store::tree::CommitMetadata;
    use marrow_store::tree::TreeStore;

    use super::{BackupError, CommitDescriptor, ensure_store_uid, mint_store_uid};

    struct FailingNondeterminism;

    impl Nondeterminism for FailingNondeterminism {
        fn now_nanos(&self) -> i128 {
            0
        }

        fn entropy_u128(&mut self) -> io::Result<u128> {
            Err(io::Error::other("entropy unavailable"))
        }
    }

    fn catalog(text: &str) -> CatalogId {
        CatalogId::new(text.to_string()).expect("valid catalog id")
    }

    #[test]
    fn catalog_serialization_has_a_stable_backup_code() {
        let catalog_error =
            marrow_catalog::CatalogMetadata::from_json("{").expect_err("invalid catalog");
        let catalog_message = catalog_error.to_string();
        let error = BackupError::CatalogSerialization(catalog_error);

        assert_eq!(error.code(), "backup.catalog_serialization");
        assert!(error.to_string().contains(&catalog_message));
    }

    #[test]
    fn manifest_serialization_has_a_stable_backup_code() {
        let serde_error = serde_json::from_str::<serde_json::Value>("{").expect_err("invalid json");
        let serde_message = serde_error.to_string();
        let error = BackupError::ManifestSerialization(serde_error);

        assert_eq!(error.code(), "backup.manifest_serialization");
        assert_eq!(
            error.to_string(),
            format!("backup manifest serialization failed: {serde_message}")
        );
    }

    #[test]
    fn store_uid_minting_reads_entropy_from_nondeterminism() {
        let mut first = FixedNondeterminism::new(0, 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let mut second = FixedNondeterminism::new(0, 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);

        let first_uid = mint_store_uid(&mut first).expect("mint first UID");
        let second_uid = mint_store_uid(&mut second).expect("mint second UID");

        assert_eq!(first_uid, second_uid);
        assert_eq!(first_uid.as_str(), "store_0102030405060708090a0b0c0d0e0f10");
    }

    #[test]
    fn store_uid_minting_returns_entropy_io_error() {
        let error = mint_store_uid(&mut FailingNondeterminism)
            .expect_err("entropy failure stops UID minting");

        assert!(matches!(error, BackupError::Io(_)));
    }

    #[test]
    fn ensure_store_uid_returns_entropy_io_error_without_stamping_store() {
        let store = TreeStore::memory();
        let error = ensure_store_uid(&store, &mut FailingNondeterminism)
            .expect_err("entropy failure stops store UID stamping");

        assert!(matches!(error, BackupError::Io(_)));
        assert!(
            store
                .read_store_uid()
                .expect("read missing store UID")
                .is_none()
        );
    }

    #[test]
    fn commit_descriptor_round_trips_the_slim_stamp() {
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
        };

        let descriptor = CommitDescriptor::from_metadata(&metadata);
        let restored = descriptor
            .to_metadata()
            .expect("descriptor restores metadata");

        assert_eq!(restored, metadata);
    }
}
