use crate::support;
use std::fs;
use std::path::Path;

use marrow_run::{ProjectMode, ProjectSession, ProjectSurfaceReadSession, ProjectSurfaceSession};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, DataPathSegment, EngineProfile, StoreUid};
use marrow_store::{AccessMode, SealedStore};
use support::{TempDir, write_temp_source};

fn write_native_config(root: &Path) {
    fs::write(
        root.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
    )
    .expect("write marrow.json");
}

/// A pure-scalar program: it declares no durable identity, so a check binds no accepted
/// epoch and proposes no catalog. Run over a stamped store, it is a binary that is not
/// admitted against the store's accepted identity.
fn storeless_source() -> &'static str {
    "module shelf\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"storeless\")\n"
}

/// A program that declares durable identity, so a first run establishes a baseline and
/// projects the committed lock from it.
fn durable_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"baseline\")\n"
}

/// Stamp a native store the way a managed write leaves it: a minted UID, commit metadata,
/// and saved records, with no catalog snapshot. This is the durable record of a store that
/// was admitted and written by some binary; the catalog snapshot is absent, so a program
/// re-checked against this store binds no accepted epoch.
fn stamp_native_store_without_snapshot(store_path: &Path) {
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create store dir");
    let store = SealedStore::open(store_path, AccessMode::Create)
        .expect("open native store")
        .into_store();
    store
        .write_store_uid(&StoreUid::from_entropy_bytes(7u128.to_be_bytes()))
        .expect("write store uid");
    let profile = EngineProfile::new(0);
    store
        .write_commit_metadata(&CommitMetadata {
            commit_id: 0,
            catalog_epoch: 1,
            layout_epoch: profile.layout_epoch(),
            source_digest:
                "sha256:db0de1bdd17cc44de180838ef6ee6540b3917f912a335b54abead5c0d9bfb595"
                    .to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
        })
        .expect("write commit metadata");
    store
        .write_data_value(
            &CatalogId::new("cat_00000000000000000000000000000001").expect("store id"),
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(
                CatalogId::new("cat_00000000000000000000000000000002").expect("member id"),
            )],
            b"v".to_vec(),
        )
        .expect("write saved record");
}

/// The store's durable identity facts an open must leave untouched when it refuses.
fn store_identity(store_path: &Path) -> (Option<String>, Option<CommitMetadata>) {
    let store = SealedStore::open(store_path, AccessMode::Read)
        .expect("open store for identity read")
        .into_store();
    let uid = store
        .read_store_uid()
        .expect("read store uid")
        .map(|uid| uid.as_str().to_string());
    let commit = store.read_commit_metadata().expect("read commit metadata");
    (uid, commit)
}

#[test]
fn run_path_fails_closed_when_accepted_epoch_absent_over_a_stamped_store() {
    let root = TempDir::new("marrow-run-fence-absent-epoch").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), storeless_source());

    let store_path = root.path().join(".data").join("marrow.redb");
    stamp_native_store_without_snapshot(&store_path);
    let before = store_identity(&store_path);
    assert!(before.1.is_some(), "fixture store is stamped");

    let error = ProjectSession::open(root.path(), ProjectMode::Run)
        .expect_err("a binary not admitted against a stamped store must fail closed");
    assert_eq!(error.code(), "run.durable_store_required");

    let after = store_identity(&store_path);
    assert_eq!(
        before, after,
        "a refused open must not write the stamped store: no re-stamp, no UID mint, no new commit",
    );
}

