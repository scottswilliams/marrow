//! Writing a backup: snapshot the store, then stream its catalog section and
//! canonical cell stream behind a typed manifest.

use std::io::Write;

use marrow_check::CheckedProgram;
use marrow_project::Sha256Digest;
use marrow_run::evolution::current_engine_profile;
use marrow_store::StoreError;
use marrow_store::tree::{StoreUid, TreeStore};

use super::archive::{
    self, CHECKSUM_SEED, catalog_section_bytes, checksum_catalog_section, checksum_cell,
    checksum_manifest,
};
use super::{
    BackupCorruptProblem, BackupError, BackupManifest, CommitDescriptor, EngineDescriptor,
    FORMAT_VERSION, require_store_uid,
};

/// What a completed backup wrote.
pub(crate) struct BackupReport {
    pub(crate) record_count: u64,
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
    let store_uid = require_store_uid(store)?;
    let _snapshot = store.read_snapshot()?;

    let catalog = store.read_catalog_snapshot()?;
    let catalog_section = catalog_section_bytes(catalog.as_ref())?;

    let (record_count, state_digest) = scan_state(store)?;

    let manifest = build_manifest(
        program,
        store,
        catalog.as_ref(),
        record_count,
        state_digest,
        &store_uid,
    )?;
    let checksum = checksum_archive(store, &manifest, &catalog_section)?;
    let manifest = BackupManifest {
        archive_checksum: checksum,
        ..manifest
    };

    archive::write_header(out, &manifest)?;
    archive::write_catalog_section(out, &catalog_section)?;
    write_data_cells(store, out)?;
    out.flush()?;

    Ok(BackupReport { record_count })
}

fn scan_state(store: &TreeStore) -> Result<(u64, String), BackupError> {
    let mut record_count = 0u64;
    let mut digest = Sha256Digest::new();
    store.visit_backup_cells(|cell| {
        record_count += 1;
        cell.write_framed(&mut DigestSink(&mut digest))
            .expect("digest sink is infallible");
        Ok(())
    })?;
    Ok((record_count, digest.finish()))
}

struct DigestSink<'a>(&'a mut Sha256Digest);

impl Write for DigestSink<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
    state_digest: String,
    store_uid: &StoreUid,
) -> Result<BackupManifest, BackupError> {
    let commit_metadata = store.read_commit_metadata()?;
    validate_catalog_snapshot_commit_binding(snapshot, commit_metadata.as_ref())?;
    let engine = EngineDescriptor::recorded(&current_engine_profile(), commit_metadata.as_ref());
    let commit = commit_metadata
        .as_ref()
        .map(CommitDescriptor::from_metadata);
    let source_digest = commit_metadata.as_ref().map_or_else(
        || program.source_digest().to_string(),
        |commit| commit.source_digest.clone(),
    );
    let catalog_epoch = commit_metadata
        .as_ref()
        .map(|commit| commit.catalog_epoch)
        .or_else(|| snapshot.map(|snapshot| snapshot.epoch));
    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        source_digest,
        catalog_epoch,
        catalog_digest: snapshot.map(|snapshot| snapshot.digest.clone()),
        state_digest,
        store_uid: store_uid.as_str().to_string(),
        parent_snapshot_digest: None,
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

fn validate_catalog_snapshot_commit_binding(
    snapshot: Option<&marrow_catalog::CatalogMetadata>,
    commit: Option<&marrow_store::tree::CommitMetadata>,
) -> Result<(), BackupError> {
    let (Some(snapshot), Some(commit)) = (snapshot, commit) else {
        return Ok(());
    };
    if snapshot.epoch != commit.catalog_epoch {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::ManifestCatalogBindingMismatch,
            "backup catalog section epoch disagrees with commit metadata",
        ));
    }
    Ok(())
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
    use marrow_run::FixedNondeterminism;
    use marrow_store::tree::{CommitMetadata, EngineProfile, TreeStore};

    use super::super::ensure_store_uid;
    use super::super::test_support::{BOOK_SOURCE, committed_program};
    use super::super::{BackupCorruptProblem, BackupError};
    use super::{build_manifest, scan_state};

    #[test]
    fn backup_manifest_uses_commit_metadata_as_stamp() {
        let (root, program) = committed_program("backup-commit-only-stamp", BOOK_SOURCE);
        let store = TreeStore::memory();
        let epoch = program.catalog.accepted_epoch.expect("accepted epoch");
        let snapshot = CatalogMetadata::new(epoch, program.catalog.accepted_entries.clone())
            .expect("catalog builds");
        store.begin().expect("begin catalog publish");
        store
            .replace_catalog_snapshot(&snapshot)
            .expect("write accepted catalog snapshot");
        store.commit().expect("commit catalog publish");
        let recorded_profile =
            EngineProfile::new(marrow_run::evolution::current_engine_profile().layout_epoch() + 1);
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 1,
                catalog_epoch: epoch,
                layout_epoch: recorded_profile.layout_epoch(),
                source_digest: program.source_digest().to_string(),
                engine_profile_digest: recorded_profile.digest_bytes(),
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
            })
            .expect("stamp commit");

        let (record_count, state_digest) = scan_state(&store).expect("scan state");
        let mut nondeterminism =
            FixedNondeterminism::new(0, 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let store_uid = ensure_store_uid(&store, &mut nondeterminism).expect("store uid");
        let manifest = build_manifest(
            &program,
            &store,
            Some(&snapshot),
            record_count,
            state_digest,
            &store_uid,
        )
        .expect("build manifest");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(manifest.catalog_epoch, Some(epoch));
        assert_eq!(
            manifest.engine.layout_epoch,
            recorded_profile.layout_epoch()
        );
        assert_eq!(
            manifest.engine.profile_digest,
            recorded_profile.digest_bytes()
        );
        assert_eq!(
            manifest.commit.as_ref().map(|commit| commit.catalog_epoch),
            Some(epoch)
        );
        assert_eq!(manifest.store_uid, store_uid.as_str());
        assert!(manifest.parent_snapshot_digest.is_none());
    }

    #[test]
    fn backup_manifest_rejects_commit_epoch_that_disagrees_with_catalog_snapshot() {
        let (root, program) = committed_program("backup-commit-snapshot-drift", BOOK_SOURCE);
        let store = TreeStore::memory();
        let epoch = program.catalog.accepted_epoch.expect("accepted epoch");
        let snapshot = CatalogMetadata::new(epoch, program.catalog.accepted_entries.clone())
            .expect("catalog builds");
        store
            .replace_catalog_snapshot(&snapshot)
            .expect("write accepted catalog snapshot");
        let profile = marrow_run::evolution::current_engine_profile();
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 1,
                catalog_epoch: epoch + 1,
                layout_epoch: profile.layout_epoch(),
                source_digest: program.source_digest().to_string(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
            })
            .expect("stamp commit");

        let (record_count, state_digest) = scan_state(&store).expect("scan state");
        let mut nondeterminism =
            FixedNondeterminism::new(0, 0x1112_1314_1516_1718_191a_1b1c_1d1e_1f20);
        let store_uid = ensure_store_uid(&store, &mut nondeterminism).expect("store uid");
        let error = build_manifest(
            &program,
            &store,
            Some(&snapshot),
            record_count,
            state_digest,
            &store_uid,
        )
        .expect_err("commit and snapshot epoch mismatch is rejected");
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(error.code(), "restore.corrupt_chunk");
        assert!(matches!(
            error,
            BackupError::CorruptChunk {
                problem: BackupCorruptProblem::ManifestCatalogBindingMismatch,
                ..
            }
        ));
    }
}
