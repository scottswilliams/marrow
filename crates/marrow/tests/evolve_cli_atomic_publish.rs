//! `evolve apply` publishes the activated catalog into the apply transaction, then
//! renders `marrow.catalog.json` from the committed store snapshot. A committed
//! activation advances the store's catalog snapshot, catalog epoch, and data together;
//! a later `run` fences clean against the snapshot the apply published.

use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use std::fs;

use support::{marrow, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, accepted_catalog,
    accepted_catalog_entry_id, commit_catalog, native_books_project, native_store_path,
    open_native_store, read_scalar_by_catalog_id, root_place, seed_title_only, store_epoch,
};

/// A committed proposal-default activation advances the store's catalog snapshot, catalog
/// epoch, and backfilled data in one apply. After it, the file is rendered from the
/// activated store snapshot, so a later `run` fences clean even if it has to repair that
/// render first.
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
    assert_eq!(
        fs::read_to_string(root.join("marrow.catalog.json")).expect("read rendered catalog"),
        published.to_json_pretty(),
        "apply renders marrow.catalog.json from the committed store snapshot"
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
    // rather than reporting a stale store. If the file render is missing, the run repairs
    // it from the committed store snapshot first.
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
}
