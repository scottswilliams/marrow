use crate::support;
use std::fs;
use std::path::Path;

use marrow_check::{ProjectConfig, StoreBackend, StoreConfig};
use marrow_run::{
    EntryArgument, EntryArgumentValue, EntryDescriptor, EntryInvocation, EntryScalarArgument, Host,
    ProjectMode, ProjectOpen, ProjectSession, RUN_ENTRY_ARGUMENT, SessionEntry,
};
use marrow_store::tree::TreeStore;
use support::{TempDir, write_temp_source};

fn native_config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: Some("shelf::show".into()),
        store: StoreConfig {
            backend: StoreBackend::Native,
            data_dir: Some(".data".into()),
        },
        tests: Vec::new(),
    }
}

fn write_native_config(root: &Path) {
    fs::write(
        root.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
    )
    .expect("write marrow.json");
}

fn baseline_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"baseline\")\n"
}

fn advanced_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     \x20\x20\x20\x20label: string\n\
     store ^counter(id: int): Counter\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"advanced\")\n"
}

fn persistent_counter_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     pub fn bump()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 1\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
     pub fn show()\n\
     \x20\x20\x20\x20if not exists(^counter(1))\n\
     \x20\x20\x20\x20\x20\x20\x20\x20print(\"absent\")\n\
     \x20\x20\x20\x20\x20\x20\x20\x20return\n\
     \x20\x20\x20\x20if const value = ^counter(1).value\n\
     \x20\x20\x20\x20\x20\x20\x20\x20print($\"value={value}\")\n"
}

fn invoke(session: &ProjectSession, entry: &str) -> String {
    let host = Host::new();
    let mut output = String::new();
    session
        .invoke(SessionEntry::new(entry, &host, &mut output))
        .expect("invoke session entry");
    output
}

#[test]
fn project_session_invokes_protocol_arguments() {
    let root = TempDir::new("marrow-run-session-protocol-args").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\
         pub fn echo(label: string, n: int): int\n\
         \x20\x20\x20\x20print(label)\n\
         \x20\x20\x20\x20return n\n",
    );
    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_entry_override("shelf::echo")
            .with_fresh_memory_store(),
    )
    .expect("open session");
    let host = Host::new();
    let mut output = String::new();

    let descriptor = EntryDescriptor::resolve(session.runtime_program(), "shelf::echo")
        .expect("entry descriptor");
    let result = session
        .invoke(SessionEntry::protocol(
            EntryInvocation {
                identity: descriptor.identity,
                arguments: vec![
                    EntryArgument {
                        name: "label".into(),
                        value: EntryArgumentValue::Scalar(EntryScalarArgument::String(
                            "typed".into(),
                        )),
                    },
                    EntryArgument {
                        name: "n".into(),
                        value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(7)),
                    },
                ],
            },
            &host,
            &mut output,
        ))
        .expect("protocol args invoke");

    assert_eq!(output, "typed\n");
    assert_eq!(result.value, Some(marrow_run::Value::Int(7)));
}

#[test]
fn project_session_rejects_stale_protocol_invocation_identity() {
    let stale = support::checked_program("module shelf\npub fn echo(n: int): int\n    return n\n");
    let stale = EntryDescriptor::resolve(&stale, "shelf::echo").expect("stale descriptor");
    let stale = EntryInvocation {
        identity: stale.identity,
        arguments: vec![EntryArgument {
            name: "n".into(),
            value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(7)),
        }],
    };
    let root = TempDir::new("marrow-run-session-stale-protocol-args").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\
         pub fn echo(label: string): string\n\
         \x20\x20\x20\x20return label\n",
    );
    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_entry_override("shelf::echo")
            .with_fresh_memory_store(),
    )
    .expect("open session");
    let host = Host::new();
    let mut output = String::new();

    let error = session
        .invoke(SessionEntry::protocol(stale, &host, &mut output))
        .expect_err("stale protocol descriptor should fail closed");

    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
    assert_eq!(output, "");
}

#[test]
fn fresh_memory_run_does_not_create_native_store_or_catalog_artifact() {
    let root = TempDir::new("marrow-run-session-fresh-memory").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open fresh-memory session");
    assert_eq!(session.run_entry(), Some("shelf::show"));
    assert!(
        session.store_stamp().expect("read store stamp").is_none(),
        "fresh-memory sessions do not expose durable store stamps"
    );

    let output = invoke(&session, "shelf::show");

    assert_eq!(output, "baseline\n");
    assert!(
        !root.path().join(".data").exists(),
        "fresh-memory session must not create the configured native data dir"
    );
    assert!(
        !root.path().join("marrow.catalog.json").exists(),
        "fresh-memory session must not freeze or render accepted catalog identity"
    );
}

#[test]
fn fresh_memory_run_does_not_read_or_advance_an_existing_native_store() {
    let root =
        TempDir::new("marrow-run-session-fresh-memory-existing-store").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        persistent_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::bump"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let catalog_path = root.path().join("marrow.catalog.json");
    let catalog_before = fs::read(&catalog_path).expect("seed run renders accepted catalog");
    let before = {
        let store = TreeStore::open_read_only(&store_path).expect("open real store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("seed run stamps the real store")
    };

    let fresh = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open fresh-memory session over existing native store");
    assert_eq!(
        invoke(&fresh, "shelf::show"),
        "absent\n",
        "fresh-memory run must not read the real native store"
    );
    assert_eq!(
        invoke(&fresh, "shelf::bump"),
        "",
        "fresh-memory writes must stay inside the in-memory session store"
    );

    let after = {
        let store = TreeStore::open_read_only(&store_path).expect("reopen real store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("real store remains stamped")
    };
    assert_eq!(
        before, after,
        "fresh-memory run must not advance the real store commit"
    );
    assert_eq!(
        catalog_before,
        fs::read(&catalog_path).expect("fresh-memory run preserves accepted catalog"),
        "fresh-memory run must not rewrite the accepted catalog artifact"
    );
}

#[test]
fn opening_a_store_behind_the_accepted_catalog_returns_the_typed_fence_code() {
    let root = TempDir::new("marrow-run-session-behind").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());
    let config = native_config();
    let store_path = root.path().join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create store dir");

    let baseline = {
        let (report, pending) =
            marrow_check::check_project_with_catalog(root.path(), &config, None)
                .expect("check baseline");
        assert!(!report.has_errors(), "{:#?}", report.diagnostics);
        let store = marrow_store::tree::TreeStore::open(&store_path).expect("open native store");
        assert!(
            marrow_run::evolution::commit_catalog_baseline(&store, &pending)
                .expect("commit baseline"),
            "baseline fixture should commit a first catalog"
        );
        store
            .read_catalog_snapshot()
            .expect("read baseline catalog")
            .expect("baseline catalog")
    };

    write_temp_source(root.path(), Path::new("src/shelf.mw"), advanced_source());
    let (report, advanced) =
        marrow_check::check_project_with_catalog(root.path(), &config, Some(&baseline))
            .expect("check advanced source");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let advanced_catalog = advanced
        .catalog
        .proposal
        .expect("advanced source proposes the next catalog");
    fs::write(
        root.path().join("marrow.catalog.json"),
        advanced_catalog.to_json_pretty().expect("catalog renders"),
    )
    .expect("write advanced accepted catalog");

    let error =
        ProjectSession::open(root.path(), ProjectMode::Run).expect_err("store behind is fenced");

    assert_eq!(error.code(), "run.store_behind");
}
