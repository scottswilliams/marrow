//! Writing a backup: snapshot the store, then stream its catalog section and
//! canonical cell stream behind a typed manifest.

use std::io::Write;

use marrow_check::CheckedProgram;
use marrow_run::evolution::current_engine_profile;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::archive::{
    self, CHECKSUM_SEED, catalog_section_bytes, checksum_catalog_section, checksum_cell,
    checksum_manifest,
};
use super::{
    BackupCorruptProblem, BackupError, BackupManifest, CommitDescriptor, EngineDescriptor,
    FORMAT_VERSION,
};

/// What a completed backup wrote.
pub(crate) struct BackupReport {
    pub(crate) record_count: u64,
    pub(crate) catalog_epoch: Option<u64>,
}

/// Write a backup of `store` (read through one pinned snapshot) to `out`. The
/// manifest binds the data to `program` and carries the accepted-catalog rows in a
/// typed section, so a restored store is self-contained. The data cells are
/// streamed under the snapshot, so the whole backup runs in bounded memory; the
/// integrity checksum folds the manifest, the catalog section, and the data cells,
/// so any later tamper is rejected on restore.
pub(crate) fn create_backup(
    program: &CheckedProgram,
    store: &TreeStore,
    out: &mut impl Write,
) -> Result<BackupReport, BackupError> {
    let _snapshot = store.read_snapshot()?;

    let catalog = store.read_catalog_snapshot()?;
    let catalog_section = catalog_section_bytes(catalog.as_ref());

    let mut record_count = 0u64;
    store.visit_backup_cells(|_cell| {
        record_count += 1;
        Ok(())
    })?;

    let manifest = build_manifest(program, store, catalog.as_ref(), record_count)?;
    let checksum = checksum_archive(store, &manifest, &catalog_section)?;
    let manifest = BackupManifest {
        archive_checksum: checksum,
        ..manifest
    };

    archive::write_header(out, &manifest)?;
    archive::write_catalog_section(out, &catalog_section)?;
    write_data_cells(store, out)?;
    out.flush()?;

    Ok(BackupReport {
        record_count,
        catalog_epoch: manifest.catalog_epoch,
    })
}

/// Fold the manifest, the catalog section, and the data cells into one integrity
/// checksum in archive order, so the read side recomputes the same value over the
/// same three regions.
fn checksum_archive(
    store: &TreeStore,
    manifest: &BackupManifest,
    catalog_section: &[u8],
) -> Result<u64, BackupError> {
    let mut checksum = checksum_manifest(CHECKSUM_SEED, manifest);
    checksum = checksum_catalog_section(checksum, catalog_section);
    store.visit_backup_cells(|cell| {
        checksum = checksum_cell(checksum, cell);
        Ok(())
    })?;
    Ok(checksum)
}

fn write_data_cells(store: &TreeStore, out: &mut impl Write) -> Result<(), BackupError> {
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
    Ok(())
}

fn build_manifest(
    program: &CheckedProgram,
    store: &TreeStore,
    snapshot: Option<&marrow_catalog::CatalogMetadata>,
    record_count: u64,
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
        catalog_digest: snapshot.map(|snapshot| snapshot.digest.clone()),
        engine,
        commit,
        record_count,
        archive_checksum: 0,
    };
    if let Some(commit) = &manifest.commit {
        commit.validate_manifest_binding(&manifest)?;
    }
    validate_catalog_manifest_binding(&manifest)?;
    Ok(manifest)
}

/// The catalog section and the stamped catalog epoch describe one accepted catalog,
/// so a backup must carry both or neither, and their epochs must agree. A manifest
/// that records a catalog epoch without the rows (or the reverse) would let a restore
/// rebuild identity from less than the whole catalog, so it fails closed at write.
pub(crate) fn validate_catalog_manifest_binding(
    manifest: &BackupManifest,
) -> Result<(), BackupError> {
    let mismatch = |message: &'static str| {
        Err(BackupError::corrupt(
            BackupCorruptProblem::ManifestCatalogBindingMismatch,
            message,
        ))
    };
    match (&manifest.catalog_digest, manifest.catalog_epoch) {
        (Some(_), None) => mismatch("backup carries a catalog digest without a catalog epoch"),
        (None, Some(_)) => mismatch("backup carries a catalog epoch without a catalog digest"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use marrow_catalog::CatalogMetadata;
    use marrow_store::tree::{CommitMetadata, TreeStore};

    use super::super::test_support::{BOOK_SOURCE, committed_program};
    use super::super::{BackupCorruptProblem, BackupError};
    use super::build_manifest;

    #[test]
    fn backup_manifest_rejects_commit_metadata_that_disagrees_with_binding() {
        let (root, program) = committed_program("backup-commit-binding", BOOK_SOURCE);
        let store = TreeStore::memory();
        let epoch = program.catalog.accepted_epoch.expect("accepted epoch");
        let snapshot = CatalogMetadata::new(epoch, program.catalog.accepted_entries.clone());
        store.begin().expect("begin catalog stamp");
        store
            .replace_catalog_snapshot(&snapshot)
            .expect("write accepted catalog snapshot");
        store.write_catalog_epoch(epoch).expect("stamp epoch");
        store.commit().expect("commit catalog stamp");
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

        let error = build_manifest(&program, &store, Some(&snapshot), 0)
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
