//! `marrow run` establishes durable identity by committing the pending catalog into the
//! store transaction, then rendering `marrow.catalog.json` from that committed snapshot.
//! The file is the source-tree artifact; the store copy is the crash bridge. A second run
//! over the now-accepted catalog churns nothing: the same catalog rows, file bytes, epoch,
//! and commit stamp.
use crate::support;
use std::fs;

use marrow_catalog::CatalogMetadata;
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

fn catalog_file(root: &std::path::Path) -> std::path::PathBuf {
    root.join("marrow.catalog.json")
}

fn store_snapshot(root: &std::path::Path) -> CatalogMetadata {
    let store = TreeStore::open(&store_path(root)).expect("open store");
    store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("project has an accepted catalog")
}

fn rendered_catalog(root: &std::path::Path) -> String {
    fs::read_to_string(catalog_file(root)).expect("read rendered catalog")
}

#[test]
fn run_commits_the_pending_catalog_into_the_store_and_renders_the_file() {
    let root = pending_native_project("run-pending-commit");

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ran\n");

    // The store publishes the accepted catalog at the baseline epoch, with an entry for
    // the saved `^books` root; the source-tree artifact is a render of those committed
    // rows, not an independently generated proposal.
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
        store
            .read_commit_metadata()
            .expect("read commit")
            .map(|commit| commit.catalog_epoch),
        Some(1),
        "the store is stamped at the baseline catalog epoch"
    );
    assert_eq!(
        rendered_catalog(&root),
        snapshot.to_json_pretty(),
        "marrow.catalog.json is rendered from the committed store snapshot"
    );
}

#[test]
fn a_second_run_does_not_churn_the_accepted_catalog() {
    let root = pending_native_project("run-accepted-noop");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let (digest_one, commit_one, file_one) = {
        let store = TreeStore::open(&store_path(&root)).expect("open store after first run");
        (
            store.catalog_snapshot_digest().expect("snapshot digest"),
            store.read_commit_metadata().expect("commit metadata"),
            rendered_catalog(&root),
        )
    };

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    let (digest_two, commit_two, file_two) = {
        let store = TreeStore::open(&store_path(&root)).expect("open store after second run");
        (
            store.catalog_snapshot_digest().expect("snapshot digest"),
            store.read_commit_metadata().expect("commit metadata"),
            rendered_catalog(&root),
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
    assert_eq!(
        file_one, file_two,
        "the rendered catalog file must stay idempotent on a second run"
    );
}

#[test]
fn run_repairs_a_missing_catalog_file_from_the_committed_store_snapshot() {
    let root = pending_native_project("run-repair-missing-catalog-file");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let snapshot = store_snapshot(&root);
    fs::remove_file(catalog_file(&root)).expect("simulate kill before file render");

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "run repairs the catalog file and proceeds: {second:?}"
    );
    assert_eq!(
        String::from_utf8(second.stdout).expect("stdout utf8"),
        "ran\n"
    );

    assert_eq!(
        rendered_catalog(&root),
        snapshot.to_json_pretty(),
        "the repair renders the exact committed store snapshot"
    );
}

#[test]
fn run_recreates_the_store_catalog_from_the_committed_file_when_the_store_is_missing() {
    let root = pending_native_project("run-recreate-store-catalog-from-file");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let committed_file = rendered_catalog(&root);
    fs::remove_dir_all(root.join(".data")).expect("simulate checkout without local store");

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "run recreates the store catalog from the committed file: {second:?}"
    );

    assert_eq!(
        store_snapshot(&root).to_json_pretty(),
        committed_file,
        "the recreated store catalog matches the committed source-tree artifact"
    );
    assert_eq!(
        rendered_catalog(&root),
        committed_file,
        "the file render stays idempotent"
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
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "app::main" } }"#,
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
            r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "main" } }"#,
        );
        write(root, "src/app.mw", "pub fn main()\n\x20   print(\"ran\")\n");
    });

    let output = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ran\n");
}
