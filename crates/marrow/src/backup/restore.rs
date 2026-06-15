//! Restoring a backup: validate it against the project and engine, then replay
//! its catalog rows and data cells into an empty target by default, or into a
//! counted replace target after clearing it inside the restore transaction. A
//! failure rolls back to the target's prior state. The restored catalog
//! re-establishes accepted identity, so a restored store runs immediately without
//! re-running evolution.

use std::io::Read;

use marrow_check::CheckedProgram;
use marrow_project::Sha256Digest;
use marrow_run::Nondeterminism;
use marrow_run::evolution::{ApplyError, current_engine_profile, rebuild_store_indexes};
use marrow_store::cell::DataCellKind;
use marrow_store::tree::{TreeBackupCellBuf, TreeStore};

use super::archive::{
    self, CHECKSUM_SEED, CatalogSection, checksum_catalog_section, checksum_cell, checksum_manifest,
};
use super::create::validate_catalog_manifest_binding;
use super::{
    BackupCorruptProblem, BackupError, BackupManifest, CatalogFingerprintRef, EngineDescriptor,
    mint_store_uid,
};

/// What a completed restore replayed.
#[derive(Debug)]
pub(crate) struct RestoreReport {
    pub(crate) record_count: u64,
    pub(crate) receipt: RestoreReceipt,
}

