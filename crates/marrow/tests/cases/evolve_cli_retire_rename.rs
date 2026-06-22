use std::fs;
use std::path::Path;
use std::process::Command;

use crate::support;
use crate::support_evolve;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};
use support::{TempProject, marrow, unique_temp_path, write};
use support_evolve::{
    BLOCK_BASELINE_SOURCE, RENAME_BLOCK_DELETED_SOURCE, RENAME_BLOCK_SOURCE, RENAME_SOURCE,
    RETIRE_BASELINE_SOURCE, RETIRE_BLOCK_DELETED_SOURCE, RETIRE_BLOCK_SOURCE, RETIRE_SOURCE,
    accepted_catalog, commit_catalog, member_catalog_id, native_books_project, native_store_path,
    open_native_store, read_scalar, root_place, seed_member, seed_title_only, store_epoch,
};

struct RetireBackupFixture {
    root: TempProject,
    accepted_place: marrow_check::CheckedSavedPlace,
    subtitle_id: String,
    epoch_before: Option<u64>,
    lock_before: Option<String>,
    source_before: String,
}

fn populated_retire_backup_fixture(name: &str) -> RetireBackupFixture {
    populated_retire_backup_fixture_with_config(name, support::native_config())
}

fn populated_retire_backup_fixture_with_config(name: &str, config: &str) -> RetireBackupFixture {
    let root = native_books_project(name, RETIRE_BASELINE_SOURCE);
    write(&root, "marrow.json", config);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books").expect("books root place");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle").expect("subtitle catalog id");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let epoch_before = store_epoch(&root);
    let lock_before = fs::read_to_string(root.join("marrow.lock")).ok();
    write(&root, "src/books.mw", RETIRE_SOURCE);
    let source_before = fs::read_to_string(root.join("src/books.mw")).expect("source before");
    RetireBackupFixture {
        root,
        accepted_place,
        subtitle_id,
        epoch_before,
        lock_before,
        source_before,
    }
}

fn assert_retire_backup_path_refused(fixture: &RetireBackupFixture, backup_path: &Path) {
    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{}:1", fixture.subtitle_id),
        "--backup",
        backup_path.to_str().expect("backup path utf8"),
        "--format",
        "json",
        fixture.root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(
        record["code"],
        serde_json::json!("evolve.backup_path_managed")
    );
    assert_ne!(
        record["kind"],
        serde_json::json!("evolve_apply"),
        "managed-path refusal must not render a success receipt: {record}"
    );
    assert_retire_fixture_unchanged(fixture);
}

fn assert_retire_fixture_unchanged(fixture: &RetireBackupFixture) {
    assert_eq!(
        store_epoch(&fixture.root),
        fixture.epoch_before,
        "managed-path refusal must not advance the store"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("marrow.lock"))
            .ok()
            .as_ref(),
        fixture.lock_before.as_ref(),
        "managed-path refusal must not advance the committed lock"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/books.mw")).expect("source after"),
        fixture.source_before,
        "managed-path refusal must not edit source"
    );
    let store =
        TreeStore::open(&native_store_path(&fixture.root)).expect("live store remains usable");
    assert_eq!(
        read_scalar(
            &store,
            &fixture.accepted_place,
            1,
            "subtitle",
            ScalarType::Str,
        ),
        Some(Scalar::Str("sub".into())),
        "retired data survives the managed-path refusal"
    );
}

#[test]
fn evolve_apply_accepts_two_repeated_approve_retire_flags() -> Result<(), Box<dyn std::error::Error>>
{
    let root = native_books_project(
        "evolve-apply-multi-retire",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   notes: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    let notes_id = member_catalog_id(&accepted_place, "notes")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
        seed_member(
            &store,
            &accepted_place,
            1,
            "notes",
            Scalar::Str("note".into()),
        );
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         \x20   retire Book.notes\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--approve-retire",
        &format!("{notes_id}:1"),
        "--no-backup",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let subtitle_present = read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str);
    let notes_present = read_scalar(&store, &accepted_place, 1, "notes", ScalarType::Str);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    // Both approved retires apply: the retire witness counts the two cells removed,
    // asserted as the typed envelope field rather than the rendered count line.
    let record = support::json(output.stdout);
    assert_eq!(record["kind"], serde_json::json!("evolve_apply"));
    assert_eq!(record["status"], serde_json::json!("applied"));
    assert_eq!(record["records_retired"], serde_json::json!(2));
    assert_eq!(subtitle_present, None, "subtitle was retired");
    assert_eq!(notes_present, None, "notes was retired");

    Ok(())
}

