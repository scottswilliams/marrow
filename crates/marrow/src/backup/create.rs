//! Writing a backup: snapshot the store, then stream its canonical cell stream
//! behind a typed manifest.

use std::io::Write;

use marrow_check::CheckedProgram;
use marrow_run::evolution::current_engine_profile;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::archive::{self, CHECKSUM_SEED, checksum_cell};
use super::{BackupError, BackupManifest, CommitDescriptor, EngineDescriptor, FORMAT_VERSION};

/// What a completed backup wrote.
pub struct BackupReport {
    pub record_count: u64,
    pub catalog_epoch: Option<u64>,
}

/// Write a backup of `store` (read through one pinned snapshot) to `out`. The
/// manifest binds the data to `program`. The store is read twice under the same
/// snapshot — once to size and checksum the stream, once to write it — so the
/// whole backup streams in bounded memory and the manifest's checksum matches the
/// bytes that follow it.
pub fn create_backup(
    program: &CheckedProgram,
    store: &TreeStore,
    out: &mut impl Write,
) -> Result<BackupReport, BackupError> {
    let _snapshot = store.read_snapshot()?;

    let mut record_count = 0u64;
    let mut checksum = CHECKSUM_SEED;
    store.visit_backup_cells(|cell| {
        record_count += 1;
        checksum = checksum_cell(checksum, cell);
        Ok(())
    })?;

    let manifest = build_manifest(program, store, record_count, checksum)?;
    archive::write_header(out, &manifest)?;

    // The store traversal reports a `StoreError`, so a write failure is stashed and
    // surfaced as the `io.write` it really is rather than a store error.
    let mut write_error = None;
    let traversal = store.visit_backup_cells(|cell| {
        if let Err(error) = archive::write_cell(out, cell) {
            write_error = Some(error);
            return Err(StoreError::Io {
                op: "backup",
                message: "backup write failed".to_string(),
            });
        }
        Ok(())
    });
    if let Some(error) = write_error {
        return Err(BackupError::Io(error));
    }
    traversal?;
    out.flush()?;

    Ok(BackupReport {
        record_count,
        catalog_epoch: manifest.catalog_epoch,
    })
}

fn build_manifest(
    program: &CheckedProgram,
    store: &TreeStore,
    record_count: u64,
    data_checksum: u64,
) -> Result<BackupManifest, BackupError> {
    let engine = EngineDescriptor::recorded(
        &current_engine_profile(),
        store.read_layout_epoch()?,
        store.read_engine_profile_digest()?,
    );
    let commit = store
        .read_commit_metadata()?
        .as_ref()
        .map(CommitDescriptor::from_metadata);
    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        source_digest: program.source_digest().to_string(),
        catalog_epoch: store.read_catalog_epoch()?,
        engine,
        commit,
        record_count,
        data_checksum,
    };
    if let Some(commit) = &manifest.commit {
        commit.validate_manifest_binding(&manifest)?;
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use marrow_store::tree::{CommitMetadata, TreeStore};

    use super::super::test_support::{BOOK_SOURCE, committed_program};
    use super::super::{BackupCorruptProblem, BackupError};
    use super::build_manifest;

    #[test]
    fn backup_manifest_rejects_commit_metadata_that_disagrees_with_binding() {
        let (root, program) = committed_program("backup-commit-binding", BOOK_SOURCE);
        let store = TreeStore::memory();
        let epoch = program.catalog.accepted_epoch.expect("accepted epoch");
        store.write_catalog_epoch(epoch).expect("stamp epoch");
        let profile = marrow_run::evolution::current_engine_profile();
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 1,
                catalog_epoch: epoch,
                layout_epoch: profile.layout_epoch(),
                source_digest:
                    "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                        .to_string(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
                activation_evolution_digest: String::new(),
                activation_proposal_catalog_digest: None,
                activation_proposal_new_catalog_ids: Vec::new(),
                activation_records_backfilled: 0,
                activation_default_records_by_id: Vec::new(),
                activation_indexes_rebuilt: 0,
                activation_records_retired: 0,
                activation_retire_evidence_digest: String::new(),
                activation_records_retired_by_id: Vec::new(),
                activation_records_transformed: 0,
            })
            .expect("stamp commit");

        let error = build_manifest(&program, &store, 0, 0)
            .expect_err("backup must not emit a self-inconsistent manifest");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(error.code(), "restore.corrupt_chunk");
        assert!(matches!(
            error,
            BackupError::CorruptChunk {
                problem: BackupCorruptProblem::ManifestCommitBindingMismatch,
                ..
            }
        ));
    }
}
