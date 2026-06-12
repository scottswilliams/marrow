//! `marrow check` is read-only over durable state: it neither freezes durable identity
//! nor creates or mutates the saved-data store. When a store already holds a committed
//! accepted catalog, `check` may repair `marrow.catalog.json` from that snapshot. The
//! durable write paths — `marrow run` over a persistent store and `marrow evolve apply`
//! — are the contrast: each commits, so each leaves the catalog artifact and store
//! changed.

use std::fs;
use std::path::Path;

use marrow_store::tree::TreeStore;

mod support;
mod support_evolve;

use support::{marrow, native_config, temp_project_uncommitted, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, accepted_catalog,
    commit_catalog, native_books_project, native_store_path, open_native_store, root_place,
    seed_title_only, store_epoch,
};

/// The canonical native-store seed source: a `Counter` resource whose `seed`
/// transaction writes one record. Declared inline here rather than reused from the
/// runtime corpus because this suite needs a `module`-bearing source file.
const COUNTER_SOURCE: &str = "module app\n\
     \n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     \n\
     pub fn seed()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 42\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n";

fn catalog_path(root: &Path) -> std::path::PathBuf {
    root.join("marrow.catalog.json")
}

fn store_path(root: &Path) -> std::path::PathBuf {
    root.join(".data").join("marrow.redb")
}

fn store_snapshot(root: &Path) -> marrow_catalog::CatalogMetadata {
    let store = TreeStore::open_read_only(&store_path(root)).expect("open store read-only");
    store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("project has an accepted catalog")
}

fn rendered_catalog(root: &Path) -> String {
    fs::read_to_string(catalog_path(root)).expect("read rendered catalog")
}

#[test]
fn check_on_an_uncommitted_project_writes_no_catalog_and_no_store() {
    // A project whose durable identity is not yet frozen checks cleanly and reports
    // informationally, but `check` must not be the command that establishes durable
    // state: it leaves the catalog file and the store absent.
    let project = temp_project_uncommitted("check-ro-uncommitted", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();

    assert!(!catalog_path(&project).exists(), "no catalog before check");
    assert!(!store_path(&project).exists(), "no store before check");

    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert!(
        !catalog_path(&project).exists(),
        "check must not freeze durable identity into the catalog file"
    );
    assert!(
        !store_path(&project).exists(),
        "check must not create the saved-data store"
    );
}

#[test]
fn run_freezes_the_catalog_into_the_store_and_renders_the_file() {
    // The contrast for the uncommitted case: `run` over a persistent store is a durable
    // write path, so the same project that `check` left untouched gains a store snapshot,
    // a created store, and a rendered catalog file the first time it runs.
    let project = temp_project_uncommitted("check-ro-run-commits", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();

    let run = marrow(&["run", "--entry", "app::seed", dir]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");

    assert!(
        store_path(&project).exists(),
        "run creates the saved-data store and commits the seeded record"
    );
    let store = TreeStore::open(&store_path(&project)).expect("open store after run");
    let snapshot = store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("run publishes the accepted catalog into the store");
    assert_eq!(
        rendered_catalog(&project),
        snapshot.to_json_pretty(),
        "run renders marrow.catalog.json from the committed store snapshot"
    );
}

#[test]
fn hostile_config_rejection_creates_no_native_store() {
    let project = temp_project_uncommitted("hostile-config-no-store", |root| {
        write(
            root,
            "marrow.json",
            "{ \"sourceRoots\": [\"src\\u0000evil\"], \"store\": { \"backend\": \"native\", \"dataDir\": \".data\" }, \"run\": { \"defaultEntry\": \"app::seed\" } }",
        );
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();

    let run = marrow(&["run", dir]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");

    assert!(
        !catalog_path(&project).exists(),
        "a hostile config must not create a catalog file"
    );
    assert!(
        !store_path(&project).exists(),
        "a hostile config must fail before creating the native store"
    );
}

#[test]
fn check_rejects_catalog_file_conflict_markers_without_creating_a_store() {
    let project = temp_project_uncommitted("check-ro-conflicted-catalog", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
        write(
            root,
            "marrow.catalog.json",
            "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> branch\n",
        );
    });
    let dir = project.to_str().unwrap();

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "conflicted catalog file must fail: {check:?}"
    );
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("catalog.merge_conflict")
            && stderr.contains("resolve the conflict")
            && stderr.contains("rerun the command"),
        "the error is typed and actionable: {stderr}"
    );
    assert!(
        !store_path(&project).exists(),
        "rejecting a conflicted catalog file must not create a store"
    );
}

#[test]
fn check_repairs_a_torn_catalog_file_from_the_committed_store_snapshot() {
    let project = temp_project_uncommitted("check-ro-torn-catalog-repair", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );
    let snapshot = store_snapshot(&project);
    fs::write(catalog_path(&project), "{\"epoch\":").expect("write torn catalog render");

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "check repairs the torn catalog file and proceeds: {check:?}"
    );

    assert_eq!(
        rendered_catalog(&project),
        snapshot.to_json_pretty(),
        "the repair replaces torn file bytes with the committed store snapshot"
    );
}

#[test]
fn check_rejects_catalog_file_conflict_markers_even_when_a_store_snapshot_exists() {
    let project = temp_project_uncommitted("check-ro-conflicted-catalog-with-store", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );
    fs::write(
        catalog_path(&project),
        "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> branch\n",
    )
    .expect("write conflicted catalog file");

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "conflicted catalog file must fail: {check:?}"
    );
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("catalog.merge_conflict")
            && stderr.contains("resolve the conflict")
            && stderr.contains("rerun the command"),
        "the error is typed and actionable: {stderr}"
    );
}