/// Which live target state restore is allowed to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreTargetMode {
    EmptyOnly,
    Replace { expected_live_records: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreReceipt {
    EmptyOnly,
    Replace {
        expected_live_records: u64,
        replaced_live_records: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedRestoreTarget {
    EmptyOnly,
    Replace {
        expected_live_records: u64,
        replaced_live_records: u64,
    },
}

/// The validated backup prologue: the manifest and the decoded catalog section. The
/// catalog rows it carries are the accepted identity a caller re-checks the project
/// against before replaying, so a restore binds the backup's own catalog rather
/// than a freshly proposed baseline.
pub(crate) struct BackupPrologue {
    manifest: BackupManifest,
    section: CatalogSection,
}

impl BackupPrologue {
    /// The accepted catalog the backup carries, the snapshot a restore re-checks the
    /// project against.
    pub(crate) fn catalog(&self) -> Option<&marrow_catalog::CatalogMetadata> {
        self.section.snapshot.as_ref()
    }

    pub(crate) fn source_digest(&self) -> &str {
        &self.manifest.source_digest
    }
}

/// Read and validate the backup header and catalog section from `input`, leaving the
/// reader positioned at the data-cell stream. The catalog section fails closed here if
/// a row was tampered or its fingerprint disagrees with the manifest, before any data
/// is replayed.
pub(crate) fn read_backup_prologue(input: &mut impl Read) -> Result<BackupPrologue, BackupError> {
    let manifest = archive::read_header(input)?;
    let section = archive::read_catalog_section(input)?;
    validate_catalog_section(&manifest, &section)?;
    Ok(BackupPrologue { manifest, section })
}

/// Validate a backup archive without replaying it into a store. This is the same
/// manifest, catalog-section, state-digest, archive-checksum, and trailing-byte
/// contract restore enforces before commit, used by writers that need proof the
/// artifact is readable before continuing with another mutation.
pub(crate) fn validate_backup_archive(input: &mut impl Read) -> Result<(), BackupError> {
    let prologue = read_backup_prologue(input)?;
    let BackupPrologue { manifest, section } = prologue;
    validate_archive_stream(&manifest, &section, input)
}

/// Restore the backup whose `prologue` was already read from `input` into `store`.
/// The target must be empty unless counted replace mode was requested. `program`
/// must be bound to the backup's catalog (see [`BackupPrologue::catalog`]). The
/// whole replay runs in one transaction: a checksum mismatch, a short stream, or a
/// `verify` failure rolls the target back to its prior state. The catalog rows
/// replay alongside the data, so the restored store carries its own accepted
/// identity. `verify` proves the restored data compiles against the restored
/// catalog before the transaction commits.
pub(crate) fn restore_backup_with_prologue(
    program: &CheckedProgram,
    store: &TreeStore,
    prologue: BackupPrologue,
    input: &mut impl Read,
    target_mode: RestoreTargetMode,
    nondeterminism: &mut impl Nondeterminism,
    verify: impl Fn(&CheckedProgram, &TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let BackupPrologue { manifest, section } = prologue;
    let restore_program = restore_program(program, &manifest)?;
    let target = prepare_restore_target(&restore_program, store, target_mode)?;

    store.begin()?;
    let replay_result = (|| {
        if matches!(target, PreparedRestoreTarget::Replace { .. }) {
            store.clear_restore_target()?;
        }
        replay(
            &restore_program,
            store,
            &manifest,
            &section,
            input,
            nondeterminism,
            &verify,
        )
    })();
    match replay_result {
        Ok(mut report) => {
            report.receipt = target.receipt();
            store.commit()?;
            Ok(report)
        }
        Err(error) => {
            let _ = store.rollback();
            Err(error)
        }
    }
}

pub(crate) fn mount_backup_for_evolution_preview(
    program: &CheckedProgram,
    prologue: BackupPrologue,
    input: &mut impl Read,
    nondeterminism: &mut impl Nondeterminism,
) -> Result<TreeStore, BackupError> {
    let BackupPrologue { manifest, section } = prologue;
    let preview_program = restore_program_for_evolution_preview(program, &manifest)?;
    let store = TreeStore::memory();
    store.begin()?;
    let replay_result = replay(
        &preview_program,
        &store,
        &manifest,
        &section,
        input,
        nondeterminism,
        &verify_evolution_preview_mount,
    );
    match replay_result {
        Ok(_) => {
            store.commit()?;
            Ok(store)
        }
        Err(error) => {
            let _ = store.rollback();
            Err(error)
        }
    }
}

fn prepare_restore_target(
    program: &CheckedProgram,
    store: &TreeStore,
    target_mode: RestoreTargetMode,
) -> Result<PreparedRestoreTarget, BackupError> {
    match target_mode {
        RestoreTargetMode::EmptyOnly => {
            if restore_target_is_empty(store)? {
                Ok(PreparedRestoreTarget::EmptyOnly)
            } else {
                Err(BackupError::target_not_empty())
            }
        }
        RestoreTargetMode::Replace {
            expected_live_records,
        } => prepare_replace_target(program, store, expected_live_records),
    }
}

fn prepare_replace_target(
    program: &CheckedProgram,
    store: &TreeStore,
    expected_live_records: u64,
) -> Result<PreparedRestoreTarget, BackupError> {
    let (_, problems) = marrow_check::tooling::count_activation_integrity_problems(store, program)?;
    if problems != 0 {
        return Err(BackupError::DataInvalid(format!(
            "replace target has {problems} data integrity problem(s); run `marrow data integrity` before restore --replace"
        )));
    }
    let replaced_live_records = count_restore_target_records(store)?;
    if replaced_live_records != expected_live_records {
        return Err(BackupError::replace_count_mismatch(
            expected_live_records,
            replaced_live_records,
        ));
    }
    Ok(PreparedRestoreTarget::Replace {
        expected_live_records,
        replaced_live_records,
    })
}

fn count_restore_target_records(store: &TreeStore) -> Result<u64, BackupError> {
    let mut records = 0u64;
    store.visit_backup_cells(|_| {
        records = records
            .checked_add(1)
            .ok_or(marrow_store::StoreError::LimitExceeded {
                limit: "restore replace record count",
            })?;
        Ok(())
    })?;
    Ok(records)
}

impl PreparedRestoreTarget {
    fn receipt(self) -> RestoreReceipt {
        match self {
            PreparedRestoreTarget::EmptyOnly => RestoreReceipt::EmptyOnly,
            PreparedRestoreTarget::Replace {
                expected_live_records,
                replaced_live_records,
            } => RestoreReceipt::Replace {
                expected_live_records,
                replaced_live_records,
            },
        }
    }
}

fn restore_target_is_empty(store: &TreeStore) -> Result<bool, BackupError> {
    Ok(store.is_empty()? && store.read_catalog_snapshot()?.is_none())
}

/// Restore the backup in `input` into `store`, an empty native store already bound to
/// the backup's catalog through `program`. Reads the prologue and replays in one step;
/// the CLI splits the two so it can re-check the project against the carried catalog.
#[cfg(test)]
pub(crate) fn restore_backup(
    program: &CheckedProgram,
    store: &TreeStore,
    input: &mut impl Read,
    verify: impl Fn(&CheckedProgram, &TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let prologue = read_backup_prologue(input)?;
    let mut nondeterminism =
        marrow_run::FixedNondeterminism::new(0, 0xfedc_ba98_7654_3210_f0e1_d2c3_b4a5_9687);
    restore_backup_with_prologue(
        program,
        store,
        prologue,
        input,
        RestoreTargetMode::EmptyOnly,
        &mut nondeterminism,
        verify,
    )
}

/// The catalog section's recomputed digest and row count must equal the manifest's
/// fingerprint: the section already failed closed if its stored digest disagreed
/// with its rows, so this also catches a manifest whose fingerprint was tampered to
/// claim a catalog the section does not carry.
fn validate_catalog_section(
    manifest: &BackupManifest,
    section: &CatalogSection,
) -> Result<(), BackupError> {
    validate_catalog_manifest_binding(manifest)?;
    let section_epoch = section.snapshot.as_ref().map(|snapshot| snapshot.epoch);
    if section_epoch != manifest.catalog_epoch {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::ManifestCatalogBindingMismatch,
            "backup catalog section epoch does not match its manifest",
        ));
    }
    let section_digest = section.snapshot.as_ref().map(|snapshot| &snapshot.digest);
    if section_digest != manifest.catalog_digest.as_ref() {
        return Err(catalog_digest_mismatch());
    }
    Ok(())
}

fn catalog_digest_mismatch() -> BackupError {
    BackupError::corrupt(
        BackupCorruptProblem::CatalogDigestMismatch,
        "backup catalog section does not match its manifest fingerprint",
    )
}

/// Refuse a backup outside this binary's checked replay contract: a different
/// engine, layout, or value codec needs a recompile; a different schema or
/// catalog identity belongs to a different program state.
fn restore_program(
    program: &CheckedProgram,
    manifest: &BackupManifest,
) -> Result<CheckedProgram, BackupError> {
    validate_engine_and_commit(manifest)?;
    let project_source_digest = program.source_digest();
    if manifest.source_digest != project_source_digest {
        return Err(BackupError::source_mismatch(
            &manifest.source_digest,
            project_source_digest.as_str(),
        ));
    }
    validate_catalog_fingerprint(program, manifest)?;
    Ok(program.clone())
}

fn restore_program_for_evolution_preview(
    program: &CheckedProgram,
    manifest: &BackupManifest,
) -> Result<CheckedProgram, BackupError> {
    validate_engine_and_commit(manifest)?;
    validate_catalog_fingerprint(program, manifest)?;
    Ok(program.clone())
}

fn validate_engine_and_commit(manifest: &BackupManifest) -> Result<(), BackupError> {
    let current = EngineDescriptor::current(&current_engine_profile());
    if manifest.engine != current {
        return Err(BackupError::EngineRecompileRequired(
            "backup was written under a different engine, layout, or value codec; \
             a cross-engine restore is a future engine recompile"
                .to_string(),
        ));
    }
    if let Some(commit) = &manifest.commit {
        commit.validate_manifest_binding(manifest)?;
    }
    Ok(())
}

fn validate_catalog_fingerprint(
    program: &CheckedProgram,
    manifest: &BackupManifest,
) -> Result<(), BackupError> {
    let backup_catalog = CatalogFingerprintRef::from_parts(
        manifest.catalog_epoch,
        manifest.catalog_digest.as_deref(),
    );
    let project_catalog = CatalogFingerprintRef::from_parts(
        program.catalog.accepted_epoch,
        program.catalog.accepted_digest.as_deref(),
    );
    if backup_catalog != project_catalog {
        return Err(BackupError::catalog_mismatch(
            backup_catalog,
            project_catalog,
        ));
    }
    Ok(())
}

fn replay(
    program: &CheckedProgram,
    store: &TreeStore,
    manifest: &BackupManifest,
    section: &CatalogSection,
    input: &mut impl Read,
    nondeterminism: &mut impl Nondeterminism,
    verify: &impl Fn(&CheckedProgram, &TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let uid = mint_store_uid(nondeterminism)?;
    store.write_store_uid(&uid)?;
    if let Some(snapshot) = &section.snapshot {
        store.replace_catalog_snapshot(snapshot)?;
    }

    let mut checksum = checksum_manifest(CHECKSUM_SEED, manifest)?;
    checksum = checksum_catalog_section(checksum, &section.bytes);
    let mut state_digest = Sha256Digest::new();
    for _ in 0..manifest.record_count {
        let cell = archive::read_cell(input)?;
        checksum = checksum_cell(checksum, cell.as_ref())?;
        cell.visit_framed_bytes(|bytes| state_digest.update(bytes))
            .map_err(BackupError::cell_frame_too_large)?;
        restore_cell(store, &cell)?;
    }
    validate_stream_integrity(manifest, state_digest.finish(), checksum, input)?;

    // Indexes are derived, so rebuild them from the replayed records rather than
    // trusting bytes that could disagree. The rebuild runs inside this open
    // transaction, so the commit makes indexes durable atomically with the catalog
    // rows and data.
    rebuild_store_indexes(program, store).map_err(rebuild_error)?;

    // Stamp the durable identity the data was written under, so the restored store
    // fences exactly as the original did.
    if let Some(commit) = &manifest.commit {
        store.write_commit_metadata(&commit.to_metadata()?)?;
    }

    // Reads inside the open transaction see the staged catalog and data, so
    // verification runs against the restored catalog before commit and a failure
    // rolls the whole restore back.
    verify(program, store)?;

    Ok(RestoreReport {
        record_count: manifest.record_count,
        receipt: RestoreReceipt::EmptyOnly,
    })
}

fn validate_archive_stream(
    manifest: &BackupManifest,
    section: &CatalogSection,
    input: &mut impl Read,
) -> Result<(), BackupError> {
    let mut checksum = checksum_manifest(CHECKSUM_SEED, manifest)?;
    checksum = checksum_catalog_section(checksum, &section.bytes);
    let mut state_digest = Sha256Digest::new();
    for _ in 0..manifest.record_count {
        let cell = archive::read_cell(input)?;
        checksum = checksum_cell(checksum, cell.as_ref())?;
        cell.visit_framed_bytes(|bytes| state_digest.update(bytes))
            .map_err(BackupError::cell_frame_too_large)?;
    }
    validate_stream_integrity(manifest, state_digest.finish(), checksum, input)
}

fn validate_stream_integrity(
    manifest: &BackupManifest,
    state_digest: String,
    checksum: u64,
    input: &mut impl Read,
) -> Result<(), BackupError> {
    if state_digest != manifest.state_digest {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::StateDigestMismatch,
            "backup state digest does not match its data stream".to_string(),
        ));
    }
    if checksum != manifest.archive_checksum {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::ChecksumMismatch,
            "backup integrity checksum does not match its manifest".to_string(),
        ));
    }
    if has_trailing_bytes(input)? {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::TrailingBytes,
            "backup carries trailing bytes after its cell stream".to_string(),
        ));
    }
    Ok(())
}

