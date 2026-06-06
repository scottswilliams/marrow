use std::fs;

use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::{marrow, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, REQUIRED_DEFAULT_SOURCE,
    REQUIRED_NO_DEFAULT_SOURCE, accepted_catalog, accepted_catalog_entry_id, commit_catalog,
    native_books_project, native_store_path, open_native_store, read_scalar,
    read_scalar_by_catalog_id, root_place, seed_title_only,
};

#[test]
fn evolve_apply_consumes_preview_witness_and_backfills() {
    let root = native_books_project("evolve-apply-default", REQUIRED_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages = read_scalar(&store, &place, 1, "pages", ScalarType::Int);
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("applied evolution"), "{stdout}");
    assert!(stdout.contains("records backfilled: 1"), "{stdout}");
    assert_eq!(pages, Some(Scalar::Int(0)));
    assert_eq!(
        commit.catalog_epoch,
        program.catalog.accepted_epoch.unwrap()
    );
}

#[test]
fn evolve_apply_backfills_proposal_required_default_before_accepting_catalog() {
    let root = native_books_project("evolve-apply-proposal-default", REQUIRED_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let output = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("records backfilled: 2"), "{stdout}");

    let catalog_epoch = accepted_catalog(&root).epoch;
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    for id in [1, 2] {
        assert_eq!(
            read_scalar_by_catalog_id(&store, &accepted_place, id, &pages_id, ScalarType::Int),
            Some(Scalar::Int(0)),
            "pages backfilled before accepted catalog publication"
        );
    }
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp");
    let stamped_epoch = store.read_catalog_epoch().expect("store epoch");

    assert_eq!(catalog_epoch, baseline_epoch + 1);
    assert_eq!(commit.catalog_epoch, baseline_epoch + 1);
    assert_eq!(stamped_epoch, Some(baseline_epoch + 1));
}

#[test]
fn evolve_apply_rejects_repair_required_witness() {
    let root = native_books_project("evolve-apply-repair", REQUIRED_NO_DEFAULT_SOURCE);
    let program = commit_catalog(&root);
    let place = root_place(&program, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &place, 1, "Dune");
    }

    let output = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages = read_scalar(&store, &place, 1, "pages", ScalarType::Int);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["code"], serde_json::json!("evolve.repair_required"));
    assert_eq!(pages, None, "repair-required apply must not write data");
}

#[test]
fn evolve_apply_noop_when_store_and_file_already_at_target() {
    // A defaulting evolution that backfills one record, then applies a second time with
    // the store and file already at the target: the catalog shape is unchanged by a
    // backfill, so the proposal is identity-stable and the second apply must touch
    // neither the catalog file nor the commit id.
    let root = native_books_project("evolve-apply-noop", REQUIRED_DEFAULT_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");

    let path = root.join("marrow.catalog.json");
    let before = fs::read_to_string(&path).expect("read catalog");
    let before_commit = TreeStore::open(&native_store_path(&root))
        .expect("reopen")
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp")
        .commit_id;

    let second = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "no-op apply: {second:?}");

    let after = fs::read_to_string(&path).expect("read catalog");
    let after_commit = TreeStore::open(&native_store_path(&root))
        .expect("reopen")
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp")
        .commit_id;

    assert_eq!(before, after, "no-op apply does not churn the catalog file");
    assert_eq!(
        before_commit, after_commit,
        "no-op apply does not bump the commit id"
    );
}
