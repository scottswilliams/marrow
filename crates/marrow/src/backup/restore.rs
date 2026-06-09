//! Restoring a backup: validate it against the project and engine, then replay
//! its cells into an empty store in one transaction so the target either gains
//! the whole backup or is left unchanged.

use std::io::Read;

use marrow_check::CheckedProgram;
use marrow_run::evolution::{ApplyError, current_engine_profile, rebuild_store_indexes};
use marrow_store::cell::DataCellKind;
use marrow_store::tree::{TreeBackupCellBuf, TreeStore};

use super::archive::{self, CHECKSUM_SEED, checksum_cell};
use super::{BackupCorruptProblem, BackupError, BackupManifest, EngineDescriptor};

/// What a completed restore replayed.
#[derive(Debug)]
pub(crate) struct RestoreReport {
    pub(crate) record_count: u64,
    pub(crate) catalog_epoch: Option<u64>,
}

/// Restore the backup in `input` into `store`, an empty native store for
/// `program`. The whole replay runs in one transaction: a checksum mismatch, a
/// short stream, or a `verify` failure rolls the target back to empty. `verify`
/// proves the restored data compiles against the project schema before the
/// transaction commits.
pub(crate) fn restore_backup(
    program: &CheckedProgram,
    store: &TreeStore,
    input: &mut impl Read,
    verify: impl Fn(&CheckedProgram, &TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let manifest = archive::read_header(input)?;
    let restore_program = restore_program(program, &manifest)?;
    if !store.is_empty()? {
        return Err(BackupError::NotEmpty);
    }

    store.begin()?;
    match replay(&restore_program, store, &manifest, input, &verify) {
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

/// Refuse a backup outside this binary's checked replay contract: a different
/// engine, layout, or value codec needs a recompile; a different schema or
/// catalog epoch belongs to a different program state.
fn restore_program(
    program: &CheckedProgram,
    manifest: &BackupManifest,
) -> Result<CheckedProgram, BackupError> {
    let current = EngineDescriptor::current(&current_engine_profile());
    if manifest.engine != current {
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
    if let Some(commit) = &manifest.commit {
        commit.validate_manifest_binding(manifest)?;
    }

    let Some(backup_epoch) = manifest.catalog_epoch else {
        return Ok(program.clone());
    };
    if Some(backup_epoch) == program.catalog.accepted_epoch {
        return Ok(program.clone());
    }

    activation_window_program(program, manifest, backup_epoch)
}

fn activation_window_program(
    program: &CheckedProgram,
    manifest: &BackupManifest,
    backup_epoch: u64,
) -> Result<CheckedProgram, BackupError> {
    let Some(commit) = &manifest.commit else {
        return catalog_mismatch();
    };
    let Some(commit_proposal_digest) = commit.activation_proposal_catalog_digest.as_deref() else {
        return catalog_mismatch();
    };
    let rebound = marrow_check::evolution::rebind_activation_resume_program(
        program,
        &commit.to_metadata()?.activation_proposal_new_catalog_ids,
    )
    .map_err(|_| {
        BackupError::CatalogMismatch(
            "backup catalog epoch does not match this project's accepted catalog".to_string(),
        )
    })?;
    let Some(proposal) = &rebound.catalog.proposal else {
        return catalog_mismatch();
    };
    if proposal.epoch != backup_epoch || proposal.digest != commit_proposal_digest {
        return catalog_mismatch();
    }
    Ok(rebound)
}

fn catalog_mismatch<T>() -> Result<T, BackupError> {
    Err(BackupError::CatalogMismatch(
        "backup catalog epoch does not match this project's accepted catalog".to_string(),
    ))
}

fn replay(
    program: &CheckedProgram,
    store: &TreeStore,
    manifest: &BackupManifest,
    input: &mut impl Read,
    verify: &impl Fn(&CheckedProgram, &TreeStore) -> Result<(), BackupError>,
) -> Result<RestoreReport, BackupError> {
    let mut checksum = CHECKSUM_SEED;
    for _ in 0..manifest.record_count {
        let cell = archive::read_cell(input)?;
        checksum = checksum_cell(checksum, cell.as_ref());
        restore_cell(store, &cell)?;
    }
    if checksum != manifest.data_checksum {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::ChecksumMismatch,
            "backup data checksum does not match its manifest".to_string(),
        ));
    }
    // Count and checksum matched, so any byte past the last cell is junk the backup
    // did not write: a truncated, doubled, or tampered file.
    if has_trailing_bytes(input)? {
        return Err(BackupError::corrupt(
            BackupCorruptProblem::TrailingBytes,
            "backup carries trailing bytes after its cell stream".to_string(),
        ));
    }

    // Indexes are derived, so rebuild them from the replayed records rather than
    // trusting bytes that could disagree. The rebuild runs inside this open
    // transaction, so the commit makes indexes durable atomically with the data.
    rebuild_store_indexes(program, store).map_err(rebuild_error)?;

    // Stamp the durable identity the data was written under, so the restored store
    // fences exactly as the original did. The engine profile is this build's, already
    // proven equal to the manifest's.
    store.write_engine_profile(&current_engine_profile())?;
    if let Some(epoch) = manifest.catalog_epoch {
        store.write_catalog_epoch(epoch)?;
    }
    if let Some(commit) = &manifest.commit {
        store.write_commit_metadata(&commit.to_metadata()?)?;
    }

    // Reads inside the open transaction see the staged data, so verification runs
    // before commit and a failure rolls the whole restore back.
    verify(program, store)?;

    Ok(RestoreReport {
        record_count: manifest.record_count,
        catalog_epoch: manifest.catalog_epoch,
    })
}

fn restore_cell(store: &TreeStore, cell: &TreeBackupCellBuf) -> Result<(), BackupError> {
    let target = cell.data_key();
    match &target.kind {
        DataCellKind::Node => store.write_node(&target.store, &target.identity)?,
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
    use std::path::Path;

    use marrow_check::CheckedProgram;
    use marrow_store::cell::CatalogId;
    use marrow_store::key::{SavedKey, encode_identity_payload};
    use marrow_store::tree::{CommitMetadata, DataPathSegment, TreeStore};

    use super::super::test_support::{BOOK_SOURCE, committed_program};
    use super::{BackupError, CHECKSUM_SEED, RestoreReport, restore_backup};
    use crate::backup::create_backup;

    /// Restore that verifies nothing: the restore.* codes under test fail in
    /// validation or replay, before a schema check would run.
    fn accept(_program: &CheckedProgram, _store: &TreeStore) -> Result<(), BackupError> {
        Ok(())
    }

    /// Seed one book through the managed tree-cell write path, then build a valid
    /// in-memory backup of the store under `program`.
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
        if let Some(epoch) = program.catalog.accepted_epoch {
            store.write_catalog_epoch(epoch).expect("stamp epoch");
        }
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

        let mut archive = Vec::new();
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
        let (root, program) = committed_program("restore-magic", BOOK_SOURCE);
        let mut archive = seeded_backup(&program);
        archive[0] ^= 0xff; // Corrupt the magic header.
        let error = restore_into_empty(&program, &archive).expect_err("bad magic is rejected");
        assert_eq!(error.code(), "restore.format_version");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_unsupported_format_version() {
        let (root, program) = committed_program("restore-version", BOOK_SOURCE);
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
        let (root_a, program_a) = committed_program("restore-source-a", BOOK_SOURCE);
        let archive = seeded_backup(&program_a);
        // A different schema: an added field changes the source digest.
        let (root_b, program_b) = committed_program(
            "restore-source-b",
            "module shelf\n\nresource Book at ^books(id: int)\n    \
             required title: string\n    pages: int\n",
        );
        let error =
            restore_into_empty(&program_b, &archive).expect_err("a foreign schema is rejected");
        assert_eq!(error.code(), "restore.source_mismatch");
        cleanup(&root_a);
        cleanup(&root_b);
    }

    #[test]
    fn rejects_a_catalog_mismatch() {
        let (root, program) = committed_program("restore-catalog", BOOK_SOURCE);
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
    fn rejects_commit_metadata_that_disagrees_with_manifest() {
        let (root, program) = committed_program("restore-commit-binding", BOOK_SOURCE);
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
        let (root, program) = committed_program("restore-engine", BOOK_SOURCE);
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
        let (root, program) = committed_program("restore-decode", BOOK_SOURCE);
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
        let (root, program) = committed_program("restore-malformed-target", BOOK_SOURCE);
        let archive = seeded_backup(&program);
        let archive = with_malformed_first_target(&archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("a malformed backup data target is rejected during replay");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_impossible_backup_target_count_even_when_the_checksum_matches() {
        let (root, program) = committed_program("restore-impossible-target-count", BOOK_SOURCE);
        let archive = seeded_backup(&program);
        let archive = with_impossible_first_target_count(&archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("an impossible backup target count is rejected during replay");
        assert_eq!(error.code(), "restore.corrupt_chunk");
        cleanup(&root);
    }

    #[test]
    fn rejects_an_empty_path_value_target_even_when_the_checksum_matches() {
        let (root, program) = committed_program("restore-empty-value-target", BOOK_SOURCE);
        let archive = seeded_backup(&program);
        let archive = with_empty_path_value_target(&program, &archive);
        let error = restore_into_empty(&program, &archive)
            .expect_err("an empty value target aliases a node cell and is rejected");
        assert_eq!(error.code(), "restore.corrupt_chunk");
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
        let (mut manifest, stream) = split_archive(archive);
        let mut cursor = &stream[..];
        let mut first = read_raw_test_cell(&mut cursor);
        first.0[0] = 0xff;

        let mut rewritten = Vec::new();
        write_raw_test_cell(&mut rewritten, &first.0, &first.1);
        rewritten.extend_from_slice(cursor);

        let mut checksum = CHECKSUM_SEED;
        let mut checksum_cursor = &rewritten[..];
        let count = manifest
            .get("record_count")
            .and_then(|v| v.as_u64())
            .unwrap();
        for _ in 0..count {
            let (key, value) = read_raw_test_cell(&mut checksum_cursor);
            checksum = checksum_raw_test_cell(checksum, &key, &value);
        }
        manifest["data_checksum"] = serde_json::json!(checksum);
        frame(&manifest, rewritten)
    }

    fn with_impossible_first_target_count(archive: &[u8]) -> Vec<u8> {
        let (mut manifest, stream) = split_archive(archive);
        let mut cursor = &stream[..];
        let mut first = read_raw_test_cell(&mut cursor);
        first.0.truncate(2);
        write_test_chunk(&mut first.0, b"cat_0123456789abcdef");
        first.0.extend_from_slice(&(u32::MAX).to_be_bytes());

        let mut rewritten = Vec::new();
        write_raw_test_cell(&mut rewritten, &first.0, &first.1);
        rewritten.extend_from_slice(cursor);
        write_matching_checksum(&mut manifest, &rewritten);
        frame(&manifest, rewritten)
    }

    fn with_empty_path_value_target(program: &CheckedProgram, archive: &[u8]) -> Vec<u8> {
        let (mut manifest, _stream) = split_archive(archive);
        manifest["record_count"] = serde_json::json!(1);

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
        write_matching_checksum(&mut manifest, &rewritten);
        frame(&manifest, rewritten)
    }

    /// Parse the manifest JSON, apply `edit`, and re-frame the archive with the rewritten
    /// manifest and an unchanged cell stream.
    fn rewrite_manifest(archive: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let (mut manifest, stream) = split_archive(archive);
        edit(&mut manifest);
        frame(&manifest, stream)
    }

    /// Re-checksum the cell stream in `archive` and write the matching `data_checksum`
    /// into the manifest, so a value mutation is not caught as a checksum error first.
    fn rebuild_checksum(archive: &[u8]) -> Vec<u8> {
        let (mut manifest, stream) = split_archive(archive);
        write_matching_checksum(&mut manifest, &stream);
        frame(&manifest, stream)
    }

    fn write_matching_checksum(manifest: &mut serde_json::Value, stream: &[u8]) {
        let mut checksum = CHECKSUM_SEED;
        let mut cursor = stream;
        let count = manifest
            .get("record_count")
            .and_then(|v| v.as_u64())
            .unwrap();
        for _ in 0..count {
            let (key, value) = read_raw_test_cell(&mut cursor);
            checksum = checksum_raw_test_cell(checksum, &key, &value);
        }
        manifest["data_checksum"] = serde_json::json!(checksum);
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
        let mut hash = fold_raw_test_checksum(hash, &(key.len() as u32).to_be_bytes());
        hash = fold_raw_test_checksum(hash, key);
        hash = fold_raw_test_checksum(hash, &(value.len() as u32).to_be_bytes());
        fold_raw_test_checksum(hash, value)
    }

    fn fold_raw_test_checksum(mut hash: u64, bytes: &[u8]) -> u64 {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// Split a framed archive into its parsed manifest JSON and the trailing cell stream.
    fn split_archive(archive: &[u8]) -> (serde_json::Value, Vec<u8>) {
        let magic_and_version = 12; // 8-byte magic + 4-byte version.
        let len_end = magic_and_version + 4;
        let manifest_len = u32::from_be_bytes(
            archive[magic_and_version..len_end]
                .try_into()
                .expect("manifest length"),
        ) as usize;
        let manifest_end = len_end + manifest_len;
        let manifest: serde_json::Value =
            serde_json::from_slice(&archive[len_end..manifest_end]).expect("parse manifest");
        (manifest, archive[manifest_end..].to_vec())
    }

    /// Re-frame an archive from a manifest value and a cell stream: magic, version,
    /// manifest length, manifest bytes, then the stream.
    fn frame(manifest: &serde_json::Value, stream: Vec<u8>) -> Vec<u8> {
        let manifest = serde_json::to_vec(manifest).expect("serialize manifest");
        let mut out = Vec::new();
        out.extend_from_slice(b"MARROWBK");
        out.extend_from_slice(&crate::backup::FORMAT_VERSION.to_be_bytes());
        out.extend_from_slice(&(manifest.len() as u32).to_be_bytes());
        out.extend_from_slice(&manifest);
        out.extend_from_slice(&stream);
        out
    }
}
