use std::fs;

mod support;

use support::{marrow_sub, temp_project, temp_project_uncommitted, write};

/// A store stamped at a catalog epoch newer than the project's accepted epoch was
/// evolved by a newer binary. `marrow run` fences itself before any execution: it
/// reports `run.store_evolved` and never runs the entry, so no program output reaches
/// stdout.
#[test]
fn run_is_fenced_when_store_evolved_past_the_project_epoch() {
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
    // The accepted catalog the fixture wrote sits at epoch 1; stamp the on-disk store
    // one epoch ahead, with this binary's engine profile so only the epoch fences.
    let store_path = root.join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    {
        let store = marrow_store::tree::TreeStore::open(&store_path).expect("open native store");
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
            .expect("stamp commit metadata");
    }

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("run.store_evolved"), "{stderr}");
    // The entry never ran: the fence fires before execution, so nothing prints.
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
    let place = marrow_check::checked_saved_root_place(
        &program,
        "counter",
        marrow_syntax::SourceSpan::default(),
    )
    .expect("checked place");
    let store_path = root.join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    {
        let store = marrow_store::tree::TreeStore::open(&store_path).expect("open native store");
        store
            .replace_catalog_snapshot(&proposal)
            .expect("publish accepted catalog without epoch stamp");
        let store_id = marrow_store::cell::CatalogId::new(
            place.store_catalog_id.clone().expect("accepted store id"),
        )
        .expect("store catalog id");
        let value_id = marrow_store::cell::CatalogId::new(
            place
                .root_members
                .iter()
                .find(|member| member.name == "value")
                .expect("value member")
                .catalog_id
                .clone()
                .expect("accepted value member id"),
        )
        .expect("value catalog id");
        store
            .write_node(&store_id, &[marrow_store::key::SavedKey::Int(1)])
            .expect("write record");
        store
            .write_data_value(
                &store_id,
                &[marrow_store::key::SavedKey::Int(1)],
                &[marrow_store::tree::DataPathSegment::Member(value_id)],
                marrow_store::value::encode_value(&marrow_store::value::Scalar::Int(7))
                    .expect("encode value"),
            )
            .expect("write value");
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
    let place = marrow_check::checked_saved_root_place(
        &program,
        "pairs",
        marrow_syntax::SourceSpan::default(),
    )
    .expect("checked place");
    let store_path = root.join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().unwrap()).expect("create data dir");
    let store_id = marrow_store::cell::CatalogId::new(
        place.store_catalog_id.clone().expect("accepted store id"),
    )
    .expect("store catalog id");
    let value_id = marrow_store::cell::CatalogId::new(
        place
            .root_members
            .iter()
            .find(|member| member.name == "value")
            .expect("value member")
            .catalog_id
            .clone()
            .expect("accepted value member id"),
    )
    .expect("value catalog id");
    let identity = [
        marrow_store::key::SavedKey::Int(1),
        marrow_store::key::SavedKey::Int(2),
    ];
    {
        let store = marrow_store::tree::TreeStore::open(&store_path).expect("open native store");
        store
            .replace_catalog_snapshot(&proposal)
            .expect("publish accepted catalog without epoch stamp");
        store
            .write_node(&store_id, &identity)
            .expect("write record");
        store
            .write_data_value(
                &store_id,
                &identity,
                &[marrow_store::tree::DataPathSegment::Member(value_id)],
                marrow_store::value::encode_value(&marrow_store::value::Scalar::Int(9))
                    .expect("encode value"),
            )
            .expect("write value");
    }
    {
        let store =
            marrow_store::tree::TreeStore::open_read_only(&store_path).expect("reopen store");
        assert!(
            store
                .read_catalog_snapshot()
                .expect("read accepted catalog")
                .is_some()
        );
        assert_eq!(store.read_commit_metadata().expect("read commit"), None);
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
