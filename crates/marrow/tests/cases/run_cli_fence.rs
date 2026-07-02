use crate::{support, support_evolve};
use std::fs;

use marrow_store::key::SavedKey;
use marrow_store::tree::EngineProfile;
use marrow_store::value::Scalar;
use marrow_store::{AccessMode, SealedStore};
use support::{marrow_sub, temp_project, temp_project_uncommitted, write};

/// A store whose commit metadata records a different catalog epoch than its accepted
/// catalog snapshot is internally inconsistent: every write path stamps both together in
/// one transaction, so no commit or evolution can produce this state. `marrow run` fails
/// closed as `store.corruption` before any execution, so no program output reaches stdout.
#[test]
fn commit_metadata_epoch_ahead_of_the_snapshot_fails_closed_as_corruption() {
    let root = temp_project("run-fence-stale", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Counter\n\
             \x20\x20\x20\x20required value: int\n\
             store ^counter(id: int): Counter\n\
             \n\
             pub fn show()\n\
             \x20\x20\x20\x20print(\"ran the entry\")\n",
        );
    });
    // The accepted catalog the fixture wrote sits at epoch 1; stamp the commit metadata one
    // epoch ahead of it, with this binary's engine profile, so the store is internally
    // inconsistent (a state no real commit or evolution produces).
    let store_path = root.join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    {
        let store = SealedStore::open(&store_path, AccessMode::Create)
            .expect("open native store")
            .into_store();
        let profile = marrow_run::evolution::current_engine_profile();
        store
            .write_commit_metadata(&marrow_store::tree::CommitMetadata {
                commit_id: 0,
                catalog_epoch: 2,
                layout_epoch: profile.layout_epoch(),
                source_digest:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000001"
                        .to_string(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
            })
            .expect("stamp commit metadata");
    }

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("store.corruption"), "{stderr}");
    // The entry never ran: the fence fires before execution, so nothing prints.
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "");
}

#[test]
fn run_is_fenced_when_store_engine_profile_drifts() {
    let root = temp_project("run-fence-engine-profile", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Counter\n\
             \x20\x20\x20\x20required value: int\n\
             store ^counter(id: int): Counter\n\
             \n\
             pub fn show()\n\
             \x20\x20\x20\x20print(\"ran the entry\")\n",
        );
    });
    let store_path = root.join(".data").join("marrow.redb");
    {
        let store = SealedStore::open(&store_path, AccessMode::Create)
            .expect("open native store")
            .into_store();
        let mut commit = store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("fixture store is stamped");
        let drifted_profile =
            EngineProfile::new(marrow_run::evolution::current_engine_profile().layout_epoch() + 1);
        commit.layout_epoch = drifted_profile.layout_epoch();
        commit.engine_profile_digest = drifted_profile.digest_bytes();
        store
            .write_commit_metadata(&commit)
            .expect("stamp drifted engine profile");
    }

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("run.engine_profile"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "");
}

#[test]
fn run_rejects_populated_unstamped_accepted_store() {
    let root = temp_project_uncommitted("run-fence-unstamped", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Counter\n\
             \x20\x20\x20\x20required value: int\n\
             store ^counter(id: int): Counter\n\
             pub fn show()\n\
             \x20\x20\x20\x20if const value = ^counter(1).value\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print($\"value={value}\")\n",
        );
    });
    let config_text = fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    // Bind the program against its own proposal so the seeded cells use accepted catalog
    // ids, then publish the catalog rows without the epoch stamp to reproduce a
    // populated-but-unstamped store.
    let (_proposal_report, proposal_program) =
        marrow_check::check_project_with_catalog(root.path(), &config, None).expect("propose");
    let proposal = proposal_program
        .catalog
        .proposal
        .clone()
        .expect("a catalog proposal");
    let (report, program) =
        marrow_check::check_project_with_catalog(root.path(), &config, Some(&proposal))
            .expect("check against proposal");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = support_evolve::root_place(&program, "counter").expect("checked place");
    let store_path = root.join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    let store_id = support_evolve::store_catalog_id(&place).expect("store catalog id");
    {
        let store = SealedStore::open(&store_path, AccessMode::Create)
            .expect("open native store")
            .into_store();
        store
            .replace_catalog_snapshot(&proposal)
            .expect("publish accepted catalog without epoch stamp");
        support_evolve::seed_record_member_value(
            &store,
            &place,
            &[SavedKey::Int(1)],
            "value",
            Scalar::Int(7),
        );
    }
    {
        let store = SealedStore::open(&store_path, AccessMode::Read)
            .expect("reopen store")
            .into_store();
        assert!(
            store
                .read_catalog_snapshot()
                .expect("read accepted catalog")
                .is_some()
        );
        assert_eq!(store.read_commit_metadata().expect("read commit"), None);
        assert_eq!(store.read_store_uid().expect("read store uid"), None);
        assert!(
            store
                .record_identity_exists_under(&store_id, &[], place.identity_keys.len())
                .expect("arity-aware presence")
        );
    }

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("run.store_unstamped"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "");
}

#[test]
fn run_rejects_composite_root_in_populated_unstamped_accepted_store() {
    let root = temp_project_uncommitted("run-fence-composite-unstamped", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             resource Pair\n\
             \x20\x20\x20\x20required value: int\n\
             store ^pairs(left: int, right: int): Pair\n\
             pub fn show()\n\
             \x20\x20\x20\x20if const value = ^pairs(1, 2).value\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print($\"value={value}\")\n",
        );
    });
    let config_text = fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    let (_proposal_report, proposal_program) =
        marrow_check::check_project_with_catalog(root.path(), &config, None).expect("propose");
    let proposal = proposal_program
        .catalog
        .proposal
        .clone()
        .expect("a catalog proposal");
    let (report, program) =
        marrow_check::check_project_with_catalog(root.path(), &config, Some(&proposal))
            .expect("check against proposal");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let place = support_evolve::root_place(&program, "pairs").expect("checked place");
    let store_path = root.join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    let store_id = support_evolve::store_catalog_id(&place).expect("store catalog id");
    let identity = [SavedKey::Int(1), SavedKey::Int(2)];
    {
        let store = SealedStore::open(&store_path, AccessMode::Create)
            .expect("open native store")
            .into_store();
        store
            .replace_catalog_snapshot(&proposal)
            .expect("publish accepted catalog without epoch stamp");
        support_evolve::seed_record_member_value(
            &store,
            &place,
            &identity,
            "value",
            Scalar::Int(9),
        );
    }
    {
        let store = SealedStore::open(&store_path, AccessMode::Read)
            .expect("reopen store")
            .into_store();
        assert!(
            store
                .read_catalog_snapshot()
                .expect("read accepted catalog")
                .is_some()
        );
        assert_eq!(store.read_commit_metadata().expect("read commit"), None);
        assert_eq!(store.read_store_uid().expect("read store uid"), None);
        assert_eq!(
            store
                .record_child_count(&store_id, &[])
                .expect("raw root child count"),
            0
        );
        assert!(
            store
                .record_identity_exists_under(&store_id, &[], place.identity_keys.len())
                .expect("arity-aware presence")
        );
    }

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("run.store_unstamped"), "{stderr}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "");
}
