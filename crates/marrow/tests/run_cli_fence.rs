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
             resource Counter at ^counter(id: int)\n\
             \x20\x20\x20\x20required value: int\n\
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
        store.write_catalog_epoch(2).expect("stamp newer epoch");
        store
            .write_engine_profile(&marrow_run::evolution::current_engine_profile())
            .expect("stamp profile");
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
             resource Counter at ^counter(id: int)\n\
             \x20\x20\x20\x20required value: int\n\
             pub fn show()\n\
             \x20\x20\x20\x20print($\"value={^counter(1).value}\")\n",
        );
    });
    let config_text = fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    // Bind the program against its own proposal so the seeded cells use the catalog ids a
    // baseline would later accept, then write them into a store left without a catalog stamp
    // to reproduce a populated-but-unstamped store.
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
