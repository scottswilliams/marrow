use std::fs;

use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

mod support;
mod support_evolve;

use support::{marrow, write};
use support_evolve::{
    OPTIONAL_PAGES_BASELINE_SOURCE, OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE, PRICE_BASELINE_SOURCE,
    PRICE_CENTS_TRANSFORM_SOURCE, RENAME_SOURCE, REQUIRED_BASELINE_SOURCE, RETIRE_BASELINE_SOURCE,
    RETIRE_SOURCE, accepted_catalog, accepted_catalog_entry_id, commit_catalog, member_catalog_id,
    native_books_project, native_store_path, open_native_store, read_scalar,
    read_scalar_by_catalog_id, root_place, seed_member, seed_title_only, store_catalog_id,
    store_epoch,
};

#[test]
fn evolve_apply_resumes_proposal_default_after_store_commit() {
    let root = native_books_project(
        "evolve-apply-proposal-default-resume",
        REQUIRED_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");

    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch);

    let resume = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );
    // The store already reached the proposal epoch, so the resume re-applies no data and
    // only brings the accepted catalog file forward: a `completed` apply with a zero
    // backfill witness, asserted as typed envelope fields.
    let record = support::json(resume.stdout);
    assert_eq!(record["kind"], serde_json::json!("evolve_apply"));
    assert_eq!(record["status"], serde_json::json!("completed"));
    assert_eq!(record["records_backfilled"], serde_json::json!(0));

    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        for id in [1, 2] {
            assert_eq!(
                read_scalar_by_catalog_id(&store, &accepted_place, id, &pages_id, ScalarType::Int),
                Some(Scalar::Int(0)),
                "resume must not lose the committed backfill"
            );
        }
    }
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch + 1);
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
}

