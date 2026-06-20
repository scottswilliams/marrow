//! `marrow check` is read-only over durable state: it neither freezes durable identity
//! nor creates, opens, repairs, or mutates the saved-data store. The durable write
//! paths — `marrow run` over a persistent store and `marrow evolve apply` — are the
//! contrast: each commits, so each re-projects the committed `marrow.lock` and advances
//! the store.

use std::fs;
use std::path::Path;

use crate::support;
use crate::support_evolve;
use marrow_store::tree::TreeStore;
use support::{counter_source, marrow, native_config, temp_project_uncommitted, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, accepted_catalog,
    commit_catalog, native_books_project, native_store_path, open_native_store, root_place,
    seed_title_only, store_epoch,
};

fn lock_path(root: &Path) -> std::path::PathBuf {
    root.join("marrow.lock")
}

fn store_path(root: &Path) -> std::path::PathBuf {
    root.join(".data").join("marrow.redb")
}

fn committed_lock(root: &Path) -> marrow_catalog::CatalogLock {
    marrow_check::read_committed_lock(root)
        .expect("read committed lock")
        .expect("project has a committed lock")
}

#[test]
fn check_on_an_uncommitted_project_writes_no_lock_and_no_store() {
    // A project whose durable identity is not yet frozen checks cleanly and reports
    // informationally, but `check` must not be the command that establishes durable
    // state: it leaves the committed lock and the store absent.
    let project = temp_project_uncommitted("check-ro-uncommitted", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");

    assert!(!lock_path(&project).exists(), "no lock before check");
    assert!(!store_path(&project).exists(), "no store before check");

    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert!(
        !lock_path(&project).exists(),
        "check must not freeze durable identity into the committed lock"
    );
    assert!(
        !store_path(&project).exists(),
        "check must not create the saved-data store"
    );
}

#[test]
fn check_does_not_open_a_hostile_native_store_file() {
    let project = temp_project_uncommitted("check-ro-hostile-store-file", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
        write(root, ".data/marrow.redb", "not a redb store");
    });
    let dir = project.to_str().expect("project path utf-8");
    let store_before = fs::read(store_path(&project)).expect("read hostile store");

    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert_eq!(
        fs::read(store_path(&project)).expect("read hostile store after check"),
        store_before,
        "ordinary check must not open or rewrite the configured store"
    );
    assert!(
        !lock_path(&project).exists(),
        "ordinary check must not repair lock state from the store"
    );
}

#[test]
fn run_freezes_the_catalog_into_the_store_and_reprojects_the_lock() {
    // The contrast for the uncommitted case: `run` over a persistent store is a durable
    // write path, so the same project that `check` left untouched gains a store snapshot,
    // a created store, and a re-projected committed lock the first time it runs.
    let project = temp_project_uncommitted("check-ro-run-commits", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");

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
    let lock = committed_lock(&project);
    assert_eq!(
        lock.epoch_high_water, snapshot.epoch,
        "run re-projects marrow.lock from the committed store snapshot"
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
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");

    let run = marrow(&["run", dir]);
    assert_eq!(run.status.code(), Some(1), "{run:?}");

    assert!(
        !lock_path(&project).exists(),
        "a hostile config must not create a committed lock"
    );
    assert!(
        !store_path(&project).exists(),
        "a hostile config must fail before creating the native store"
    );
}

#[test]
fn check_rejects_lock_conflict_markers_without_creating_a_store() {
    let project = temp_project_uncommitted("check-ro-conflicted-lock", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
        write(
            root,
            "marrow.lock",
            "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> branch\n",
        );
    });
    let dir = project.to_str().expect("project path utf-8");

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "conflicted lock must fail: {check:?}"
    );
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("catalog.lock_corrupt"),
        "the error is the typed lock-corrupt code: {stderr}"
    );
    assert!(
        !store_path(&project).exists(),
        "rejecting a conflicted lock must not create a store"
    );
}

