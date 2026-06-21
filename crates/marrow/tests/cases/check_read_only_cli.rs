//! `marrow check` is read-only over durable state: it neither freezes durable identity
//! nor creates, opens, repairs, or mutates the saved-data store. The durable write
//! paths — `marrow run` over a persistent store and `marrow evolve apply` — are the
//! contrast: each commits, so each re-projects the committed `marrow.lock` and advances
//! the store.

use std::fs;
use std::path::Path;

use crate::support;
use crate::support_evolve;
use marrow_store::tree::{StoreUid, TreeStore};
use support::{
    corrupt_primary_slot_selector, counter_source, marrow, native_config, temp_project_uncommitted,
    write,
};
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
fn check_json_envelope_carries_the_stale_lock_advisory() {
    // A machine consumer parses the stdout JSON envelope, never stderr. A plain `check` passes
    // with a stale lock (status ok), but the advisory must still appear as a structured
    // diagnostic in the envelope, with its typed code, not only as a stderr note.
    let root = native_books_project("check-ro-stale-lock-json", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books").expect("books root place");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let check = marrow(&["check", "--format", "json", root.to_str().unwrap()]);
    assert_eq!(
        check.status.code(),
        Some(0),
        "a stale lock is a non-fatal advisory: plain check still succeeds: {check:?}"
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&check.stdout).expect("stdout is one JSON envelope");
    assert_eq!(
        envelope["status"], "ok",
        "the advisory keeps the success envelope: {envelope:#?}"
    );
    let diagnostics = envelope["diagnostics"]
        .as_array()
        .expect("the envelope carries a diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "check.stale_lock"),
        "the stale-lock advisory must be a structured diagnostic in the stdout envelope, not \
         only on stderr: {envelope:#?}"
    );
}

#[test]
fn check_locked_fails_on_a_stale_lock_that_plain_check_only_advises() {
    // The lockfile-ecosystem convention: `--locked` (cf. cargo --locked) turns the stale-lock
    // advisory into a fatal CI gate. Plain `check` keeps the non-fatal advisory so the ordinary
    // edit -> check -> run loop is never blocked; both report the same typed stale-lock code, and
    // neither opens or rewrites the store.
    let root = native_books_project("check-ro-locked-strict", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books").expect("books root place");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }
    let lock_before = committed_lock(&root);
    let store_before = fs::read(native_store_path(&root)).expect("read store before check");

    // Drift the source so the committed lock is stale.
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let advisory = marrow(&["check", root.to_str().unwrap()]);
    assert_eq!(
        advisory.status.code(),
        Some(0),
        "plain check keeps the stale lock non-fatal: {advisory:?}"
    );
    assert!(
        String::from_utf8(advisory.stderr)
            .expect("stderr utf8")
            .contains("check.stale_lock"),
        "plain check still surfaces the typed advisory"
    );

    let strict = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        strict.status.code(),
        Some(1),
        "--locked makes a stale lock fatal: {strict:?}"
    );
    assert!(
        String::from_utf8(strict.stderr)
            .expect("stderr utf8")
            .contains("check.stale_lock"),
        "--locked reports the same typed stale-lock code"
    );

    // Both runs are read-only: the store bytes and committed lock are untouched.
    assert_eq!(
        fs::read(native_store_path(&root)).expect("read store after check"),
        store_before,
        "a --locked check must not open or rewrite the store"
    );
    assert_eq!(
        committed_lock(&root),
        lock_before,
        "a --locked check must not re-project the committed lock"
    );
}

#[test]
fn check_locked_json_envelope_reports_a_stale_lock_as_failed() {
    // A CI tool parses the stdout JSON envelope, never stderr. A fatal `--locked` stale lock
    // exits 1, so the envelope status must agree: `failed`, carrying the `check.stale_lock`
    // diagnostic, and dropping the success-only `entry_footprints`/`surface_abi`/`surface_routes`.
    let root = native_books_project("check-ro-locked-json", REQUIRED_BASELINE_SOURCE);
    commit_catalog(&root);
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let strict = marrow(&[
        "check",
        "--locked",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        strict.status.code(),
        Some(1),
        "--locked makes a stale lock fatal: {strict:?}"
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&strict.stdout).expect("stdout is one JSON envelope");
    assert_eq!(
        envelope["status"], "failed",
        "the envelope status must agree with the nonzero exit: {envelope:#?}"
    );
    let diagnostics = envelope["diagnostics"]
        .as_array()
        .expect("a failed check envelope carries diagnostics");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "check.stale_lock"),
        "the fatal stale-lock diagnostic is in the envelope, not only on stderr: {envelope:#?}"
    );
    assert!(
        envelope.get("entry_footprints").is_none()
            && envelope.get("surface_abi").is_none()
            && envelope.get("surface_routes").is_none(),
        "footprints and surface descriptors are success-only: {envelope:#?}"
    );
}