fn restore_cell(store: &TreeStore, cell: &TreeBackupCellBuf) -> Result<(), BackupError> {
    let target = cell.data_key();
    match &target.kind {
        DataCellKind::Node => store.write_node(&target.store, &target.identity)?,
        DataCellKind::PathNode { path } => {
            store.write_data_node(&target.store, &target.identity, path)?
        }
        DataCellKind::Leaf { member } => store.write_leaf(
            &target.store,
            &target.identity,
            member,
            cell.value().to_vec(),
        )?,
        DataCellKind::Sequence { member, position } => store.write_sequence_position(
            &target.store,
            &target.identity,
            member,
            *position,
            cell.value().to_vec(),
        )?,
        DataCellKind::Value { path } => {
            store.write_data_value(&target.store, &target.identity, path, cell.value().to_vec())?
        }
    }
    Ok(())
}

/// A well-formed backup ends exactly at the last cell, so one readable byte means
/// the file is not the backup the manifest describes.
fn has_trailing_bytes(input: &mut impl Read) -> Result<bool, BackupError> {
    let mut byte = [0u8; 1];
    Ok(input.read(&mut byte)? != 0)
}

fn verify_evolution_preview_mount(
    restore_program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), BackupError> {
    match marrow_check::tooling::count_integrity_problems(store, restore_program) {
        Ok((_, 0)) => Ok(()),
        Ok((_, problems)) => Err(BackupError::DataInvalid(format!(
            "backup data has {problems} schema problem(s); the backup does not match this project"
        ))),
        Err(error) => Err(BackupError::Store(error)),
    }
}

