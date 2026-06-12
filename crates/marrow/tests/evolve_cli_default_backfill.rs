use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, TreeStore};
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::{marrow, marrow_sub, write};
use support_evolve::{
    OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, REQUIRED_BASELINE_SOURCE, REQUIRED_DEFAULT_SOURCE,
    REQUIRED_NO_DEFAULT_SOURCE, accepted_catalog, accepted_catalog_entry_id, commit_catalog,
    native_books_project, native_store_path, open_native_store, read_scalar,
    read_scalar_by_catalog_id, root_place, seed_member, seed_title_only, store_catalog_id,
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

    let output = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages = read_scalar(&store, &place, 1, "pages", ScalarType::Int);
    let commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["kind"], serde_json::json!("evolve_apply"));
    assert_eq!(record["status"], serde_json::json!("applied"));
    assert_eq!(record["records_backfilled"], serde_json::json!(1));
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

    let output = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let record = support::json(output.stdout);
    assert_eq!(record["status"], serde_json::json!("applied"));
    assert_eq!(record["records_backfilled"], serde_json::json!(2));

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

    assert_eq!(catalog_epoch, baseline_epoch + 1);
    assert_eq!(commit.catalog_epoch, baseline_epoch + 1);
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
fn evolve_apply_noop_when_store_already_at_target() {
    // A defaulting evolution that backfills one record, then applies a second time with
    // the store already at the target: the catalog shape is unchanged by a backfill, so
    // the proposal is identity-stable and the second apply must touch neither the store's
    // accepted catalog snapshot nor the commit id.
    let root = native_books_project("evolve-apply-noop", REQUIRED_DEFAULT_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
    }

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");

    let snapshot_before = catalog_snapshot_digest(&root);
    let before_commit = commit_id(&root);

    let second = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "no-op apply: {second:?}");

    assert_eq!(
        snapshot_before,
        catalog_snapshot_digest(&root),
        "no-op apply does not churn the store's accepted catalog snapshot"
    );
    assert_eq!(
        before_commit,
        commit_id(&root),
        "no-op apply does not bump the commit id"
    );
}

#[test]
fn evolve_apply_does_not_rerun_an_already_applied_transform() {
    let root = native_books_project(
        "evolve-apply-transform-stamped-noop",
        "module books\n\
         resource Book\n\
         \x20   required price: int\n\
         store ^books(id: int): Book\n\
         pub fn add(price: int): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        let store_id = store_catalog_id(&accepted_place);
        store
            .write_node(&store_id, &[SavedKey::Int(1)])
            .expect("write record");
        seed_member(&store, &accepted_place, 1, "price", Scalar::Int(3));
    }
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required price: int\n\
         \x20   required priceCents: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.priceCents\n\
         \x20       return old.price * 100\n\
         pub fn overrideCents()\n\
         \x20   transaction\n\
         \x20       ^books(1).priceCents = 999\n",
    );

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "first apply: {first:?}");
    let cents_id = accepted_catalog_entry_id(&root, "books::Book::priceCents");
    let first_commit = commit_metadata(&root);
    assert_eq!(first_commit.activation_records_transformed, 1);
    assert!(
        !first_commit.activation_evolution_digest.is_empty(),
        "the first apply stamps historical applied-step evidence",
    );
    {
        let after_first = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar_by_catalog_id(&after_first, &accepted_place, 1, &cents_id, ScalarType::Int),
            Some(Scalar::Int(300)),
            "the initial transform computes the derived member",
        );
    }

    let override_run = marrow_sub(
        "run",
        &["--entry", "books::overrideCents", root.to_str().unwrap()],
    );
    assert_eq!(
        override_run.status.code(),
        Some(0),
        "post-activation write: {override_run:?}",
    );
    {
        let after_override =
            TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        let after_override_commit = after_override
            .read_commit_metadata()
            .expect("read commit")
            .expect("post-activation commit");
        assert_eq!(after_override_commit.activation_records_transformed, 1);
        assert_eq!(
            after_override_commit.activation_evolution_digest,
            first_commit.activation_evolution_digest,
            "a normal write carries historical applied evidence for stale replay suppression",
        );
        assert_eq!(
            read_scalar_by_catalog_id(
                &after_override,
                &accepted_place,
                1,
                &cents_id,
                ScalarType::Int
            ),
            Some(Scalar::Int(999)),
            "the current target-state data is the value a stale apply must preserve",
        );
    }
    let before_second_commit = commit_metadata(&root);

    let second = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(0), "second apply: {second:?}");

    {
        let after_second = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar_by_catalog_id(
                &after_second,
                &accepted_place,
                1,
                &cents_id,
                ScalarType::Int
            ),
            Some(Scalar::Int(999)),
            "a stale apply must not re-run the transform over already-activated data",
        );
    }
    assert_eq!(
        before_second_commit.commit_id,
        commit_id(&root),
        "the stale apply is a no-op against a matching target stamp"
    );
    assert_eq!(
        before_second_commit.activation_records_transformed,
        commit_metadata(&root).activation_records_transformed,
        "the stale no-op preserves historical applied-step evidence without re-running",
    );
}

/// The digest of the store's published accepted-catalog snapshot, the durable identity
/// state a no-op apply must leave untouched.
fn catalog_snapshot_digest(root: impl AsRef<std::path::Path>) -> Option<String> {
    TreeStore::open(&native_store_path(root))
        .expect("reopen")
        .catalog_snapshot_digest()
        .expect("read snapshot digest")
}

/// The store's last commit id, which a no-op apply must not advance.
fn commit_id(root: impl AsRef<std::path::Path>) -> u64 {
    commit_metadata(root).commit_id
}

fn commit_metadata(root: impl AsRef<std::path::Path>) -> CommitMetadata {
    TreeStore::open(&native_store_path(root))
        .expect("reopen")
        .read_commit_metadata()
        .expect("read commit")
        .expect("commit stamp")
}
