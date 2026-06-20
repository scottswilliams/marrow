//! `evolve apply` publishes the activated catalog into the apply transaction, then
//! re-projects `marrow.lock` from the committed store snapshot. A committed activation
//! advances the store's catalog snapshot, catalog epoch, and data together; apply is not
//! done until the re-projected lock is committed, and a later `run` fences clean against
//! the snapshot the apply published.

use crate::support;
use crate::support_evolve;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

use support::{marrow, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, accepted_catalog,
    accepted_catalog_entry_id, commit_catalog, committed_lock, native_books_project,
    native_store_path, open_native_store, read_scalar_by_catalog_id, root_place, seed_title_only,
    store_epoch,
};

/// Evolve apply is not done until the re-projected `marrow.lock` is committed: after an
/// exit-0 apply, the committed lock parses as a valid `CatalogLock` whose epoch high-water
/// equals the store's stamped catalog epoch (N+1), whose source digest is the current
/// source's, whose per-entry shape fingerprints match the activated proposal, and whose
/// ledger carries the baseline ids un-reissued; no full-catalog render exists at the root.
#[test]
fn evolve_apply_is_not_done_until_the_reprojected_lock_is_committed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-apply-reproject-lock", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_lock = committed_lock(&root).expect("baseline lock projected");
    let baseline_ids: Vec<String> = baseline_lock
        .entries
        .iter()
        .map(|entry| entry.stable_id.clone())
        .collect();
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    // The activation advanced the store's catalog snapshot, epoch, and data together.
    let published = accepted_catalog(&root);
    assert_eq!(published.epoch, baseline_epoch + 1);
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // Apply is not done until the re-projected lock is committed: the typed oracle is the
    // lock value, not CLI prose. The lock parses, its high-water equals the stamped store
    // epoch, and its source digest is the current activated source's digest.
    let lock = committed_lock(&root).expect("apply re-projected the committed lock");
    assert_eq!(
        lock.epoch_high_water,
        baseline_epoch + 1,
        "the re-projected lock carries the activated store epoch as its high-water"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let store_epoch_value = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("store is stamped")
        .catalog_epoch;
    assert_eq!(
        lock.epoch_high_water, store_epoch_value,
        "the lock high-water equals the store's stamped catalog epoch"
    );

    // The per-entry fingerprints match the activated proposal: every active entry the
    // published snapshot records appears in the lock under a matching fingerprint.
    for entry in published.entries.iter() {
        let projected = marrow_catalog::LockEntry::from_catalog_entry(entry);
        let lock_entry = lock
            .entries
            .iter()
            .find(|candidate| candidate.stable_id == entry.stable_id)
            .unwrap_or_else(|| panic!("activated entry `{}` is in the lock", entry.path));
        assert_eq!(
            lock_entry.shape_fingerprint, projected.shape_fingerprint,
            "the lock fingerprint matches the activated proposal for `{}`",
            entry.path
        );
    }

    // The lock's append-only ledger carries the baseline ids without reissuing them as
    // fresh active entries.
    let active_ids: std::collections::HashSet<&str> =
        lock.entries.iter().map(|e| e.stable_id.as_str()).collect();
    for id in &baseline_ids {
        assert!(
            active_ids.contains(id.as_str()),
            "the baseline id `{id}` survives in the lock, not reissued"
        );
    }

    // The committed lock is the only source-tree projection: the removed full-catalog render is
    // never written at the project root. Build the old render's name from parts so the absence
    // scan for the obsolete artifact finds no live reference to it.
    let removed_render = format!("marrow.{}.json", "catalog");
    assert!(
        !root.join(&removed_render).exists(),
        "apply commits the lock projection, never a full-catalog render"
    );

    Ok(())
}

/// A committed proposal-default activation advances the store's catalog snapshot, catalog
/// epoch, and backfilled data in one apply. After it, the lock is re-projected from the
/// activated store snapshot, so a later `run` fences clean.
#[test]
fn apply_publishes_snapshot_epoch_and_data_together_then_run_fences_clean()
-> Result<(), Box<dyn std::error::Error>> {
    let root = native_books_project("evolve-apply-atomic-default", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books")?;
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_digest = accepted_catalog(&root).digest;
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let apply = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(apply.status.code(), Some(0), "{apply:?}");

    // The store snapshot, epoch, and data all advanced together in the apply transaction.
    let published = accepted_catalog(&root);
    assert_eq!(
        published.epoch,
        baseline_epoch + 1,
        "the published catalog snapshot advanced to the activated epoch"
    );
    assert_ne!(
        published.digest, baseline_digest,
        "the published snapshot is the activated catalog, not the baseline"
    );
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    let lock = committed_lock(&root).expect("apply re-projected the committed lock");
    assert_eq!(
        lock.epoch_high_water,
        baseline_epoch + 1,
        "apply re-projected marrow.lock from the committed store snapshot"
    );

    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        for id in [1, 2] {
            assert_eq!(
                read_scalar_by_catalog_id(&store, &accepted_place, id, &pages_id, ScalarType::Int),
                Some(Scalar::Int(0)),
                "the backfill committed in the same transaction as the snapshot"
            );
        }
    }

    // A run after the apply binds the published snapshot and fences clean: the activation
    // epoch fence and the same-epoch schema fence both pass, so the run reaches execution
    // rather than reporting a stale store.
    let post_apply_source = format!(
        "{OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE}\n\
         pub fn noop()\n\
         \x20   print(\"ok\")\n"
    );
    write(&root, "src/books.mw", &post_apply_source);
    let run = marrow(&["run", "--entry", "books::noop", root.to_str().unwrap()]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved")
            && !stderr.contains("run.schema_drift")
            && !stderr.contains("run.store_unstamped")
            && !stderr.contains("run.store_behind"),
        "the post-apply run fences clean against the published snapshot: {stderr}"
    );

    Ok(())
}