#[test]
fn evolve_apply_resumes_existing_optional_default_with_preserved_value() {
    let root = native_books_project(
        "evolve-apply-existing-optional-default-resume",
        OPTIONAL_PAGES_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(&store, &accepted_place, 1, "pages", Scalar::Int(7));
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");

    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    let preserved =
        read_scalar_by_catalog_id(&store, &accepted_place, 1, &pages_id, ScalarType::Int);
    let defaulted =
        read_scalar_by_catalog_id(&store, &accepted_place, 2, &pages_id, ScalarType::Int);
    let catalog_epoch = accepted_catalog(&root).epoch;

    assert_eq!(preserved, Some(Scalar::Int(7)));
    assert_eq!(defaulted, Some(Scalar::Int(0)));
    assert_eq!(catalog_epoch, baseline_epoch + 1);
}

#[test]
fn evolve_apply_resumes_redundant_existing_optional_default_without_backfill() {
    let root = native_books_project(
        "evolve-apply-existing-optional-default-no-backfill-resume",
        OPTIONAL_PAGES_BASELINE_SOURCE,
    );
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(&store, &accepted_place, 1, "pages", Scalar::Int(7));
        seed_title_only(&store, &accepted_place, 2, "Hyperion");
        seed_member(&store, &accepted_place, 2, "pages", Scalar::Int(9));
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");

    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let pages_id = accepted_catalog_entry_id(&root, "books::Book::pages");
    let first_pages =
        read_scalar_by_catalog_id(&store, &accepted_place, 1, &pages_id, ScalarType::Int);
    let second_pages =
        read_scalar_by_catalog_id(&store, &accepted_place, 2, &pages_id, ScalarType::Int);
    let catalog_epoch = accepted_catalog(&root).epoch;

    assert_eq!(resume.status.code(), Some(0), "{resume:?}");
    assert_eq!(first_pages, Some(Scalar::Int(7)));
    assert_eq!(second_pages, Some(Scalar::Int(9)));
    assert_eq!(catalog_epoch, baseline_epoch + 1);
}

#[test]
fn evolve_apply_resumes_proposal_transform_after_store_commit() {
    let root = native_books_project(
        "evolve-apply-proposal-transform-resume",
        PRICE_BASELINE_SOURCE,
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
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", PRICE_CENTS_TRANSFORM_SOURCE);

    let first = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let price_cents_id = accepted_catalog_entry_id(&root, "books::Book::priceCents");

    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    let resume = marrow(&["evolve", "apply", root.to_str().unwrap()]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );
    let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
    let cents =
        read_scalar_by_catalog_id(&store, &accepted_place, 1, &price_cents_id, ScalarType::Int);
    let catalog_epoch = accepted_catalog(&root).epoch;

    assert_eq!(cents, Some(Scalar::Int(300)));
    assert_eq!(catalog_epoch, baseline_epoch + 1);
}

#[test]
fn evolve_apply_resumes_a_half_applied_store_by_writing_the_file_only() {
    let root = native_books_project("evolve-apply-resume", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    // First apply advances both the store and the file.
    let first = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // Re-create the half-applied crash window: the store is stamped to the target
    // epoch, but the accepted file was never advanced (it still records the baseline).
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch);

    // The subtitle cell is already gone (the first apply deleted it), so a resume must
    // do no data re-apply.
    {
        let store = TreeStore::open(&native_store_path(&root)).expect("reopen native store");
        assert_eq!(
            read_scalar(&store, &accepted_place, 1, "subtitle", ScalarType::Str),
            None,
            "data was already retired by the first apply"
        );
    }

    let resume = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);
    assert_eq!(
        resume.status.code(),
        Some(0),
        "resume completes: {resume:?}"
    );

    // Resuming completes the file side without re-applying data work.
    assert_eq!(accepted_catalog(&root).epoch, baseline_epoch + 1);
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    // The retire was already committed by the first apply, so the resume retires no
    // records: a `completed` apply with a zero retire witness, asserted as typed fields.
    let record = support::json(resume.stdout);
    assert_eq!(record["kind"], serde_json::json!("evolve_apply"));
    assert_eq!(record["status"], serde_json::json!("completed"));
    assert_eq!(
        record["records_retired"],
        serde_json::json!(0),
        "resume re-applies no data"
    );

    let run = marrow(&["run", "--entry", "books::add", root.to_str().unwrap()]);
    let stderr = String::from_utf8(run.stderr).expect("stderr utf8");
    assert!(
        !stderr.contains("run.store_evolved"),
        "epoch fence no longer rejects after resume completes the file: {stderr}"
    );
}

#[test]
fn evolve_apply_resume_fails_closed_when_source_diverges_from_the_store_commit() {
    // The half-applied crash window leaves the store at the target epoch while the file
    // still records the baseline. A resume completes by writing the file alone, but only
    // if the source still describes the evolution the store actually committed. Here the
    // store committed a retire, then the author rewrote the source to a divergent rename
    // before re-running apply. The rename proposes the same epoch the store holds, so the
    // epoch signature alone cannot tell the two apart; the schema-bearing source digest
    // can. Resume must refuse to freeze the rename catalog over the retire the store ran.
    let root = native_books_project("evolve-apply-resume-divergent", RETIRE_BASELINE_SOURCE);
    let accepted = commit_catalog(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    {
        let store = open_native_store(&root);
        seed_title_only(&store, &accepted_place, 1, "Dune");
        seed_member(
            &store,
            &accepted_place,
            1,
            "subtitle",
            Scalar::Str("sub".into()),
        );
    }
    let baseline_epoch = accepted.catalog.accepted_epoch.expect("baseline epoch");
    let baseline_catalog_json =
        fs::read_to_string(root.join("marrow.catalog.json")).expect("baseline catalog");
    write(&root, "src/books.mw", RETIRE_SOURCE);

    // First apply commits the retire to both the store and the file.
    let first = marrow(&[
        "evolve",
        "apply",
        "--maintenance",
        "--approve-retire",
        &format!("{subtitle_id}:1"),
        root.to_str().unwrap(),
    ]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));

    // Re-create the crash window: the store stays at the retire epoch, the file is rewound
    // to the baseline, and the source is replaced with a divergent rename that proposes the
    // same epoch the store already holds.
    fs::write(root.join("marrow.catalog.json"), &baseline_catalog_json).expect("rewind file");
    write(&root, "src/books.mw", RENAME_SOURCE);

    let resume = marrow(&[
        "evolve",
        "apply",
        "--format",
        "json",
        root.to_str().unwrap(),
    ]);

    // The file must remain at the baseline: the divergent rename catalog is never frozen.
    assert_eq!(
        accepted_catalog(&root).epoch,
        baseline_epoch,
        "the divergent rename catalog must not be frozen over the committed retire",
    );
    assert_eq!(store_epoch(&root), Some(baseline_epoch + 1));
    let code = resume.status.code();
    let record = support::json(resume.stdout);
    assert_eq!(code, Some(1), "resume fails closed: {code:?} {record}");
    assert_eq!(
        record["code"],
        serde_json::json!("run.schema_drift"),
        "resume reports schema drift against the committed shape"
    );
}
