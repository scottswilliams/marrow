//! `marrow run` establishes durable identity by committing the pending catalog into the
//! store transaction, then re-projecting `marrow.lock` from that committed snapshot. The lock
//! is the committed source-tree projection; the store is the sole accepted authority. A second
//! run over the now-accepted catalog churns nothing: the same catalog rows, lock bytes, epoch,
//! and commit stamp.
use crate::support;

use marrow_catalog::{CatalogEntryKind, CatalogLock, CatalogMetadata};
use marrow_store::tree::TreeStore;
use support::{TempProject, find_code_segment, marrow, marrow_sub, write};

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

fn store_snapshot(root: &std::path::Path) -> CatalogMetadata {
    let store = TreeStore::open(&store_path(root)).expect("open store");
    store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("project has an accepted catalog")
}

fn committed_lock(root: &std::path::Path) -> CatalogLock {
    marrow_check::read_committed_lock(root)
        .expect("read committed lock")
        .expect("project has a committed lock")
}

#[test]
fn run_commits_the_pending_catalog_into_the_store_and_reprojects_the_lock() {
    let root = pending_native_project("run-pending-commit");

    let output = marrow_sub("run", &[root.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert_eq!(stdout, "ran\n");
    // The first run writes the otherwise-invisible committed lock, so it announces the creation on
    // stderr (off the program's stdout) and tells the developer to commit it.
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("wrote marrow.lock") && stderr.contains("commit"),
        "first run announces the new committed lock on stderr: {stderr}"
    );

    // The store publishes the accepted catalog at the baseline epoch, with an entry for
    // the saved `^books` root; the committed lock is a projection of those committed rows.
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
            .any(|entry| entry.kind == CatalogEntryKind::Store && entry.path.ends_with("^books")),
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

    // The committed lock re-projects the store snapshot: its high-water equals the store
    // epoch, and every active store entry appears under a matching fingerprint.
    let lock = committed_lock(&root);
    assert_eq!(
        lock.epoch_high_water, snapshot.epoch,
        "the lock high-water equals the committed store epoch"
    );
    for entry in &snapshot.entries {
        let projected = marrow_catalog::LockEntry::from_catalog_entry(entry);
        let lock_entry = lock
            .entries
            .iter()
            .find(|candidate| candidate.stable_id == entry.stable_id)
            .unwrap_or_else(|| panic!("snapshot entry `{}` is in the lock", entry.path));
        assert_eq!(
            lock_entry.shape_fingerprint, projected.shape_fingerprint,
            "the lock fingerprint matches the committed snapshot for `{}`",
            entry.path
        );
    }
}

#[test]
fn a_second_run_does_not_churn_the_accepted_catalog() {
    let root = pending_native_project("run-accepted-noop");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let (digest_one, commit_one, lock_one) = {
        let store = TreeStore::open(&store_path(&root)).expect("open store after first run");
        (
            store.catalog_snapshot_digest().expect("snapshot digest"),
            store.read_commit_metadata().expect("commit metadata"),
            committed_lock(&root),
        )
    };

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    // A second run re-projects nothing, so it emits no lock notice: the announcement fires only
    // when the lock is actually created or changed, never on an idempotent re-run.
    assert!(
        !String::from_utf8(second.stderr.clone())
            .expect("stderr utf8")
            .contains("marrow.lock"),
        "an idempotent second run must not announce a lock write: {second:?}"
    );
    let (digest_two, commit_two, lock_two) = {
        let store = TreeStore::open(&store_path(&root)).expect("open store after second run");
        (
            store.catalog_snapshot_digest().expect("snapshot digest"),
            store.read_commit_metadata().expect("commit metadata"),
            committed_lock(&root),
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
        lock_one, lock_two,
        "the committed lock must stay idempotent on a second run"
    );
}

#[test]
fn a_fresh_checkout_adopts_the_committed_lock_into_an_empty_store() {
    // The committed lock is a generated source-tree projection that seeds a fresh empty
    // store: a checkout that keeps the lock but loses the local store re-establishes the
    // same accepted identity rather than minting fresh ids.
    let root = pending_native_project("run-fresh-checkout-adopts-lock");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let committed = committed_lock(&root);
    let baseline_ids: Vec<String> = store_snapshot(&root)
        .entries
        .iter()
        .map(|entry| entry.stable_id.clone())
        .collect();

    // Simulate a checkout with the committed lock but no local store.
    std::fs::remove_dir_all(root.join(".data")).expect("simulate checkout without local store");

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "a fresh checkout adopts the committed lock: {second:?}"
    );

    // The re-established store carries the lock's committed identity, not freshly minted ids,
    // and the re-projected lock is byte-identical to the committed one.
    let snapshot = store_snapshot(&root);
    let adopted_ids: Vec<String> = snapshot
        .entries
        .iter()
        .map(|entry| entry.stable_id.clone())
        .collect();
    assert_eq!(
        adopted_ids, baseline_ids,
        "the empty store adopts the committed lock identity, not fresh ids"
    );
    assert_eq!(
        committed_lock(&root),
        committed,
        "re-projecting the adopted store yields the committed lock unchanged"
    );
}

#[test]
fn a_retired_store_index_reseeds_a_wiped_store_from_the_lock_alone() {
    // The catastrophic store-loss path: a committed lock whose append-only ledger tombstones a
    // retired STORE INDEX must still re-seed a fresh empty store. The lock is the sole survivor of
    // a checkout that loses its local store, so a valid committed lock that retired an index must
    // never brick the project on the next write-capable open, and the retired id must stay reserved
    // rather than being reissued.
    let root = support::temp_project_uncommitted("run-retired-index-reseeds-wiped-store", |root| {
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
             \x20   index byTitle(title, id)\n\
             pub fn main()\n\
             \x20   print(\"ran\")\n",
        );
    });

    // First run commits the store index into the accepted store and re-projects the lock.
    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let index_id = committed_lock(&root)
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::StoreIndex)
        .map(|entry| entry.stable_id.clone())
        .expect("the committed lock carries the store index");

    // Retire the index: drop its declaration and stage an explicit evolve retire of it.
    write(
        &root,
        "src/app.mw",
        "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire ^books.byTitle\n\
         pub fn main()\n\
         \x20   print(\"ran\")\n",
    );
    // An index retire clears only derived cells, so it activates with no destructive approval.
    let apply = marrow(&["evolve", "apply", root.to_str().unwrap(), "--no-backup"]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");

    // The retire is consumed; remove the spent evolve block so the source is clean again. The
    // ledger now holds the index as a Reserved tombstone with its id retired forever.
    write(
        &root,
        "src/app.mw",
        "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         pub fn main()\n\
         \x20   print(\"ran\")\n",
    );
    let check = marrow_sub("check", &[root.to_str().unwrap()]);
    assert_eq!(check.status.code(), Some(0), "post-retire check: {check:?}");
    let tombstoned = committed_lock(&root);
    assert!(
        tombstoned
            .ledger
            .iter()
            .any(|tombstone| tombstone.kind == CatalogEntryKind::StoreIndex
                && tombstone.id == index_id),
        "the retired index is tombstoned in the committed ledger: {:#?}",
        tombstoned.ledger
    );

    // Store loss: a checkout that keeps the committed lock but loses the local store.
    std::fs::remove_dir_all(root.join(".data")).expect("simulate checkout without local store");

    // Re-seeding the fresh empty store from the lock alone must recover, not brick on a
    // synthesized store.corruption over the shapeless reserved index row.
    let reseed = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(
        reseed.status.code(),
        Some(0),
        "a retired store index must reseed a wiped store from the lock: {reseed:?}"
    );

    // The retired id is preserved as a Reserved row in the re-seeded store and is never reissued:
    // the re-projected lock still tombstones the same id, byte-identical to the pre-loss lock.
    let snapshot = store_snapshot(&root);
    let reserved = snapshot
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::StoreIndex)
        .expect("the re-seeded store carries the reserved index row");
    assert_eq!(
        reserved.stable_id, index_id,
        "the reserved index preserves the retired id, never reissues it"
    );
    assert_eq!(
        reserved.lifecycle,
        marrow_catalog::CatalogLifecycle::Reserved,
        "the re-seeded index row is reserved, not active"
    );
    assert_eq!(
        committed_lock(&root),
        tombstoned,
        "re-projecting the re-seeded store yields the committed lock unchanged"
    );
}

#[test]
fn a_memory_backed_durable_baseline_fails_with_a_typed_error() {
    // A project whose source declares a durable surface (a saved root) but configures no
    // persistent store has identity nothing can hold. The backend is statically known, so
    // `run` checks the project first and fails closed with the typed check error before
    // ever reaching the runtime.
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
        code, "check.durable_store_required",
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
