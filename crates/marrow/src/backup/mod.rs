//! Typed portable backup of a project's saved data.
//!
//! A backup is a Marrow artifact, not a raw engine-file copy: a small header, a
//! typed manifest, and the canonical ordered tree-cell stream. The manifest binds
//! the data to the program that wrote it — its source digest, accepted catalog
//! epoch, engine profile, value-codec version, and a checksum over the cell
//! stream — so a restore refuses data it cannot faithfully reproduce. The cell
//! stream itself is backend-independent (tree-cell keys derive from catalog stable
//! IDs), so equal data yields a byte-identical backup that restores into any
//! conforming backend at the same layout and codec.
//!
//! [`create`] writes a backup over a stable read snapshot; [`restore`] validates a
//! backup against the project and replays it into an empty store in one
//! transaction. The on-disk framing and the manifest live here; the two
//! operations live in their own modules.

mod archive;
pub mod create;
pub mod restore;

pub use create::create_backup;
pub use restore::restore_backup;

use marrow_store::tree::{CommitMetadata, EngineProfile, EngineProfileDigest};

/// The on-disk format version. It advances only on an incompatible change to the
/// header, manifest, or cell framing.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// A short name identifying the engine family a backup was taken from. v0.1 has
/// one; the layout, key-profile, and value-codec versions distinguish revisions.
pub(crate) const ENGINE_NAME: &str = "marrow-tree-cell";

/// The typed header binding a backup's data to the program and engine that wrote
/// it. Restore validates every field before replaying a single cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupManifest {
    pub format_version: u32,
    /// The schema-bearing source digest of the program that wrote the data.
    pub source_digest: String,
    /// The store's stamped catalog epoch, or `None` for an unstamped store.
    pub catalog_epoch: Option<u64>,
    /// The engine profile the data was written under, so restore refuses a layout
    /// or codec it cannot reproduce (reporting an engine recompile is required).
    pub engine: EngineDescriptor,
    /// The commit the snapshot observed, replayed so the restored store fences
    /// exactly like the original. `None` for an unstamped store.
    pub commit: Option<CommitDescriptor>,
    /// How many tree cells the data stream carries.
    pub record_count: u64,
    /// A checksum over the canonical cell stream, so a corrupt chunk is rejected.
    pub data_checksum: u64,
}

/// The engine identity a restore must match to replay bytes verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDescriptor {
    pub name: String,
    pub layout_epoch: u64,
    pub key_profile_version: u8,
    pub value_codec_version: u32,
    pub profile_digest: EngineProfileDigest,
}

impl EngineDescriptor {
    /// The engine identity the running binary writes and restores into.
    pub(crate) fn current(profile: &EngineProfile) -> Self {
        Self {
            name: ENGINE_NAME.to_string(),
            layout_epoch: profile.layout_epoch(),
            key_profile_version: profile.key_profile_version(),
            value_codec_version: marrow_store::value::VALUE_CODEC_VERSION,
            profile_digest: profile.digest_bytes(),
        }
    }
}

/// The commit metadata a backup records and a restore restamps, mirroring
/// [`CommitMetadata`] with catalog ids carried as their opaque text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDescriptor {
    pub commit_id: u64,
    pub catalog_epoch: u64,
    pub layout_epoch: u64,
    pub source_digest: String,
    pub engine_profile_digest: EngineProfileDigest,
    pub changed_root_catalog_ids: Vec<String>,
    pub changed_index_catalog_ids: Vec<String>,
}

impl CommitDescriptor {
    pub(crate) fn from_metadata(metadata: &CommitMetadata) -> Self {
        Self {
            commit_id: metadata.commit_id,
            catalog_epoch: metadata.catalog_epoch,
            layout_epoch: metadata.layout_epoch,
            source_digest: metadata.source_digest.clone(),
            engine_profile_digest: metadata.engine_profile_digest,
            changed_root_catalog_ids: metadata
                .changed_root_catalog_ids
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            changed_index_catalog_ids: metadata
                .changed_index_catalog_ids
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
        }
    }

    /// Rebuild the engine-facing commit metadata, rejecting a malformed catalog id
    /// as a corrupt manifest.
    pub(crate) fn to_metadata(&self) -> Result<CommitMetadata, BackupError> {
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

fn catalog_ids(ids: &[String]) -> Result<Vec<marrow_store::cell::CatalogId>, BackupError> {
    ids.iter()
        .map(|id| {
            marrow_store::cell::CatalogId::new(id.clone())
                .map_err(|_| BackupError::corrupt("manifest carries a malformed catalog id"))
        })
        .collect()
}

/// A backup or restore failure, carrying a stable dotted code for tools.
#[derive(Debug)]
pub enum BackupError {
    /// The backup file could not be read or written.
    Io(std::io::Error),
    /// A store read or write failed.
    Store(marrow_store::StoreError),
    /// The backup header or manifest is not a Marrow backup this build understands.
    FormatVersion(String),
    /// A cell chunk failed its checksum or framing.
    CorruptChunk(String),
    /// The restore target already holds saved data.
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

impl BackupError {
    fn corrupt(message: &str) -> Self {
        Self::CorruptChunk(message.to_string())
    }

    /// The stable dotted code a tool reports for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io.write",
            Self::Store(error) => error.code(),
            Self::FormatVersion(_) => "restore.format_version",
            Self::CorruptChunk(_) => "restore.corrupt_chunk",
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
            Self::FormatVersion(message)
            | Self::CorruptChunk(message)
            | Self::EngineRecompileRequired(message)
            | Self::SourceMismatch(message)
            | Self::CatalogMismatch(message)
            | Self::DataInvalid(message) => write!(f, "{message}"),
            Self::NotEmpty => write!(
                f,
                "the restore target already holds saved data; restore writes into an empty store"
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