#[test]
fn evolve_apply_counts_and_deletes_a_retired_member_in_each_owning_root()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-apply-retire-second-root",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         store ^library(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let books_place = root_place(&accepted, "books")?;
    let library_place = root_place(&accepted, "library")?;
    let subtitle_id = member_catalog_id(&library_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &books_place, 1, "Dune");
        seed_title_only(&store, &library_place, 2, "Hyperion");
        seed_member(
            &store,
            &library_place,
            2,
            "subtitle",
            Scalar::Str("Cantos".into()),
        );
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         store ^library(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(preview.status.code(), Some(1), "{preview:?}");
    let preview_record = support::json(preview.stdout);
    let retire_report = preview_record["blocking"]
        .as_array()
        .expect("blocking reports")
        .iter()
        .find(|report| report["data"]["catalog_id"] == serde_json::json!(subtitle_id))
        .unwrap_or_else(|| panic!("{preview_record:#?}"));
    assert_eq!(retire_report["data"]["populated"], serde_json::json!(1));

    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--no-backup",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");
    let apply_record = support::json(apply.stdout);
    assert_eq!(apply_record["records_retired"], serde_json::json!(1));
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &library_place, 2, "subtitle", ScalarType::Str),
        None,
        "the retired cell in the second owning root was deleted"
    );

    Ok(())
}

#[test]
fn retire_apply_requires_backup_or_explicit_opt_out_before_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-apply-retire-requires-backup",
        RETIRE_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let epoch_before = store_epoch(&root);
    let lock_path = root.join("marrow.lock");
    let lock_before = fs::read_to_string(&lock_path).ok();
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], serde_json::json!("evolve.requires_backup"));
    assert_eq!(
        store_epoch(&root),
        epoch_before,
        "backup refusal must not advance the store"
    );
    assert_eq!(
        fs::read_to_string(&lock_path).ok(),
        lock_before,
        "backup refusal must not advance the committed lock"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("sub".into())),
        "retired data survives the fail-closed refusal"
    );

    Ok(())
}

#[test]
fn retire_apply_requires_recovery_choice_for_zero_count_retire()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-apply-retire-zero-count-requires-backup",
        RETIRE_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }
    let epoch_before = store_epoch(&root);
    let lock_path = root.join("marrow.lock");
    let lock_before = fs::read_to_string(&lock_path).ok();
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:0"),
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], serde_json::json!("evolve.requires_backup"));
    assert!(
        record.get("recovery_point").is_none(),
        "a recovery refusal is not an apply receipt: {record}"
    );
    let epoch_after_refusal = store_epoch(&root);
    assert_eq!(
        epoch_after_refusal, epoch_before,
        "zero-count retire backup refusal must not advance the store"
    );
    let lock_after_refusal = fs::read_to_string(&lock_path).ok();
    assert_eq!(
        lock_after_refusal, lock_before,
        "zero-count retire backup refusal must not advance the committed lock"
    );

    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:0"),
        "--no-backup",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(apply.status.code(), Some(0), "{apply:?}");
    let receipt = support::json(apply.stdout);
    assert_eq!(receipt["records_retired"], serde_json::json!(0));
    assert_eq!(
        receipt["recovery_point"],
        serde_json::json!({ "kind": "no_backup" }),
        "the zero-count retire still records the explicit recovery opt-out: {receipt}"
    );

    Ok(())
}

#[test]
fn retire_apply_no_backup_opt_out_is_recorded_in_json_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-apply-retire-no-backup", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--no-backup",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["records_retired"], serde_json::json!(1));
    assert_eq!(
        record["recovery_point"],
        serde_json::json!({ "kind": "no_backup" }),
        "the explicit opt-out is part of the rendered receipt: {record}"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        None,
        "explicit opt-out permits the approved retire"
    );

    Ok(())
}

