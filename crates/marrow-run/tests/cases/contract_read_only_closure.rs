use crate::support;
use marrow_check::{CheckedFunctionRef, EntryStoreOpenMode, check_project_with_catalog};
use marrow_run::Value;
use marrow_store::{AccessMode, SealedStore};

use support::{TempDir, run_entry, test_project_config, write_temp_source};

fn function_ref(
    program: &marrow_check::CheckedProgram,
    module: &str,
    function: &str,
) -> CheckedFunctionRef {
    let module_id = program.facts.module_id(module).expect("module");
    let function_id = program
        .facts
        .function_id(module_id, function)
        .expect("function");
    let fact = program.facts.function(function_id);
    CheckedFunctionRef {
        module: fact.module.0,
        function: fact.source_index,
    }
}

#[test]
fn read_only_closure_runs_against_native_read_only_store_after_identity_is_frozen() {
    let root = TempDir::new("marrow-read-only-closure").expect("temp project");
    let source = "module app\n\
        resource Book\n\
        \x20   required title: string\n\
        store ^books(id: int): Book\n\
        pub fn seed()\n\
        \x20   transaction\n\
        \x20       ^books(1).title = \"Mort\"\n\
        pub fn title(): string\n\
        \x20   return ^books(1).title ?? \"\"\n";
    write_temp_source(root.path(), std::path::Path::new("src/app.mw"), source);
    let config = test_project_config();

    let (report, pending_program) =
        marrow_check::check_project(root.path(), &config).expect("pending check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let title_ref = function_ref(&pending_program, "app", "title");
    assert_eq!(
        pending_program.entry_store_open_mode(title_ref),
        Some(EntryStoreOpenMode::WriteCapable),
        "a read-only closure still needs a write-capable first open before catalog identity is frozen"
    );

    let store_path = root.path().join("marrow.redb");
    {
        let store = SealedStore::open(&store_path, AccessMode::Create)
            .expect("open native store writable")
            .into_store();
        marrow_run::evolution::commit_catalog_baseline(&store, &pending_program)
            .expect("commit catalog baseline");
    }
    let accepted = SealedStore::open(&store_path, AccessMode::Read)
        .expect("open baseline store read-only")
        .into_store()
        .read_catalog_snapshot()
        .expect("read catalog snapshot");
    let (report, program) = check_project_with_catalog(root.path(), &config, accepted.as_ref())
        .expect("accepted check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let runtime = program.runtime();
    {
        let store = SealedStore::open(&store_path, AccessMode::Create)
            .expect("reopen writable for seed")
            .into_store();
        run_entry(&store, checked_entry!(&runtime, "app::seed")).expect("seed record");
    }

    let title_ref = function_ref(&program, "app", "title");
    assert_eq!(
        program.entry_store_open_mode(title_ref),
        Some(EntryStoreOpenMode::ReadOnly)
    );
    let read_only = SealedStore::open(&store_path, AccessMode::Read)
        .expect("open read-only store")
        .into_store();
    let output = run_entry(&read_only, checked_entry!(&runtime, "app::title")).expect("read title");
    assert_eq!(output.value, Some(Value::Str("Mort".to_string())));
}
