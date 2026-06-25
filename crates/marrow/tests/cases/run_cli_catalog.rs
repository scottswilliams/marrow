//! `marrow run` establishes durable identity by committing the pending catalog into the
//! store transaction, then re-projecting `marrow.lock` from that committed snapshot. The lock
//! is the committed source-tree projection; the store is the sole accepted authority. A second
//! run over the now-accepted catalog churns nothing: the same catalog rows, lock bytes, epoch,
//! and commit stamp.
use crate::support;

use marrow_catalog::{CatalogEntryKind, CatalogLock};
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
fn run_over_a_store_lost_under_a_committed_lock_seeds_an_empty_store() {
    // An ABSENT store body is the disposable-store case: a fresh checkout or a `rm -rf .data` that
    // leaves the committed `marrow.lock` behind. A write-capable run seeds an empty store from the
    // committed identity in the lock — reproducing the accepted ids rather than minting fresh — and
    // announces the seed loudly, never failing closed. Only a PRESENT store that lost roots is
    // corruption.
    let root = pending_native_project("run-store-lost-under-lock");

    let first = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "first run: {first:?}");
    let committed = committed_lock(&root);
    assert!(
        committed.records_active_roots(),
        "the committed lock records the accepted ^books root",
    );

    // The store is removed while the committed lock survives.
    std::fs::remove_dir_all(root.join(".data")).expect("simulate a lost local store");

    let second = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "a run over an absent store under a committed lock must seed and succeed: {second:?}"
    );
    let stderr = String::from_utf8(second.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("initialized an empty store from marrow.lock"),
        "the seed from the committed lock must be announced loudly: {stderr}"
    );
    // The seeded store reproduces the committed identity from the lock rather than minting fresh.
    assert_eq!(
        committed_lock(&root),
        committed,
        "seeding from the lock must reproduce the committed identity, not mint a new one",
    );
}

#[test]
fn a_retired_store_index_in_the_lock_seeds_a_lost_store_keeping_the_tombstone() {
    // A committed lock whose append-only ledger tombstones a retired STORE INDEX still records the
    // surviving accepted roots. When the local store body is lost, the lock seeds an empty store
    // from the committed identity. Seeding must carry the retired-index tombstone forward — the
    // retired id stays reserved, never reissued — rather than fail closed.
    let root = support::temp_project_uncommitted("run-retired-index-lost-store", |root| {
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

    // Store loss while the committed lock — still recording the surviving ^books root — survives.
    std::fs::remove_dir_all(root.join(".data")).expect("simulate a lost local store");
    assert!(
        tombstoned.records_active_roots(),
        "the committed lock still records the surviving ^books root",
    );

    // A write-capable open over the absent store seeds an empty store from the lock and succeeds,
    // carrying the retired-index tombstone forward so the retired id stays reserved.
    let reseed = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(
        reseed.status.code(),
        Some(0),
        "a lost store under a committed lock with a retired index must seed and succeed: {reseed:?}"
    );
    let stderr = String::from_utf8(reseed.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("initialized an empty store from marrow.lock"),
        "the seed from the committed lock must be announced loudly: {stderr}"
    );
    let after_seed = committed_lock(&root);
    assert!(
        after_seed
            .ledger
            .iter()
            .any(|tombstone| tombstone.kind == CatalogEntryKind::StoreIndex
                && tombstone.id == index_id),
        "seeding from the lock must keep the retired index tombstoned: {:#?}",
        after_seed.ledger
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

/// The canonical lock entry view for divergence comparison: `(kind, path, lifecycle)` per entry,
/// sorted. Stable ids are intentionally excluded here so the comparison isolates the entry set;
/// the stable-id drift is asserted separately.
fn lock_entry_set(root: &std::path::Path) -> Vec<String> {
    let mut entries: Vec<_> = committed_lock(root)
        .entries
        .into_iter()
        .map(|entry| format!("{:?}|{}|{:?}", entry.kind, entry.path, entry.lifecycle))
        .collect();
    entries.sort();
    entries
}

fn lock_id_for(root: &std::path::Path, path: &str) -> Option<String> {
    committed_lock(root)
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .map(|entry| entry.stable_id)
}

const TWO_STORE_SOURCE: &str = "module app\n\
     resource Book\n\
     \x20   required title: string\n\
     resource Tag\n\
     \x20   required label: string\n\
     store ^books(id: int): Book\n\
     store ^tags(id: int): Tag\n\
     pub fn main()\n\
     \x20   print(\"ran\")\n";

const ONE_STORE_SOURCE: &str = "module app\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn main()\n\
     \x20   print(\"ran\")\n";

fn native_two_store_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(root, "src/app.mw", TWO_STORE_SOURCE);
    })
}

fn native_one_store_project(name: &str) -> TempProject {
    support::temp_project_uncommitted(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(root, "src/app.mw", ONE_STORE_SOURCE);
    })
}

