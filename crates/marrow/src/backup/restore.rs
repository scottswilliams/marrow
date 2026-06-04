//! Restoring a backup: validate it against the project and engine, then replay
//! its cells into an empty store in one transaction so the target either gains
//! the whole backup or is left unchanged.

use std::io::Read;

use marrow_check::CheckedProgram;
use marrow_run::evolution::current_engine_profile;
use marrow_store::tree::TreeStore;

use super::archive::{self, CHECKSUM_SEED, checksum_cell};
use super::{BackupError, BackupManifest, EngineDescriptor};

/// What a completed restore replayed.
pub struct RestoreReport {
    pub record_count: u64,
    pub catalog_epoch: Option<u64>,
}

/// Restore the backup in `input` into `store`, an empty native store for
/// `program`. The whole replay runs in one transaction: a checksum mismatch, a
/// short stream, or a `verify` failure rolls the target back to empty. `verify`
/// proves the restored data compiles against the project schema before the
/// transaction commits.
pub fn restore_backup(
    program: &CheckedProgram,
    store: &TreeStore,
    input: &mut impl Read,
    verify: impl Fn(&TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let manifest = archive::read_header(input)?;
    validate_against_project(program, &manifest)?;
    if !store.is_empty()? {
        return Err(BackupError::NotEmpty);
    }

    store.begin()?;
    match replay(store, &manifest, input, &verify) {
        Ok(report) => {
            store.commit()?;
            Ok(report)
        }
        Err(error) => {
            // Leave the target exactly as it was found: empty.
            let _ = store.rollback();
            Err(error)
        }
    }
}

/// Refuse a backup the running binary cannot faithfully reproduce: a different
/// engine, layout, or value codec needs a recompile; a different schema or
/// catalog epoch belongs to a different program state.
fn validate_against_project(
    program: &CheckedProgram,
    manifest: &BackupManifest,
) -> Result<(), BackupError> {
    let current = EngineDescriptor::current(&current_engine_profile());
    if manifest.engine.layout_epoch != current.layout_epoch
        || manifest.engine.key_profile_version != current.key_profile_version
        || manifest.engine.value_codec_version != current.value_codec_version
        || manifest.engine.profile_digest != current.profile_digest
    {
        return Err(BackupError::EngineRecompileRequired(
            "backup was written under a different engine, layout, or value codec; \
             a cross-engine restore is a future engine recompile"
                .to_string(),
        ));
    }
    if manifest.source_digest != program.source_digest() {
        return Err(BackupError::SourceMismatch(
            "backup was written from a program whose schema does not match this project"
                .to_string(),
        ));
    }
    // A backup carries the catalog epoch its data was committed at. An empty store
    // has none, so only a backup that actually carries committed data binds an
    // epoch the project's accepted catalog must match.
    if let Some(backup_epoch) = manifest.catalog_epoch
        && Some(backup_epoch) != program.catalog.accepted_epoch
    {
        return Err(BackupError::CatalogMismatch(
            "backup catalog epoch does not match this project's accepted catalog".to_string(),
        ));
    }
    Ok(())
}

fn replay(
    store: &TreeStore,
    manifest: &BackupManifest,
    input: &mut impl Read,
    verify: &impl Fn(&TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let mut checksum = CHECKSUM_SEED;
    for _ in 0..manifest.record_count {
        let (key, value) = archive::read_cell(input)?;
        checksum = checksum_cell(checksum, &key, &value);
        store.restore_cell(&key, value)?;
    }
    if checksum != manifest.data_checksum {
        return Err(BackupError::CorruptChunk(
            "backup data checksum does not match its manifest".to_string(),
        ));
    }

    // Stamp the durable identity the data was written under, so the restored store
    // fences exactly as the original did. The engine profile is the current
    // build's, already proven equal to the manifest's.
    store.write_engine_profile(&current_engine_profile())?;
    if let Some(epoch) = manifest.catalog_epoch {
        store.write_catalog_epoch(epoch)?;
    }
    if let Some(commit) = &manifest.commit {
        store.write_commit_metadata(&commit.to_metadata()?)?;
    }

    // Reads inside the open transaction see the staged data, so verification runs
    // before commit and a failure rolls the whole restore back.
    verify(store)?;

    Ok(RestoreReport {
        record_count: manifest.record_count,
        catalog_epoch: manifest.catalog_epoch,
    })
}