#[test]
fn retire_apply_refuses_backup_path_that_is_live_store_file_before_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-apply-retire-backup-live-store-refused",
        RETIRE_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let epoch_before = store_epoch(&root);
    let lock_path = root.join("marrow.lock");
    let lock_before = fs::read_to_string(&lock_path).ok();
    write(&root, "src/books.mw", RETIRE_SOURCE);
    let backup_path = native_store_path(&root);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--backup",
        backup_path.to_str().expect("backup path utf8"),
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(
        record["code"],
        serde_json::json!("evolve.backup_path_managed")
    );
    assert_ne!(
        record["kind"],
        serde_json::json!("evolve_apply"),
        "managed-path refusal must not render a success receipt: {record}"
    );
    assert_eq!(
        store_epoch(&root),
        epoch_before,
        "managed-path refusal must not advance the store"
    );
    assert_eq!(
        fs::read_to_string(&lock_path).ok(),
        lock_before,
        "managed-path refusal must not advance the committed lock"
    );
    let store = TreeStore::open(&backup_path).expect("live store remains usable");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("sub".into())),
        "retired data survives the managed-path refusal"
    );

    Ok(())
}

#[test]
fn retire_apply_refuses_backup_path_that_is_committed_lock_before_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project(
        "evolve-apply-retire-backup-lock-refused",
        RETIRE_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let epoch_before = store_epoch(&root);
    let lock_path = root.join("marrow.lock");
    let lock_before = fs::read_to_string(&lock_path).expect("committed lock before");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--backup",
        lock_path.to_str().expect("backup path utf8"),
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(
        record["code"],
        serde_json::json!("evolve.backup_path_managed")
    );
    assert_eq!(
        store_epoch(&root),
        epoch_before,
        "lock-path refusal must not advance the store"
    );
    assert_eq!(
        fs::read_to_string(&lock_path).expect("committed lock after"),
        lock_before,
        "lock-path refusal must not replace the committed lock"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("live store remains usable");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("sub".into())),
        "retired data survives the managed-path refusal"
    );

    Ok(())
}

#[test]
fn retire_apply_refuses_backup_path_under_source_root_before_mutation() {
    let fixture = populated_retire_backup_fixture("evolve-apply-retire-backup-source-refused");
    let backup_path = fixture.root.join("src/books.mw");

    assert_retire_backup_path_refused(&fixture, &backup_path);
}

#[cfg(unix)]
#[test]
fn retire_apply_refuses_backup_symlink_inside_source_root_before_mutation() {
    let fixture =
        populated_retire_backup_fixture("evolve-apply-retire-backup-source-symlink-refused");
    let outside_target = unique_temp_path("evolve-backup-symlink-target.mwbackup");
    fs::write(&outside_target, b"outside before").expect("outside symlink target");
    let backup_path = fixture.root.join("src/recovery-link.mwbackup");
    std::os::unix::fs::symlink(&outside_target, &backup_path).expect("backup symlink");

    assert_retire_backup_path_refused(&fixture, &backup_path);
    assert!(
        fs::symlink_metadata(&backup_path)
            .expect("backup symlink metadata")
            .file_type()
            .is_symlink(),
        "managed-path refusal must not replace the symlink inside source"
    );
    assert_eq!(
        fs::read(&outside_target).expect("outside target after"),
        b"outside before",
        "managed-path refusal must not overwrite through the symlink"
    );
}

#[test]
fn retire_apply_refuses_backup_path_that_is_project_config_before_mutation() {
    let fixture = populated_retire_backup_fixture("evolve-apply-retire-backup-config-refused");
    let backup_path = fixture.root.join("marrow.json");
    let config_before = fs::read_to_string(&backup_path).expect("config before");

    assert_retire_backup_path_refused(&fixture, &backup_path);
    assert_eq!(
        fs::read_to_string(&backup_path).expect("config after"),
        config_before,
        "managed-path refusal must not replace marrow.json"
    );
}

#[test]
fn retire_apply_refuses_backup_path_under_configured_test_path_before_mutation() {
    let fixture = populated_retire_backup_fixture_with_config(
        "evolve-apply-retire-backup-test-refused",
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "tests": ["tests"] }"#,
    );
    let backup_path = fixture.root.join("tests/smoke.mw");
    write(&fixture.root, "tests/smoke.mw", "fn smoke()\n    return\n");
    let test_before = fs::read_to_string(&backup_path).expect("test before");

    assert_retire_backup_path_refused(&fixture, &backup_path);
    assert_eq!(
        fs::read_to_string(&backup_path).expect("test after"),
        test_before,
        "managed-path refusal must not replace configured tests"
    );
}