#[test]
fn check_locked_passes_a_fresh_lock() {
    // `--locked` only fails on staleness: a project whose committed lock matches the current
    // source checks cleanly with exit 0 and emits no stale-lock advisory.
    let root = native_books_project("check-ro-locked-fresh", REQUIRED_BASELINE_SOURCE);
    commit_catalog(&root);

    let strict = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        strict.status.code(),
        Some(0),
        "--locked passes a fresh lock: {strict:?}"
    );
    assert!(
        !String::from_utf8(strict.stderr)
            .expect("stderr utf8")
            .contains("check.stale_lock"),
        "a fresh lock raises no stale-lock condition"
    );
}

#[test]
fn check_locked_fails_when_the_lock_is_absent_over_a_durable_store() {
    // `--locked` is a CI gate whose purpose is to fail when the committed lock is not current.
    // An entirely absent lock over a project that has durable shape to lock (a stamped store /
    // accepted catalog) is not current — it is missing — so the gate must fail closed with a
    // distinct typed code rather than passing green. A developer who forgets to commit, or
    // deletes, `marrow.lock` must not get a false green in CI.
    let root = native_books_project("check-ro-locked-missing", REQUIRED_BASELINE_SOURCE);
    commit_catalog(&root);
    assert!(
        store_path(&root).exists(),
        "commit established the durable store shape"
    );
    fs::remove_file(lock_path(&root)).expect("remove committed lock");

    let strict = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        strict.status.code(),
        Some(1),
        "--locked over a durable store with no committed lock is fatal: {strict:?}"
    );
    assert!(
        String::from_utf8(strict.stderr)
            .expect("stderr utf8")
            .contains("check.lock_missing"),
        "the missing committed lock surfaces the distinct typed code"
    );
    assert!(
        !lock_path(&root).exists(),
        "a fatal --locked check does not reconstruct the missing lock"
    );
}