#[test]
fn check_on_a_committed_project_repairs_only_the_catalog_file() {
    // Once durable state exists, `check` can repair the derived catalog artifact, but the
    // store file's bytes are identical before and after, so a CI `check` cannot drift the
    // committed durable state.
    let project = temp_project_uncommitted("check-ro-committed", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );

    let store = store_path(&project);
    let store_before = fs::read(&store).expect("read store");
    let snapshot = store_snapshot(&project);
    if catalog_path(&project).exists() {
        fs::remove_file(catalog_path(&project)).expect("remove rendered catalog");
    }

    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert_eq!(
        fs::read(&store).expect("read store"),
        store_before,
        "check left the store file bytes unchanged"
    );
    assert_eq!(
        rendered_catalog(&project),
        snapshot.to_json_pretty(),
        "check repaired the catalog artifact from the committed store snapshot"
    );
}

#[test]
fn check_repairs_same_epoch_catalog_file_drift_from_the_committed_store_snapshot() {
    let project_a = temp_project_uncommitted("check-ro-same-epoch-drift-a", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let project_b = temp_project_uncommitted("check-ro-same-epoch-drift-b", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });

    let dir_a = project_a.to_str().unwrap();
    let dir_b = project_b.to_str().unwrap();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir_a])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir_b])
            .status
            .code(),
        Some(0)
    );

    let snapshot_a = store_snapshot(&project_a);
    let snapshot_b = store_snapshot(&project_b);
    assert_eq!(snapshot_a.epoch, snapshot_b.epoch);
    assert_ne!(
        snapshot_a.digest, snapshot_b.digest,
        "independent baseline catalogs must carry distinct identity"
    );
    fs::copy(catalog_path(&project_b), catalog_path(&project_a))
        .expect("copy project B catalog over project A");

    let check = marrow(&["check", dir_a]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "check repairs same-epoch file drift and proceeds: {check:?}"
    );

    assert_eq!(
        rendered_catalog(&project_a),
        snapshot_a.to_json_pretty(),
        "same-epoch drift is repaired from project A's committed store snapshot"
    );
}

#[test]
fn check_preserves_a_valid_catalog_file_ahead_of_the_local_store() {
    let project = temp_project_uncommitted("check-ro-file-ahead-store-behind", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", COUNTER_SOURCE);
    });
    let dir = project.to_str().unwrap();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );

    let store_epoch_one = store_snapshot(&project);
    assert_eq!(store_epoch_one.epoch, 1);
    let file_epoch_two = marrow_catalog::CatalogMetadata::new(
        store_epoch_one.epoch + 1,
        store_epoch_one.entries.clone(),
    );
    fs::write(catalog_path(&project), file_epoch_two.to_json_pretty())
        .expect("write later committed catalog artifact");

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "check binds against the file artifact without rewinding it: {check:?}"
    );
    assert_eq!(
        rendered_catalog(&project),
        file_epoch_two.to_json_pretty(),
        "a valid file artifact ahead of the local store is not repaired backward"
    );

    let run = marrow(&["run", "--entry", "app::seed", dir]);
    assert_eq!(
        run.status.code(),
        Some(1),
        "a write path must fence the older local store: {run:?}"
    );
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("run.store_behind") && stderr.contains("marrow evolve apply"),
        "store-behind guidance is typed and actionable: {stderr}"
    );
    assert_eq!(
        rendered_catalog(&project),
        file_epoch_two.to_json_pretty(),
        "the store-behind fence does not rewind the committed file artifact"
    );
}

#[test]
fn evolve_apply_advances_the_committed_catalog_and_store() {
    // The contrast for the committed case: `evolve apply` is the durable write path that
    // a check must not be. It advances the accepted catalog epoch and stamps the store,
    // so the two surfaces are not interchangeable.
    let root = native_books_project("check-ro-evolve-apply", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    // The seeded store sits at its baseline epoch. A plain source-only check over the
    // changed source passes and leaves the store epoch where the baseline put it: check
    // is not the surface that advances a store.
    assert_eq!(
        marrow(&["check", root.to_str().unwrap()]).status.code(),
        Some(0)
    );
    assert_eq!(
        store_epoch(&root),
        Some(baseline_epoch),
        "check did not advance the store epoch past the baseline"
    );

    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    assert_eq!(
        accepted_catalog(&root).epoch,
        baseline_epoch + 1,
        "apply advanced the accepted catalog epoch"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit")
            .map(|commit| commit.catalog_epoch),
        Some(baseline_epoch + 1),
        "apply stamped the store with the new epoch"
    );
}