#[test]
fn retire_apply_refuses_backup_path_under_native_data_dir_before_mutation() {
    let fixture = populated_retire_backup_fixture("evolve-apply-retire-backup-data-dir-refused");
    let backup_path = fixture.root.join(".data/recovery.mwbackup");
    assert!(
        !backup_path.exists(),
        "test expects a missing backup target"
    );

    assert_retire_backup_path_refused(&fixture, &backup_path);
    assert!(
        !backup_path.exists(),
        "managed-path refusal must not create a backup inside the native data dir"
    );
}

#[test]
fn retire_apply_backup_writes_valid_archive_then_applies() -> Result<(), Box<dyn std::error::Error>>
{
    let root = native_books_project("evolve-apply-retire-with-backup", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let lock_before =
        fs::read_to_string(root.join("marrow.lock")).expect("read committed lock before");
    write(&root, "src/books.mw", RETIRE_SOURCE);
    let backup_path = root.join("before-retire.mwbackup");
    let backup_arg = backup_path.to_str().expect("backup path utf8");

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--backup",
        backup_arg,
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["records_retired"], serde_json::json!(1));
    assert_eq!(
        record["recovery_point"],
        serde_json::json!({ "kind": "backup", "path": backup_arg }),
        "the rendered receipt records the backup path: {record}"
    );
    assert!(backup_path.exists(), "backup artifact is published");
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        None,
        "retire applies after the backup is written"
    );

    let restore_root = native_books_project(
        "evolve-apply-retire-backup-restores",
        RETIRE_BASELINE_SOURCE,
    );
    write(&restore_root, "marrow.lock", &lock_before);

    let restore = marrow(&[
        "restore",
        restore_root.to_str().expect("project path utf-8"),
        backup_arg,
    ]);

    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");
    let restored = TreeStore::open(&native_store_path(&restore_root)).expect("open restored store");
    assert_eq!(
        read_scalar(&restored, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("sub".into())),
        "the recovery archive preserves the pre-retire data"
    );

    Ok(())
}

#[test]
fn retire_apply_backup_failure_exits_before_mutating_store()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-apply-retire-backup-fails", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let epoch_before = store_epoch(&root);
    write(&root, "src/books.mw", RETIRE_SOURCE);
    let backup_path = unique_temp_path("evolve-backup-failure.mwbackup");

    let output = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .env("MARROW_TEST_BACKUP_FAIL_AFTER_BYTES", "0")
        .args([
            "evolve",
            "apply",
            "--maintenance",
            "--approve-retire",
            &format!("{subtitle_id}:1"),
            "--backup",
            backup_path.to_str().expect("backup path utf8"),
            "--format",
            "json",
            root.to_str().expect("project path utf-8"),
        ])
        .output()
        .expect("run marrow");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], serde_json::json!("io.write"));
    assert!(
        !backup_path.exists(),
        "failed backup must not publish the target artifact"
    );
    assert_eq!(
        store_epoch(&root),
        epoch_before,
        "apply must not advance the store after a backup write failure"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("sub".into())),
        "retired cell survives backup failure"
    );

    Ok(())
}

/// A bare source rename of a populated member — `subtitle` renamed to `blurb` in source
/// with no `evolve rename` intent — must not silently auto-apply on a plain `marrow run`.
/// A bare diff is ambiguous between rename and delete-and-add; reading it as delete-and-add
/// would orphan the populated `subtitle` and silently advance the epoch. The populated-drop
/// fence catches it: the run fails closed naming the required repair rather than dropping
/// the data, and the epoch does not advance.
#[test]
fn a_bare_rename_of_a_populated_member_does_not_silently_auto_apply()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("bare-rename-fences", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("Appendix".into()),
        );
    }

    // Rename `subtitle` to `blurb` in source only, with no `evolve rename` block and a
    // runnable entry that reads the renamed member.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   blurb: string\n\
         store ^books(id: int): Book\n\
         pub fn show(): string\n\
         \x20   return (^books(1).blurb ?? \"absent\")\n",
    );

    let run = marrow(&["run", "--entry", "books::show", root.to_str().unwrap()]);
    assert_eq!(
        run.status.code(),
        Some(1),
        "a bare rename over populated data must fence, not silently auto-apply: {run:?}"
    );
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.schema_drift"),
        "the run fences on schema drift rather than dropping the data: {stderr}"
    );

    // The epoch did not advance and the old `subtitle` cell still carries its data: nothing
    // was silently dropped.
    assert_eq!(
        store_epoch(&root),
        Some(baseline_epoch),
        "a fenced bare rename does not advance the epoch"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
        Some(Scalar::Str("Appendix".into())),
        "the populated member's cell survives the fenced run"
    );

    Ok(())
}