#[test]
fn check_locked_passes_a_legitimate_first_run_with_no_durable_shape() {
    // The carve-out: a legitimate first run has no durable shape to lock yet — no stamped store,
    // no accepted catalog — so an absent lock is the expected first-run state, not a missing
    // commit. `--locked` stays clean and exits 0, and the advisory mode is silent on absence too.
    let project = temp_project_uncommitted("check-ro-locked-firstrun", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");
    assert!(!lock_path(&project).exists(), "no lock on a first run");
    assert!(!store_path(&project).exists(), "no store on a first run");

    let strict = marrow(&["check", "--locked", dir]);
    assert_eq!(
        strict.status.code(),
        Some(0),
        "--locked is clean on a first run with no durable shape to lock: {strict:?}"
    );
    assert!(
        !String::from_utf8(strict.stderr)
            .expect("stderr utf8")
            .contains("check.lock_missing"),
        "absence is silent when there is no durable shape to lock"
    );
}

#[test]
fn check_locked_passes_a_uid_only_store_with_no_committed_catalog() {
    // The crash-window remnant: `ensure_store_uid` stamped the store uid, but the process died
    // before `establish_store_baseline` published any accepted catalog (the same uid-only state
    // `data recover` can leave). A read-only open succeeds and yields no catalog snapshot, so the
    // store carries no durable shape to lock yet. `--locked` must treat that like a first run and
    // pass green, not falsely demand a committed lock for shape that does not exist.
    let project = temp_project_uncommitted("check-ro-locked-uid-only", |root| {
        write(root, "marrow.json", native_config());
        write(root, "src/app.mw", counter_source());
    });
    let dir = project.to_str().expect("project path utf-8");
    {
        let store = open_native_store(&project);
        store
            .write_store_uid(
                &StoreUid::new("store_00000000000000000000000000000099".to_string())
                    .expect("valid fixture store uid"),
            )
            .expect("write fixture store uid");
        assert!(
            store
                .read_catalog_snapshot()
                .expect("read store catalog snapshot")
                .is_none(),
            "the uid-only store carries no committed catalog"
        );
    }
    assert!(!lock_path(&project).exists(), "no committed lock yet");

    let strict = marrow(&["check", "--locked", dir]);
    assert_eq!(
        strict.status.code(),
        Some(0),
        "--locked passes a uid-only store with no committed catalog to lock: {strict:?}"
    );
    assert!(
        !String::from_utf8(strict.stderr)
            .expect("stderr utf8")
            .contains("check.lock_missing"),
        "a uid-only store has no durable shape, so an absent lock is not missing"
    );
}

#[test]
fn check_locked_still_fails_on_a_present_stale_lock() {
    // The missing-lock fix does not weaken the stale-lock gate: a present-but-stale committed lock
    // over a durable store is still fatal under `--locked`, reported as `check.stale_lock`, not as
    // a missing lock.
    let root = native_books_project("check-ro-locked-stale-distinct", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books").expect("books root place");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let strict = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        strict.status.code(),
        Some(1),
        "--locked over a present stale lock is fatal: {strict:?}"
    );
    let stderr = String::from_utf8(strict.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("check.stale_lock"),
        "a present stale lock is reported as stale, not missing: {stderr}"
    );
    assert!(
        !stderr.contains("check.lock_missing"),
        "a present lock is never a missing-lock condition: {stderr}"
    );
}

#[test]
fn check_locked_fails_when_the_lock_is_absent_over_an_unreadable_store() {
    // The post-crash state where the `--locked` gate matters most: a durable store exists on
    // disk but an unclean shutdown left it recovery-required, so a read-only open fails. A store
    // file that is present-but-unreadable still carries durable shape that demands a committed
    // lock — treating an unopenable store as no-authority would misclassify a crashed project as
    // a first run and pass an absent lock green, defeating the gate exactly when it is needed.
    let root = native_books_project("check-ro-locked-missing-unclean", REQUIRED_BASELINE_SOURCE);
    commit_catalog(&root);
    assert!(
        store_path(&root).exists(),
        "commit established the durable store shape"
    );
    corrupt_primary_slot_selector(&root);
    fs::remove_file(lock_path(&root)).expect("remove committed lock");
    assert!(
        TreeStore::open_read_only(&store_path(&root)).is_err(),
        "the corrupted store must be unreadable read-only"
    );

    let strict = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        strict.status.code(),
        Some(1),
        "--locked over a present-but-unreadable store with no committed lock is fatal: {strict:?}"
    );
    assert!(
        String::from_utf8(strict.stderr)
            .expect("stderr utf8")
            .contains("check.lock_missing"),
        "a present-but-unreadable durable store still demands a committed lock"
    );
    assert!(
        !lock_path(&root).exists(),
        "a fatal --locked check does not reconstruct the missing lock or repair the store"
    );
}

#[test]
fn check_locked_consistent_across_data_recover_for_an_unclean_store() {
    // `data recover` repairs the recovery-required store back to readable; with the committed
    // lock still absent, the `--locked` gate must remain fatal `check.lock_missing` before and
    // after recovery. Repairing the store does not invent a lock, so the gate's verdict is
    // consistent across the unreadable and the readable state of the same durable store.
    let root = native_books_project(
        "check-ro-locked-recover-consistent",
        REQUIRED_BASELINE_SOURCE,
    );
    commit_catalog(&root);
    corrupt_primary_slot_selector(&root);
    fs::remove_file(lock_path(&root)).expect("remove committed lock");

    let before = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(before.status.code(), Some(1), "{before:?}");
    assert!(
        String::from_utf8(before.stderr)
            .expect("stderr utf8")
            .contains("check.lock_missing"),
        "unclean store with absent lock fails the gate"
    );

    let recover = marrow(&["data", "recover", root.to_str().unwrap()]);
    assert_eq!(recover.status.code(), Some(0), "{recover:?}");
    TreeStore::open_read_only(&store_path(&root)).expect("recover leaves the store readable");

    let after = marrow(&["check", "--locked", root.to_str().unwrap()]);
    assert_eq!(
        after.status.code(),
        Some(1),
        "a recovered store with no committed lock is still a fatal gate: {after:?}"
    );
    assert!(
        String::from_utf8(after.stderr)
            .expect("stderr utf8")
            .contains("check.lock_missing"),
        "the gate verdict is consistent across recovery"
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