/// An index rebuild over restored data fails only on a store fault: a malformed catalog
/// id or a backend write error. It is the store reporting, surfaced under its own code.
fn rebuild_error(error: ApplyError) -> BackupError {
    match error {
        ApplyError::Store(store) => BackupError::Store(store),
        other => BackupError::Store(marrow_store::StoreError::Corruption {
            message: format!("index rebuild on restore failed: {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use marrow_catalog::CatalogMetadata;
    use marrow_check::CheckedProgram;
    use marrow_run::{FixedNondeterminism, Nondeterminism};
    use marrow_store::cell::CatalogId;
    use marrow_store::key::{SavedKey, encode_identity_payload};
    use marrow_store::tree::{CommitMetadata, DataPathSegment, TreeStore};

    use super::super::test_support::{BOOK_SOURCE, committed_program};
    use super::super::{BackupCorruptProblem, BackupFormatProblem, archive};
    use super::{
        BackupError, CHECKSUM_SEED, RestoreReport, RestoreTargetMode, read_backup_prologue,
        restore_backup, restore_backup_with_prologue,
    };
    use crate::backup::{create_backup, ensure_store_uid};

    struct FailingNondeterminism;

    impl Nondeterminism for FailingNondeterminism {
        fn now_nanos(&self) -> i128 {
            0
        }

        fn entropy_u128(&mut self) -> io::Result<u128> {
            Err(io::Error::other("entropy unavailable"))
        }
    }

    /// Restore that verifies nothing: the restore.* codes under test fail in
    /// validation or replay, before a schema check would run.
    fn accept(_program: &CheckedProgram, _store: &TreeStore) -> Result<(), BackupError> {
        Ok(())
    }

    /// The accepted catalog snapshot the program's checked baseline carries, the rows a
    /// faithful backup writes into its catalog section.
    fn accepted_catalog(program: &CheckedProgram) -> CatalogMetadata {
        CatalogMetadata::new(
            program.catalog.accepted_epoch.expect("accepted epoch"),
            program.catalog.accepted_entries.clone(),
        )
        .expect("catalog builds")
    }

    /// Seed one book through the managed tree-cell write path, write the accepted
    /// catalog rows, then build a valid in-memory backup of the store under `program`.
    fn seeded_backup(program: &CheckedProgram) -> Vec<u8> {
        let store = TreeStore::memory();
        let place = marrow_check::checked_saved_root_place(
            program,
            "books",
            marrow_syntax::SourceSpan::default(),
        )
        .expect("checked saved place");
        let store_id = CatalogId::new(place.store_catalog_id.clone().expect("accepted store id"))
            .expect("store id");
        let title = place
            .root_members
            .iter()
            .find(|member| member.name == "title")
            .map(|member| {
                CatalogId::new(member.catalog_id.clone().expect("accepted title id"))
                    .expect("title id")
            })
            .expect("title member");
        store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(title)],
                marrow_store::value::encode_value(&marrow_store::value::SavedValue::Str(
                    "Mort".into(),
                ))
                .expect("encode title"),
            )
            .expect("seed title");
        let snapshot = accepted_catalog(program);
        store.begin().expect("begin catalog stamp");
        store
            .replace_catalog_snapshot(&snapshot)
            .expect("write accepted catalog snapshot");
        store.commit().expect("commit catalog stamp");
        let profile = marrow_run::evolution::current_engine_profile();
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 1,
                catalog_epoch: program.catalog.accepted_epoch.expect("accepted epoch"),
                layout_epoch: profile.layout_epoch(),
                source_digest: program.source_digest().to_string(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
            })
            .expect("stamp commit");

        let mut archive = Vec::new();
        let mut nondeterminism =
            FixedNondeterminism::new(0, 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        ensure_store_uid(&store, &mut nondeterminism).expect("store uid");
        create_backup(program, &store, &mut archive).expect("create backup");
        archive
    }

    fn restore_into_empty(
        program: &CheckedProgram,
        archive: &[u8],
    ) -> Result<RestoreReport, BackupError> {
        let target = TreeStore::memory();
        restore_backup(program, &target, &mut &archive[..], accept)
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_a_bad_magic_with_format_version() {
        let (root, program) =
            committed_program("restore-magic", BOOK_SOURCE).expect("committed backup fixture");
        let mut archive = seeded_backup(&program);
        archive[0] ^= 0xff; // Corrupt the magic header.
        let error = restore_into_empty(&program, &archive).expect_err("bad magic is rejected");
        assert_eq!(error.code(), "restore.format_version");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_unsupported_format_version() {
        let (root, program) =
            committed_program("restore-version", BOOK_SOURCE).expect("committed backup fixture");
        let mut archive = seeded_backup(&program);
        // The version is the four bytes after the 8-byte magic; bump it past what this
        // build writes.
        archive[11] = archive[11].wrapping_add(1);
        let error = restore_into_empty(&program, &archive).expect_err("bad version is rejected");
        assert_eq!(error.code(), "restore.format_version");
        cleanup(&root);
    }

    #[test]
    fn rejects_a_source_mismatch() {
        let (root_a, program_a) =
            committed_program("restore-source-a", BOOK_SOURCE).expect("committed backup fixture");
        let archive = seeded_backup(&program_a);
        // A different schema: an added field changes the source digest.
        let (root_b, program_b) = committed_program(
            "restore-source-b",
            "module shelf\n\nresource Book\n    \
             required title: string\n    pages: int\nstore ^books(id: int): Book\n",
        )
        .expect("committed backup fixture");
        let error =
            restore_into_empty(&program_b, &archive).expect_err("a foreign schema is rejected");
        assert_eq!(error.code(), "restore.source_mismatch");
        cleanup(&root_a);
        cleanup(&root_b);
    }

    #[test]
    fn rejects_a_catalog_mismatch() {
        let (root, program) =
            committed_program("restore-catalog", BOOK_SOURCE).expect("committed backup fixture");
        let archive = seeded_backup(&program);
        // Same source digest, different accepted epoch: the data belongs to another
        // committed catalog state.
        let mut other = program.clone();
        other.catalog.accepted_epoch = Some(program.catalog.accepted_epoch.unwrap_or(0) + 7);
        let error = restore_into_empty(&other, &archive)
            .expect_err("a different accepted epoch is rejected");
        assert_eq!(error.code(), "restore.catalog_mismatch");
        cleanup(&root);
    }

    #[test]
    fn rejects_a_non_empty_parent_snapshot_digest() {
        let (root, program) = committed_program("restore-parent-snapshot", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let archive = rewrite_manifest(&archive, |manifest| {
            manifest["parent_snapshot_digest"] = serde_json::json!(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            );
        });

        let error = restore_into_empty(&program, &archive)
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
        cleanup(&root);
    }

    #[test]
    fn rejects_commit_metadata_that_disagrees_with_manifest() {
        let (root, program) = committed_program("restore-commit-binding", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let archive = rewrite_manifest(&archive, |manifest| {
            manifest["commit"]["source_digest"] = serde_json::json!(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            );
        });

        let error = restore_into_empty(&program, &archive)
            .expect_err("commit metadata must agree with the validated manifest");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_engine_recompile() {
        let (root, program) =
            committed_program("restore-engine", BOOK_SOURCE).expect("committed backup fixture");
        let archive = seeded_backup(&program);
        // Rewrite the manifest's engine layout epoch to a value this build does not
        // write, so the restore reports an engine recompile is required.
        let archive = with_bumped_layout_epoch(&archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("a foreign engine layout is rejected");
        assert_eq!(error.code(), "restore.engine_recompile_required");
        cleanup(&root);
    }

    #[test]
    fn rejects_a_declared_cell_that_does_not_decode() {
        let (root, program) =
            committed_program("restore-decode", BOOK_SOURCE).expect("committed backup fixture");
        let archive = seeded_backup(&program);
        // Restore replays the data, then the verify proves the declared records decode.
        // A title leaf whose bytes are not a canonical string is `restore.data_invalid`.
        let target = TreeStore::memory();
        let verify = |restore_program: &CheckedProgram, store: &TreeStore| {
            match marrow_check::tooling::count_activation_integrity_problems(store, restore_program)
            {
                Ok((_, 0)) => Ok(()),
                Ok((_, _)) => Err(BackupError::DataInvalid(
                    "declared data does not decode".into(),
                )),
                Err(error) => Err(BackupError::Store(error)),
            }
        };
        // Replace the seeded title value with bytes that are not a canonical string.
        let archive = with_corrupt_first_value(&archive);
        let error = restore_backup(&program, &target, &mut &archive[..], verify)
            .expect_err("undecodable declared data is rejected");
        assert_eq!(error.code(), "restore.data_invalid");
        cleanup(&root);
    }

    #[test]
    fn rejects_a_malformed_data_cell_target_even_when_the_checksum_matches() {
        let (root, program) = committed_program("restore-malformed-target", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let archive = with_malformed_first_target(&archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("a malformed backup data target is rejected during replay");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_impossible_backup_target_count_even_when_the_checksum_matches() {
        let (root, program) = committed_program("restore-impossible-target-count", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let archive = with_impossible_first_target_count(&archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("an impossible backup target count is rejected during replay");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_empty_path_value_target_even_when_the_checksum_matches() {
        let (root, program) = committed_program("restore-empty-value-target", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let archive = with_empty_path_value_target(&program, &archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("an empty value target aliases a node cell and is rejected");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        cleanup(&root);
    }

    #[test]
    fn restore_replays_the_accepted_catalog_rows() {
        let (root, program) = committed_program("restore-catalog-rows", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let target = TreeStore::memory();
        restore_backup(&program, &target, &mut &archive[..], accept).expect("restore");

        let restored = target
            .read_catalog_snapshot()
            .expect("read restored catalog")
            .expect("restored catalog is present");
        assert_eq!(
            restored,
            accepted_catalog(&program),
            "restore replays the accepted catalog rows verbatim"
        );
        cleanup(&root);
    }

    #[test]
    fn restore_mints_a_fresh_store_uid() {
        let (root, program) =
            committed_program("restore-store-uid", BOOK_SOURCE).expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let source_uid = split_archive(&archive).manifest["store_uid"]
            .as_str()
            .expect("manifest store uid")
            .to_string();
        let target = TreeStore::memory();

        restore_backup(&program, &target, &mut &archive[..], accept).expect("restore");

        let restored_uid = target
            .read_store_uid()
            .expect("read restored uid")
            .expect("restored uid");
        assert_ne!(restored_uid.as_str(), source_uid);
        cleanup(&root);
    }

    #[test]
    fn rejects_a_manifest_catalog_digest_that_disagrees_with_the_section() {
        let (root, program) = committed_program("restore-catalog-digest", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        // Tamper only the manifest's catalog fingerprint, leaving the catalog section
        // rows untouched, so the recomputed section digest no longer matches.
        let archive = rewrite_manifest(&archive, |manifest| {
            manifest["catalog_digest"] = serde_json::json!(
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            );
        });

        let target = TreeStore::memory();
        let error = restore_backup(&program, &target, &mut &archive[..], accept)
            .expect_err("a catalog digest that disagrees with the section is rejected");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        assert!(matches!(
            error,
            BackupError::CorruptChunk {
                problem: BackupCorruptProblem::CatalogDigestMismatch,
                ..
            }
        ));
        assert!(
            target
                .read_catalog_snapshot()
                .expect("read target")
                .is_none(),
            "a rejected restore leaves the target without a catalog"
        );
        cleanup(&root);
    }

    #[test]
    fn rejects_a_manifest_catalog_epoch_that_disagrees_with_the_section() {
        let (root, program) = committed_program("restore-catalog-epoch", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let archive = rewrite_manifest(&archive, |manifest| {
            let epoch = manifest
                .get("catalog_epoch")
                .and_then(|value| value.as_u64())
                .expect("catalog epoch");
            manifest["catalog_epoch"] = serde_json::json!(epoch + 1);
        });

        let result = read_backup_prologue(&mut &archive[..]);
        cleanup(&root);

        let error = match result {
            Ok(_) => panic!("manifest/catalog section epoch mismatch must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "restore.corrupt_chunk");
        assert!(matches!(
            error,
            BackupError::CorruptChunk {
                problem: BackupCorruptProblem::ManifestCatalogBindingMismatch,
                ..
            }
        ));
    }

    #[test]
    fn rejects_a_tampered_manifest_through_the_folded_checksum() {
        let (root, program) = committed_program("restore-manifest-tamper", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        // Flip a manifest field that passes every structural check but is not part of
        // any prior binding: the record count stays, the engine and catalog still agree,
        // yet the folded integrity checksum no longer matches the manifest bytes.
        let archive = rewrite_manifest(&archive, |manifest| {
            let commit = manifest.get_mut("commit").expect("commit object");
            commit["commit_id"] = serde_json::json!(999);
        });

        let target = TreeStore::memory();
        let error = restore_backup(&program, &target, &mut &archive[..], accept)
            .expect_err("a tampered manifest is rejected by the folded checksum");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        assert!(matches!(
            error,
            BackupError::CorruptChunk {
                problem: BackupCorruptProblem::ChecksumMismatch,
                ..
            }
        ));
        assert!(
            target.is_empty().expect("read target")
                && target
                    .read_catalog_snapshot()
                    .expect("read catalog")
                    .is_none(),
            "a rejected restore leaves the target untouched"
        );
        cleanup(&root);
    }

    #[test]
    fn rolls_back_when_a_catalog_row_corrupts_mid_replay() {
        let (root, program) = committed_program("restore-catalog-rollback", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        // Corrupt one catalog row inside the section. The section decode recomputes the
        // catalog digest from the rows, so a tampered row fails closed before any data
        // or catalog write reaches the target.
        let archive = with_corrupt_catalog_row(&archive);
        let target = TreeStore::memory();
        let error = restore_backup(&program, &target, &mut &archive[..], accept)
            .expect_err("a corrupt catalog row is rejected");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        assert!(
            target.is_empty().expect("data empty")
                && target
                    .read_catalog_snapshot()
                    .expect("catalog read")
                    .is_none(),
            "a corrupt catalog row leaves the target empty with no catalog"
        );
        cleanup(&root);
    }

    #[test]
    fn rolls_back_when_entropy_fails_before_replay_writes_uid() {
        let (root, program) = committed_program("restore-entropy-rollback", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let mut input = &archive[..];
        let prologue = read_backup_prologue(&mut input).expect("read prologue");
        let target = TreeStore::memory();
        let error = restore_backup_with_prologue(
            &program,
            &target,
            prologue,
            &mut input,
            RestoreTargetMode::EmptyOnly,
            &mut FailingNondeterminism,
            accept,
        )
        .expect_err("entropy failure rejects restore");

        assert!(matches!(error, BackupError::Io(_)));
        assert!(
            target.read_store_uid().expect("read store UID").is_none()
                && target
                    .read_catalog_snapshot()
                    .expect("read catalog")
                    .is_none()
                && target.is_empty().expect("data empty"),
            "entropy failure leaves no store UID, catalog snapshot, or data"
        );
        cleanup(&root);
    }

    #[test]
    fn the_catalog_section_and_data_stream_are_disjoint() {
        let (root, program) = committed_program("restore-section-disjoint", BOOK_SOURCE)
            .expect("committed backup fixture");
        let archive = seeded_backup(&program);
        let parts = split_archive(&archive);

        // The catalog section carries exactly the accepted rows.
        let section_text = std::str::from_utf8(&parts.catalog).expect("catalog section is utf8");
        let section: CatalogMetadata =
            serde_json::from_str(section_text).expect("catalog section parses");
        assert_eq!(section, accepted_catalog(&program));
        assert!(
            !section.entries.is_empty(),
            "the fixture has accepted catalog rows to carry"
        );

        // Every chunk in the data stream decodes through the production backup-cell reader
        // as a data-family cell. Catalog rows use a different family and grammar, so a
        // catalog row framed into the data stream would fail this read.
        let mut cursor = &parts.data[..];
        let mut data_cells = 0;
        while !cursor.is_empty() {
            archive::read_cell(&mut cursor).expect("data stream holds only data cells");
            data_cells += 1;
        }
        assert_eq!(
            data_cells, 1,
            "the data stream carries only the seeded cell"
        );
        cleanup(&root);
    }

    /// Re-encode the manifest with a layout epoch bumped past the running build's, so a
    /// restore validates it as a foreign engine. The cell stream is left intact.
    fn with_bumped_layout_epoch(archive: &[u8]) -> Vec<u8> {
        rewrite_manifest(archive, |manifest| {
            let engine = manifest.get_mut("engine").expect("engine object");
            let epoch = engine.get("layout_epoch").and_then(|v| v.as_u64()).unwrap();
            engine["layout_epoch"] = serde_json::json!(epoch + 100);
        })
    }

    /// Flip the first cell value byte so its declared scalar no longer decodes, leaving
    /// the framing and checksum consistent with the mutated bytes.
    fn with_corrupt_first_value(archive: &[u8]) -> Vec<u8> {
        // The title value "Mort" is the only leaf; replacing it with an invalid UTF-8
        // byte sequence keeps the framing but breaks the canonical string decode. Find
        // the ASCII "Mort" in the stream and overwrite it with high bytes, then rebuild
        // the manifest checksum to match.
        let mut bytes = archive.to_vec();
        let needle = b"Mort";
        let pos = bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("seeded title is in the stream");
        for byte in &mut bytes[pos..pos + needle.len()] {
            *byte = 0xff;
        }
        rebuild_checksum(&bytes)
    }

    fn with_malformed_first_target(archive: &[u8]) -> Vec<u8> {
        let parts = split_archive(archive);
        let mut cursor = &parts.data[..];
        let mut first = read_raw_test_cell(&mut cursor);
        first.0[0] = 0xff;

        let mut rewritten = Vec::new();
        write_raw_test_cell(&mut rewritten, &first.0, &first.1);
        rewritten.extend_from_slice(cursor);
        reframe_with_matching_checksum(parts.manifest, parts.catalog, rewritten)
    }

    fn with_impossible_first_target_count(archive: &[u8]) -> Vec<u8> {
        let parts = split_archive(archive);
        let mut cursor = &parts.data[..];
        let mut first = read_raw_test_cell(&mut cursor);
        first.0.truncate(2);
        write_test_chunk(&mut first.0, b"cat_0123456789abcdef");
        first.0.extend_from_slice(&(u32::MAX).to_be_bytes());

        let mut rewritten = Vec::new();
        write_raw_test_cell(&mut rewritten, &first.0, &first.1);
        rewritten.extend_from_slice(cursor);
        reframe_with_matching_checksum(parts.manifest, parts.catalog, rewritten)
    }

    fn with_empty_path_value_target(program: &CheckedProgram, archive: &[u8]) -> Vec<u8> {
        let mut parts = split_archive(archive);
        parts.manifest["record_count"] = serde_json::json!(1);

        let place = marrow_check::checked_saved_root_place(
            program,
            "books",
            marrow_syntax::SourceSpan::default(),
        )
        .expect("checked saved place");
        let mut target = Vec::new();
        target.push(0); // typed target-frame version.
        target.push(3); // value target.
        target.extend_from_slice(&0u32.to_be_bytes()); // empty member/key path.
        let store_catalog_id = place.store_catalog_id.expect("accepted store id");
        write_test_chunk(&mut target, store_catalog_id.as_bytes());
        target.extend_from_slice(&1u32.to_be_bytes());
        write_test_chunk(&mut target, &encode_identity_payload(&[SavedKey::Int(1)]));

        let mut rewritten = Vec::new();
        write_raw_test_cell(&mut rewritten, &target, b"not-a-node-marker");
        reframe_with_matching_checksum(parts.manifest, parts.catalog, rewritten)
    }

    /// Parse the manifest JSON, apply `edit`, and re-frame the archive with the rewritten
    /// manifest, leaving the catalog section and cell stream unchanged.
    fn rewrite_manifest(archive: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let mut parts = split_archive(archive);
        edit(&mut parts.manifest);
        frame(&parts.manifest, &parts.catalog, parts.data)
    }

    /// Re-checksum the whole archive and write the matching `archive_checksum` into the
    /// manifest, so a value mutation is not caught as a checksum error first.
    fn rebuild_checksum(archive: &[u8]) -> Vec<u8> {
        let parts = split_archive(archive);
        reframe_with_matching_checksum(parts.manifest, parts.catalog, parts.data)
    }

    /// Tamper one catalog entry's path inside the section, leaving the section's stored
    /// digest unchanged. The section decode recomputes the digest from the rows, so the
    /// mismatch fails the row closed; the archive checksum is rebuilt over the mutated
    /// bytes so the row decode, not the checksum, is the gate under test.
    fn with_corrupt_catalog_row(archive: &[u8]) -> Vec<u8> {
        let parts = split_archive(archive);
        let mut section: CatalogMetadata =
            serde_json::from_slice(&parts.catalog).expect("parse catalog section");
        // Mutate a row but leave the stored digest, which was computed over the original
        // rows, so the section's recompute-compare on read rejects the tampered row.
        let entry = section
            .entries
            .first_mut()
            .expect("a catalog entry to tamper");
        entry.path.push_str("-tampered");
        let catalog = section
            .to_json_pretty()
            .expect("catalog renders")
            .into_bytes();
        reframe_with_matching_checksum(parts.manifest, catalog, parts.data)
    }

    /// Recompute the integrity checksum over the manifest, catalog section, and data
    /// cells, write it into the manifest, and re-frame, the read side's exact contract.
    fn reframe_with_matching_checksum(
        mut manifest: serde_json::Value,
        catalog: Vec<u8>,
        data: Vec<u8>,
    ) -> Vec<u8> {
        manifest["state_digest"] = serde_json::json!(marrow_project::sha256_digest(&data));
        manifest["archive_checksum"] = serde_json::json!(0u64);
        let checksum = archive_checksum(&manifest, &catalog, &data);
        manifest["archive_checksum"] = serde_json::json!(checksum);
        frame(&manifest, &catalog, data)
    }

    /// The integrity checksum over the manifest (with its checksum field zeroed), the
    /// catalog-section chunk, and the data cells, in archive order.
    fn archive_checksum(manifest: &serde_json::Value, catalog: &[u8], data: &[u8]) -> u64 {
        let mut zeroed = manifest.clone();
        zeroed["archive_checksum"] = serde_json::json!(0u64);
        let manifest_bytes = serde_json::to_vec(&zeroed).expect("serialize manifest");
        let mut hash = fold_raw_test_chunk(CHECKSUM_SEED, &manifest_bytes);
        hash = fold_raw_test_chunk(hash, catalog);
        let count = manifest
            .get("record_count")
            .and_then(|v| v.as_u64())
            .unwrap();
        let mut cursor = data;
        for _ in 0..count {
            let (key, value) = read_raw_test_cell(&mut cursor);
            hash = checksum_raw_test_cell(hash, &key, &value);
        }
        hash
    }

    fn read_raw_test_cell(input: &mut &[u8]) -> (Vec<u8>, Vec<u8>) {
        let key = read_raw_test_chunk(input);
        let value = read_raw_test_chunk(input);
        (key, value)
    }

    fn read_raw_test_chunk(input: &mut &[u8]) -> Vec<u8> {
        let (len, rest) = input.split_at(4);
        let len = u32::from_be_bytes(len.try_into().expect("cell chunk length")) as usize;
        let (chunk, rest) = rest.split_at(len);
        *input = rest;
        chunk.to_vec()
    }

    fn write_raw_test_cell(out: &mut Vec<u8>, key: &[u8], value: &[u8]) {
        out.extend_from_slice(&(key.len() as u32).to_be_bytes());
        out.extend_from_slice(key);
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
    }

    fn write_test_chunk(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    fn checksum_raw_test_cell(hash: u64, key: &[u8], value: &[u8]) -> u64 {
        let hash = fold_raw_test_chunk(hash, key);
        fold_raw_test_chunk(hash, value)
    }

    /// Fold one length-prefixed chunk the way the production checksum frames the manifest,
    /// catalog section, and each cell chunk.
    fn fold_raw_test_chunk(hash: u64, bytes: &[u8]) -> u64 {
        let hash = fold_raw_test_checksum(hash, &(bytes.len() as u32).to_be_bytes());
        fold_raw_test_checksum(hash, bytes)
    }

    fn fold_raw_test_checksum(mut hash: u64, bytes: &[u8]) -> u64 {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// A framed archive split into its parsed manifest JSON, the raw catalog section
    /// bytes, and the trailing cell stream.
    struct ArchiveParts {
        manifest: serde_json::Value,
        catalog: Vec<u8>,
        data: Vec<u8>,
    }

    fn split_archive(archive: &[u8]) -> ArchiveParts {
        let magic_and_version = 12; // 8-byte magic + 4-byte version.
        let len_end = magic_and_version + 4;
        let manifest_len = read_len(archive, magic_and_version);
        let manifest_end = len_end + manifest_len;
        let manifest: serde_json::Value =
            serde_json::from_slice(&archive[len_end..manifest_end]).expect("parse manifest");

        let catalog_len = read_len(archive, manifest_end);
        let catalog_start = manifest_end + 4;
        let catalog_end = catalog_start + catalog_len;
        ArchiveParts {
            manifest,
            catalog: archive[catalog_start..catalog_end].to_vec(),
            data: archive[catalog_end..].to_vec(),
        }
    }

    fn read_len(archive: &[u8], at: usize) -> usize {
        u32::from_be_bytes(archive[at..at + 4].try_into().expect("length frame")) as usize
    }

    /// Re-frame an archive from a manifest value, a catalog section, and a cell stream:
    /// magic, version, manifest length, manifest, catalog length, catalog, then the cells.
    fn frame(manifest: &serde_json::Value, catalog: &[u8], stream: Vec<u8>) -> Vec<u8> {
        let manifest = serde_json::to_vec(manifest).expect("serialize manifest");
        let mut out = Vec::new();
        out.extend_from_slice(b"MARROWBK");
        out.extend_from_slice(&crate::backup::FORMAT_VERSION.to_be_bytes());
        out.extend_from_slice(&(manifest.len() as u32).to_be_bytes());
        out.extend_from_slice(&manifest);
        out.extend_from_slice(&(catalog.len() as u32).to_be_bytes());
        out.extend_from_slice(catalog);
        out.extend_from_slice(&stream);
        out
    }
}