#[test]
fn evolve_apply_advances_accepted_catalog_in_lockstep_for_retire()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-apply-retire-lockstep", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    let output = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--no-backup",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let snapshot_epoch = accepted_catalog(&root).epoch;
    let store_epoch = store_epoch(&root);
    assert_eq!(
        store_epoch,
        Some(baseline_epoch + 1),
        "store advanced one epoch"
    );
    assert_eq!(
        snapshot_epoch,
        baseline_epoch + 1,
        "the accepted catalog snapshot advanced in lockstep with the store"
    );

    // With the accepted snapshot left behind the store epoch, the open fence rejects every
    // later run as `run.store_evolved` with no recovery; the lockstep advance keeps the
    // snapshot and store at one epoch, so the fence never reports the store as evolved.
    let run = marrow(&[
        "run",
        "--entry",
        "books::add",
        "--arg",
        "title=Dune",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        run.status.code(),
        Some(0),
        "run succeeds after lockstep advance: {run:?}"
    );
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after lockstep advance: {stderr}"
    );

    Ok(())
}

// A store index is a valid `evolve rename` target: a rename preserves the index's stable id
// and changes only its path. Discharge must not read the accepted old-path index as a drop
// while the same id is rebuilt under the new path, which would emit two verdicts for one id
// and surface a false `store.corruption`. Both preview and apply must succeed.
#[test]
fn evolve_rename_of_a_store_index_previews_and_applies_cleanly()
-> Result<(), Box<dyn std::error::Error>> {
    const INDEX_BASELINE_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         \x20   index byPages(pages, id)\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    const INDEX_RENAME_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         \x20   index byPageCount(pages, id)\n\
         evolve\n\
         \x20   rename ^books.byPages -> ^books.byPageCount\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";

    let root = native_books_project("evolve-rename-store-index", INDEX_BASELINE_SOURCE);
    commit_catalog(&root);
    write(&root, "src/books.mw", INDEX_RENAME_SOURCE);

    let preview = marrow(&[
        "evolve",
        "preview",
        "--format",
        "json",
        root.to_str().expect("project path utf-8"),
    ]);
    let preview_record = support::json(preview.stdout.clone());
    assert_ne!(
        preview_record["code"],
        serde_json::json!("store.corruption"),
        "renaming a store index must not surface a false store.corruption: {preview_record:#?}"
    );
    assert_eq!(preview.status.code(), Some(0), "{preview:?}");

    let apply = marrow(&[
        "evolve",
        "apply",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    Ok(())
}

#[test]
fn evolve_apply_advances_accepted_catalog_in_lockstep_for_rename()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-apply-rename-lockstep", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", RENAME_SOURCE);

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    let catalog = accepted_catalog(&root);
    assert_eq!(
        catalog.epoch,
        baseline_epoch + 1,
        "file advanced in lockstep"
    );
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // The renamed member keeps its stable id, records the new path, and leaves
    // the old spelling as an alias rather than a live path.
    let blurb = catalog
        .entries
        .iter()
        .find(|entry| entry.path == "books::Book::blurb")
        .expect("renamed member recorded at its new path");
    assert_eq!(
        blurb.stable_id, subtitle_id,
        "rename preserves the stable id"
    );
    assert!(
        catalog
            .entries
            .iter()
            .all(|entry| entry.path != "books::Book::subtitle"),
        "old path is not left as a live spelling"
    );
    assert!(
        blurb
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::subtitle"),
        "old path survives as an alias"
    );

    let run = marrow(&[
        "run",
        "--entry",
        "books::add",
        "--arg",
        "title=Dune",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        run.status.code(),
        Some(0),
        "run succeeds after lockstep advance: {run:?}"
    );
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after lockstep advance: {stderr}"
    );

    Ok(())
}