#[test]
fn surface_path_fails_closed_when_accepted_epoch_absent_over_a_stamped_store() {
    // Run-vs-surface symmetry: the same input class returns the same typed code through the
    // surface read and write open paths as it does through the run open path.
    let root = TempDir::new("marrow-run-fence-absent-epoch-surface").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), storeless_source());

    let store_path = root.path().join(".data").join("marrow.redb");
    stamp_native_store_without_snapshot(&store_path);
    let before = store_identity(&store_path);

    let read_error = ProjectSurfaceReadSession::open(root.path())
        .expect_err("surface read must fail closed over a stamped store with no accepted epoch");
    assert_eq!(read_error.code(), "run.durable_store_required");

    let write_error = ProjectSurfaceSession::open(root.path())
        .expect_err("surface write must fail closed over a stamped store with no accepted epoch");
    assert_eq!(write_error.code(), "run.durable_store_required");

    assert_eq!(
        before,
        store_identity(&store_path),
        "refused surface opens must not write the stamped store",
    );
}

#[test]
fn a_baseline_run_projects_the_committed_lock_from_the_store() {
    let root = TempDir::new("marrow-run-fence-baseline-lock").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), durable_source());

    ProjectSession::open(root.path(), ProjectMode::Run).expect("open baseline run");

    let lock_path = root.path().join("marrow.lock");
    let projected = fs::read_to_string(&lock_path).expect("a baseline run projects marrow.lock");
    let lock = marrow_catalog::CatalogLock::from_lock_json(&projected)
        .expect("the projection is a valid lock, not catalog metadata");

    let store_path = root.path().join(".data").join("marrow.redb");
    let store = SealedStore::open(&store_path, AccessMode::Read)
        .expect("open seeded store")
        .into_store();
    let snapshot = store
        .read_catalog_snapshot()
        .expect("read store snapshot")
        .expect("baseline stamps a catalog snapshot");
    assert_eq!(
        lock.epoch_high_water, snapshot.epoch,
        "the projected lock carries the committed store epoch",
    );
    assert!(
        !root.path().join("marrow.catalog.json").exists(),
        "the run path projects a lock, never the removed catalog artifact",
    );
}

#[test]
fn a_commit_run_reprojects_a_missing_lock_without_advancing_the_store() {
    // Convergence: a lock lost after the baseline committed is re-projected on the next commit
    // open from the still-authoritative store, and that re-projection does not re-stamp the store.
    let root = TempDir::new("marrow-run-fence-reproject").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), durable_source());

    ProjectSession::open(root.path(), ProjectMode::Run).expect("open baseline run");
    let lock_path = root.path().join("marrow.lock");
    let store_path = root.path().join(".data").join("marrow.redb");
    let lock_after_seed = fs::read(&lock_path).expect("baseline projects the lock");
    let store_after_seed = store_identity(&store_path);

    fs::remove_file(&lock_path).expect("lose the committed lock");

    ProjectSession::open(root.path(), ProjectMode::Run).expect("open converging run");

    assert_eq!(
        lock_after_seed,
        fs::read(&lock_path).expect("the converging run re-projects the lock"),
        "a re-projected lock is byte-identical to the original projection",
    );
    assert_eq!(
        store_after_seed,
        store_identity(&store_path),
        "re-projecting a missing lock must not advance or re-stamp the store",
    );
}

#[test]
fn a_converged_commit_run_reprojects_nothing() {
    // No churn: a run over a store whose lock already matches re-projects the same bytes, so the
    // committed lock is untouched and the store is not re-stamped.
    let root = TempDir::new("marrow-run-fence-no-churn").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), durable_source());

    ProjectSession::open(root.path(), ProjectMode::Run).expect("open baseline run");
    let lock_path = root.path().join("marrow.lock");
    let store_path = root.path().join(".data").join("marrow.redb");
    let lock_before = fs::read(&lock_path).expect("baseline projects the lock");
    let store_before = store_identity(&store_path);

    ProjectSession::open(root.path(), ProjectMode::Run).expect("open converged run");

    assert_eq!(
        lock_before,
        fs::read(&lock_path).expect("read lock after converged run"),
        "a converged run leaves the committed lock byte-identical",
    );
    assert_eq!(
        store_before,
        store_identity(&store_path),
        "a converged run does not re-stamp the store",
    );
}
