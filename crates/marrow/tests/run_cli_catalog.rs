//! `marrow run` establishes durable identity in the engine-resident store: the first
//! run over a native store freezes the pending catalog into the store (never a
//! `marrow.catalog.json` file), and a second run over the now-accepted catalog churns
//! nothing — the same catalog rows, epoch, and commit stamp.

mod support;

use marrow_store::tree::TreeStore;
use support::{TempProject, find_code_segment, marrow_sub, write};

/// A native-store project with a saved root but no committed catalog: checking it
/// proposes durable identity that no flow has frozen yet. Built without committing
/// so the catalog stays pending until a run commits it.
fn pending_native_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn main()\n\
             \x20   print(\"ran\")\n",
        );
    })
}

fn store_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".data").join("marrow.redb")
}

#[test]
fn run_commits_the_pending_catalog_into_the_store_not_a_file() {
    let root = pending_native_project("run-pending-commit");
    let catalog_file = root.join("marrow.catalog.json");

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ran\n");

    // No production write to the accepted-catalog file: identity is engine-resident.
    assert!(
        !catalog_file.exists(),
        "run must not write a marrow.catalog.json file"
    );

    // The store now publishes the accepted catalog at the baseline epoch, with an entry
    // for the saved `^books` root.
    let store = TreeStore::open(&store_path(&root)).expect("open store after run");
    let snapshot = store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("first run publishes an accepted catalog into the store");
    assert_eq!(snapshot.epoch, 1, "baseline epoch is 1");
    assert!(
        snapshot
            .entries
            .iter()
            .any(|entry| entry.path.contains("books")),
        "the accepted catalog holds the ^books store entry: {:#?}",
        snapshot.entries
    );
    assert_eq!(
        store.read_catalog_epoch().expect("store epoch"),
        Some(1),
        "the store is stamped at the baseline catalog epoch"
    );
}

#[test]
fn a_second_run_does_not_churn_the_accepted_catalog() {
    let root = pending_native_project("run-accepted-noop");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let (digest_one, commit_one) = {
        let store = TreeStore::open(&store_path(&root)).expect("open store after first run");
        (
            store.catalog_snapshot_digest().expect("snapshot digest"),
            store.read_commit_metadata().expect("commit metadata"),
        )
    };

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    let (digest_two, commit_two) = {
        let store = TreeStore::open(&store_path(&root)).expect("open store after second run");
        (
            store.catalog_snapshot_digest().expect("snapshot digest"),
            store.read_commit_metadata().expect("commit metadata"),
        )
    };

    assert_eq!(
        digest_one, digest_two,
        "the catalog rows must not change on a second run"
    );
    assert_eq!(
        commit_one, commit_two,
        "the commit metadata must not advance on a second run over an accepted catalog"
    );
}

#[test]
fn a_memory_backed_durable_baseline_fails_with_a_typed_error() {
    // A project whose source declares a durable surface (a saved root) but configures no
    // persistent store has identity nothing can hold. It must fail closed rather than run
    // with an identity nothing stamps.
    let root = support::temp_project_uncommitted("run-memory-durable", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "app::main" } }"#,
        );
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn main()\n\
             \x20   print(\"ran\")\n",
        );
    });

    let output = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    let segments: Vec<&str> = stderr.trim().split(": ").collect();
    let (_, code) = find_code_segment(&segments);
    assert_eq!(
        code, "run.durable_store_required",
        "a memory-backed durable baseline reports the typed error: {stderr}"
    );
}

#[test]
fn a_plain_script_runs_over_memory_with_no_baseline() {
    // A script with no resources, stores, or enums proposes no catalog, so it runs over
    // the throwaway memory store and needs no durable identity.
    let root = support::temp_project_uncommitted("run-memory-script", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "run": { "defaultEntry": "main" } }"#,
        );
        write(root, "src/app.mw", "pub fn main()\n\x20   print(\"ran\")\n");
    });

    let output = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ran\n");
}