// After a rename apply, the rename is recorded in the accepted catalog. The evolve
// block is a transient transition the author may keep or delete; neither choice may
// break `marrow run`. The store fences on the durable shape, which a consumed rename
// block does not change, and the consumed rename is treated as satisfied at check.
#[test]
fn run_succeeds_after_rename_apply_with_block_present_or_deleted()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("run-after-rename-block", BLOCK_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;

    // The baseline run stamps the store and writes record 2; a subtitle cell on that
    // stamped record gives the later rename real data to carry forward.
    let baseline = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "baseline run: {baseline:?}"
    );
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &accepted_place,
            2,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }

    write(&root, "src/books.mw", RENAME_BLOCK_SOURCE);
    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "rename apply: {apply:?}");

    let kept = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "run with the consumed rename block still present: {kept:?}"
    );

    write(&root, "src/books.mw", RENAME_BLOCK_DELETED_SOURCE);
    let deleted = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "run after deleting the consumed rename block: {deleted:?}"
    );

    Ok(())
}

/// Durable never-reuse survives lock loss: a retired id lives in the store catalog as a
/// reserved entry, so the committed lock's id ledger is re-derivable from the store alone.
/// After a retire, deleting `marrow.lock` and re-opening on a write (commit) path recovers the
/// retired id into the re-projected lock's ledger — recovered from the store, not the deleted
/// lock — and it is never reissued as an active entry.
#[test]
fn a_retired_id_is_recovered_from_the_store_after_lock_loss_and_never_reissued()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("retire-lock-loss-converges", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    write(&root, "src/books.mw", RETIRE_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--no-backup",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(0), "retire apply: {apply:?}");

    // The retire reserved the id in the store catalog and projected it into the lock's
    // append-only ledger, never as an active lock entry.
    let lock_after_retire = support_evolve::committed_lock(&root).expect("lock after retire");
    assert!(
        lock_after_retire
            .ledger
            .iter()
            .any(|tombstone| tombstone.id == subtitle_id),
        "the retired id is a ledger tombstone in the projected lock"
    );
    assert!(
        lock_after_retire
            .entries
            .iter()
            .all(|entry| entry.stable_id != subtitle_id),
        "the retired id is not an active lock entry"
    );

    // Lose the lock entirely, then re-open on a write path so the store re-projects it.
    fs::remove_file(root.join("marrow.lock")).expect("delete the committed lock");
    let run = marrow(&[
        "run",
        "--entry",
        "books::add",
        "--arg",
        "title=Dune",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        run.status.code(),
        Some(0),
        "post-loss run re-projects: {run:?}"
    );

    // The re-projected lock recovered the retired id from the store catalog's reserved
    // entry, even though the lock file that previously recorded it was deleted.
    let recovered = support_evolve::committed_lock(&root).expect("lock re-projected after loss");
    assert!(
        recovered
            .ledger
            .iter()
            .any(|tombstone| tombstone.id == subtitle_id),
        "the retired id is recovered from the store into the re-projected ledger"
    );
    assert!(
        recovered
            .entries
            .iter()
            .all(|entry| entry.stable_id != subtitle_id),
        "the recovered retired id is never reissued as an active entry"
    );

    Ok(())
}

/// Durable never-reuse survives STORE loss when only the committed lock remains: re-seeding a
/// fresh store from the surviving lock must materialize the lock's tombstoned identity as a
/// reserved store entry, so the never-reuse defense is not silently lost across store loss.
///
/// Retire `subtitle` so a reserved store entry and a lock ledger tombstone exist, then capture
/// the committed lock. WIPE the store, re-seed a fresh checkout that carries only source +
/// the surviving lock through an ordinary `marrow run`. After the re-seed:
/// (a) the regenerated lock STILL carries the tombstone (the ledger is not emptied);
/// (b) re-declaring the retired `(kind, path)` in source FAILS CLOSED with the same typed
///     `check.catalog_intent` reserved-path-reuse diagnostic the live store gives; and
/// (c) the retired id is never handed back as a live active identity in the re-seeded store.
#[test]
fn a_retired_id_survives_store_loss_and_reseed_from_the_committed_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("retire-store-loss-reseed", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    write(&root, "src/books.mw", RETIRE_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--no-backup",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(0), "retire apply: {apply:?}");

    // The committed lock now records the retired id as a ledger tombstone, never as an active
    // entry. This is the surviving artifact a fresh checkout is seeded from.
    let lock_after_retire =
        fs::read_to_string(root.join("marrow.lock")).expect("committed lock after retire");
    let parsed_after_retire =
        support_evolve::committed_lock(&root).expect("lock after retire parses");
    assert!(
        parsed_after_retire
            .ledger
            .iter()
            .any(|tombstone| tombstone.id == subtitle_id),
        "precondition: the retired id is a ledger tombstone in the committed lock"
    );

    // A fresh checkout: only the post-retire source (subtitle gone, the consumed `evolve`
    // block deleted) and the surviving committed lock, no store. Mimics cloning a repo whose
    // store was never committed, then re-seeding it from the lock.
    const RETIRED_NO_BLOCK_SOURCE: &str = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let reseed_root =
        native_books_project("retire-store-loss-reseed-fresh", RETIRED_NO_BLOCK_SOURCE);
    write(&reseed_root, "marrow.lock", &lock_after_retire);

    let run = marrow(&[
        "run",
        "--entry",
        "books::add",
        "--arg",
        "title=Dune",
        reseed_root.to_str().unwrap(),
    ]);
    assert_eq!(
        run.status.code(),
        Some(0),
        "re-seed from the surviving lock runs: {run:?}"
    );

    // Oracle (a): the regenerated lock STILL carries the tombstone — the never-reuse defense
    // survived the store loss rather than projecting an empty ledger.
    let reseeded_lock =
        support_evolve::committed_lock(&reseed_root).expect("lock after re-seed parses");
    assert!(
        reseeded_lock
            .ledger
            .iter()
            .any(|tombstone| tombstone.id == subtitle_id),
        "the retired id survives store loss in the re-projected ledger: {:#?}",
        reseeded_lock.ledger
    );
    assert!(
        reseeded_lock
            .entries
            .iter()
            .all(|entry| entry.stable_id != subtitle_id),
        "the retired id is never re-projected as an active lock entry"
    );

    // Oracle (c): the re-seeded store carries the retired id only as a Reserved entry, never as
    // a live active identity handed back at the retired path.
    let reseeded_catalog = accepted_catalog(&reseed_root);
    let reserved = reseeded_catalog
        .entries
        .iter()
        .find(|entry| entry.stable_id == subtitle_id)
        .expect("the re-seeded store carries the retired id as a reserved entry");
    assert_eq!(
        reserved.lifecycle,
        marrow_catalog::CatalogLifecycle::Reserved,
        "the retired id rests reserved in the re-seeded store, never re-minted active"
    );

    // Oracle (b): re-declaring the retired `(kind, path)` over the re-seeded store FAILS CLOSED
    // with the same typed `check.catalog_intent` reserved-path-reuse diagnostic the live store
    // gives — the never-reuse contract holds against a store re-seeded from the lock.
    write(&reseed_root, "src/books.mw", RETIRE_BASELINE_SOURCE);
    let reuse = marrow(&["check", "--format", "json", reseed_root.to_str().unwrap()]);
    assert_eq!(
        reuse.status.code(),
        Some(1),
        "re-declaring a retired path over the re-seeded store must fail closed: {reuse:?}"
    );
    let report = support::json(reuse.stdout);
    let diagnostics = report["diagnostics"]
        .as_array()
        .expect("check diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == serde_json::json!("check.catalog_intent")),
        "re-declaring a reserved path surfaces the typed reserved-reuse code: {report:#?}"
    );

    Ok(())
}

// After a retire apply, the retire is recorded in the accepted catalog. The evolve
// block is transient; keeping or deleting it must not break `marrow run`.
#[test]
fn run_succeeds_after_retire_apply_with_block_present_or_deleted()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("run-after-retire-block", BLOCK_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle")?;

    // The baseline run stamps the store and writes record 2; a subtitle cell on that
    // stamped record gives the later retire one populated cell to approve.
    let baseline = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        baseline.status.code(),
        Some(0),
        "baseline run: {baseline:?}"
    );
    {
        let store = open_native_store(&root);
        seed_member(
            &store,
            &accepted_place,
            2,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }

    write(&root, "src/books.mw", RETIRE_BLOCK_SOURCE);
    let apply = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        "--no-backup",
        root.to_str().expect("project path utf-8"),
    ]);
    assert_eq!(apply.status.code(), Some(0), "retire apply: {apply:?}");

    let kept = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        kept.status.code(),
        Some(0),
        "run with the consumed retire block still present: {kept:?}"
    );

    write(&root, "src/books.mw", RETIRE_BLOCK_DELETED_SOURCE);
    let deleted = marrow(&["run", "--entry", "books::seed", root.to_str().unwrap()]);
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "run after deleting the consumed retire block: {deleted:?}"
    );

    Ok(())
}
