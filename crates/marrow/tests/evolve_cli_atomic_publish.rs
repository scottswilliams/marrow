//! `evolve apply` publishes the activated catalog into the apply transaction, so there
//! is no post-commit publish window and no resume step. A committed activation advances
//! the store's catalog snapshot, catalog epoch, and data together; a later `run` fences
//! clean against the snapshot the apply published.

use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::{marrow, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, accepted_catalog,
    accepted_catalog_entry_id, commit_catalog, native_books_project, native_store_path,
    open_native_store, read_scalar_by_catalog_id, root_place, seed_title_only, store_epoch,
};

/// A committed proposal-default activation advances the store's catalog snapshot, catalog
/// epoch, and backfilled data in one apply. After it, the store snapshot is the activated
/// catalog (no separate file publish), so a later `run` fences clean with no resume step.
/// This is the end-to-end contract Lane 4 left failing: the run binds the published
/// snapshot directly.
#[test]
fn apply_publishes_snapshot_epoch_and_data_together_then_run_fences_clean() {
    let root = native_books_project("evolve-apply-atomic-default", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
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

    // A run after the apply binds the published snapshot and fences clean with no resume
    // step: the activation epoch fence and the same-epoch schema fence both pass, so the
    // run reaches execution rather than reporting a stale store. (Under Lane 4 the store
    // snapshot lagged the apply and this run fenced as evolved.)
    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved")
            && !stderr.contains("run.schema_drift")
            && !stderr.contains("run.store_unstamped")
            && !stderr.contains("run.store_behind"),
        "the post-apply run fences clean against the published snapshot: {stderr}"
    );
}