#[test]
fn check_rejects_a_torn_lock_without_opening_the_store_snapshot() {
    let project = temp_project_uncommitted("check-ro-torn-lock-repair", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );
    let store_before = fs::read(store_path(&project)).expect("read store before check");
    fs::write(lock_path(&project), "{\"epoch\":").expect("write torn lock");

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "check rejects the torn lock without store repair: {check:?}"
    );
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("catalog.lock_corrupt"),
        "the torn lock surfaces the typed lock-corrupt code: {stderr}"
    );
    assert_eq!(
        fs::read(store_path(&project)).expect("read store after check"),
        store_before,
        "rejecting a torn lock must not open the store for repair"
    );
    assert_eq!(
        fs::read_to_string(lock_path(&project)).expect("read torn lock"),
        "{\"epoch\":",
        "ordinary check leaves invalid lock bytes for the user to fix"
    );
}

#[test]
fn check_rejects_lock_conflict_markers_even_when_a_store_snapshot_exists() {
    let project = temp_project_uncommitted("check-ro-conflicted-lock-with-store", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );
    fs::write(
        lock_path(&project),
        "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> branch\n",
    )
    .expect("write conflicted lock");

    let check = marrow(&["check", dir]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "conflicted lock must fail: {check:?}"
    );
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("catalog.lock_corrupt"),
        "the error is the typed lock-corrupt code: {stderr}"
    );
}

#[test]
fn check_on_a_committed_project_does_not_repair_a_missing_lock() {
    let project = temp_project_uncommitted("check-ro-committed", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", dir]).status.code(),
        Some(0)
    );

    let store = store_path(&project);
    let store_before = fs::read(&store).expect("read store");
    if lock_path(&project).exists() {
        fs::remove_file(lock_path(&project)).expect("remove committed lock");
    }

    let check = marrow(&["check", dir]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");

    assert_eq!(
        fs::read(&store).expect("read store"),
        store_before,
        "check left the store file bytes unchanged"
    );
    assert!(
        !lock_path(&project).exists(),
        "ordinary check does not reconstruct a missing lock from the store"
    );
}

#[test]
fn check_reports_a_stale_lock_when_the_source_digest_drifts() {
    // A valid committed lock whose recorded source digest no longer matches the current
    // source is stale: check is read-only, so it reports the typed stale-lock rather than
    // re-projecting, and it never opens or rewrites the store.
    let root = native_books_project("check-ro-stale-lock", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books").expect("books root place");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }
    let lock_before = committed_lock(&root);
    let store_before = fs::read(native_store_path(&root)).expect("read store before check");

    // Edit the source so its shape digest drifts from the lock's recorded digest.
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let check = marrow(&["check", root.to_str().unwrap()]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "a stale lock is a non-fatal advisory: check still succeeds: {check:?}"
    );
    let stderr = String::from_utf8(check.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("check.stale_lock"),
        "check surfaces the typed stale-lock advisory: {stderr}"
    );

    // Read-only: the store bytes are untouched and the committed lock is not re-projected.
    assert_eq!(
        fs::read(native_store_path(&root)).expect("read store after check"),
        store_before,
        "a stale-lock check must not open or rewrite the store"
    );
    assert_eq!(
        committed_lock(&root),
        lock_before,
        "a stale-lock check must not re-project the committed lock"
    );
}

#[test]
fn evolve_apply_advances_the_committed_lock_and_store() -> Result<(), Box<dyn std::error::Error>> {
    // The contrast for the committed case: `evolve apply` is the durable write path that
    // a check must not be. It advances the accepted catalog epoch, stamps the store, and
    // re-projects the committed lock, so the two surfaces are not interchangeable.
    let root = native_books_project("check-ro-evolve-apply", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let place = root_place(&accepted, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    // A plain source-only check over the changed source binds the source against the lock.
    // Because the source shape changed, the committed lock is now stale, so check reports the
    // non-fatal advisory and leaves the store where the baseline put it: check is read-only and
    // is not the surface that advances a store.
    let check = marrow(&["check", root.to_str().unwrap()]);
    assert_eq!(check.status.code(), Some(0), "{check:?}");
    assert!(
        String::from_utf8(check.stderr)
            .expect("stderr utf8")
            .contains("check.stale_lock"),
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
    assert_eq!(
        committed_lock(&root).epoch_high_water,
        baseline_epoch + 1,
        "apply re-projected the committed lock to the new epoch"
    );

    Ok(())
}