fn run_ok(root: &std::path::Path, label: &str) {
    let output = marrow_sub("run", &[root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{label}: {output:?}");
}

#[test]
fn in_place_removal_of_an_unpopulated_store_matches_a_reseed_lock() {
    // Run in place: the first run commits both stores, then the source drops the unpopulated
    // `^tags` store and a second run reprojects the lock.
    let in_place = native_two_store_project("catalog-remove-in-place");
    run_ok(&in_place, "first run");
    write(in_place.path(), "src/app.mw", ONE_STORE_SOURCE);
    run_ok(&in_place, "second run");

    // Reseed: the identical final source committed from scratch.
    let reseed = native_one_store_project("catalog-remove-reseed");
    run_ok(&reseed, "reseed run");

    assert_eq!(
        lock_entry_set(&in_place),
        lock_entry_set(&reseed),
        "an in-place removal of an unpopulated store must drop it from the lock exactly as a reseed does"
    );
    assert!(
        lock_id_for(&in_place, "app::^tags").is_none(),
        "the removed unpopulated store must not survive as a lock entry"
    );
}

#[test]
fn in_place_remove_then_readd_of_an_unpopulated_store_mints_a_fresh_id() {
    // The store's pre-removal frozen id, captured before any removal.
    let in_place = native_two_store_project("catalog-readd-in-place");
    run_ok(&in_place, "first run");
    let original_id = lock_id_for(&in_place, "app::^tags").expect("tags id before removal");

    write(in_place.path(), "src/app.mw", ONE_STORE_SOURCE);
    run_ok(&in_place, "removal run");
    write(in_place.path(), "src/app.mw", TWO_STORE_SOURCE);
    run_ok(&in_place, "re-add run");
    let readded_id = lock_id_for(&in_place, "app::^tags").expect("tags id after re-add");

    // A reseed of the same re-added source mints fresh random identity. An in-place remove+re-add
    // must do the same: it must not silently resurrect the frozen pre-removal id.
    assert_ne!(
        readded_id, original_id,
        "an in-place remove+re-add must mint a fresh id, matching a reseed, not reuse the retired one"
    );
}

const TWO_MEMBER_ENUM_SOURCE: &str = "module app\n\
     enum Status\n\
     \x20   active\n\
     \x20   archived\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn main()\n\
     \x20   print(\"ran\")\n";

const ONE_MEMBER_ENUM_SOURCE: &str = "module app\n\
     enum Status\n\
     \x20   active\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     pub fn main()\n\
     \x20   print(\"ran\")\n";

#[test]
fn in_place_removal_of_an_unselected_enum_member_matches_a_reseed_lock() {
    // No record selects the enum, so dropping a member is a free no-op. An in-place run must
    // drop the member from the lock exactly as a reseed of the one-member source does.
    let in_place = support::temp_project_uncommitted("catalog-enum-remove-in-place", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(root, "src/app.mw", TWO_MEMBER_ENUM_SOURCE);
    });
    run_ok(&in_place, "first run");
    write(in_place.path(), "src/app.mw", ONE_MEMBER_ENUM_SOURCE);
    run_ok(&in_place, "second run");

    let reseed = support::temp_project_uncommitted("catalog-enum-remove-reseed", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::main" } }"#,
        );
        write(root, "src/app.mw", ONE_MEMBER_ENUM_SOURCE);
    });
    run_ok(&reseed, "reseed run");

    assert_eq!(
        lock_entry_set(&in_place),
        lock_entry_set(&reseed),
        "an in-place removal of an unselected enum member must drop it from the lock exactly as a reseed does"
    );
    assert!(
        lock_id_for(&in_place, "app::Status::archived").is_none(),
        "the removed enum member must not survive as a lock entry"
    );
}
