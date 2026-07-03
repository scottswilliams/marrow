//! Multi-epoch evolution lifecycle coverage: each case advances through real
//! checked programs, witnesses, and runtime apply calls over a store-published
//! accepted catalog.
use crate::evolution_apply_support;
use std::path::Path;

use evolution_apply_support::*;

use marrow_check::{CHECK_CATALOG_INTENT, CheckReport, check_project_with_catalog};
use marrow_run::evolution::{ApplyError, Approval, apply, commit_catalog_baseline};
use marrow_store::cell::CatalogId;
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, ScalarType};

const BASELINE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_baseline.mw"
));

const ADD_PAGES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_default.mw"
));

const ADD_RATING: &str = "module books\n\
     \n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   required pages: int\n\
     \x20   required rating: int\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   default Book.rating = 5\n\
     pub fn add(title: string): Id(^books)\n\
     \x20   return nextId(^books)\n";

const WITH_SUBTITLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_subtitle_baseline.mw"
));

const RETIRE_SUBTITLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_retire_subtitle.mw"
));

const REUSE_SUBTITLE: &str = WITH_SUBTITLE;

const RENAME_TITLE_TO_NAME: &str = "module books\n\
     \n\
     resource Book\n\
     \x20   required name: string\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   rename Book.title -> Book.name\n";

const RENAME_NAME_TO_LABEL: &str = "module books\n\
     \n\
     resource Book\n\
     \x20   required label: string\n\
     store ^books(id: int): Book\n\
     evolve\n\
     \x20   rename Book.name -> Book.label\n";

// A resource with a nested group, so a whole-resource rename must carry a descendant
// leaf's identity forward, not just the top-level members.
const DEEP_BASELINE: &str = "module books\n\
     resource Book\n\
     \x20   required title: string\n\
     \x20   meta\n\
     \x20       required note: string\n\
     store ^books(id: int): Book\n";

// The same tree renamed `Book -> Volume`. The store keeps its own path and stable id;
// only the resource and its subtree relocate.
const DEEP_RENAME: &str = "module books\n\
     resource Volume\n\
     \x20   required title: string\n\
     \x20   meta\n\
     \x20       required note: string\n\
     store ^books(id: int): Volume\n\
     evolve\n\
     \x20   rename Book -> Volume\n";

fn publish_baseline(root: &Path, store: &TreeStore, source: &str) -> marrow_check::CheckedProgram {
    write(root, "src/books.mw", source);
    let pending = checked(root).expect("checked fixture");
    let wrote = commit_catalog_baseline(store, &pending).expect("commit baseline");
    assert!(wrote, "baseline catalog should publish once");
    recheck_against_store_snapshot(root, store)
}

fn check_against_store_snapshot(
    root: &Path,
    store: &TreeStore,
) -> (CheckReport, marrow_check::CheckedProgram) {
    let accepted = store
        .read_catalog_snapshot()
        .expect("read catalog snapshot")
        .expect("store catalog snapshot");
    check_project_with_catalog(root, &config(), Some(&accepted)).expect("check project")
}

fn recheck_against_store_snapshot(root: &Path, store: &TreeStore) -> marrow_check::CheckedProgram {
    let (report, program) = check_against_store_snapshot(root, store);
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

fn apply_program(
    program: &marrow_check::CheckedProgram,
    store: &TreeStore,
) -> marrow_run::evolution::ApplyOutcome {
    let w = witness(program, store);
    apply(&w, program, store, false, None).expect("apply")
}

fn catalog_entry<'a>(
    catalog: &'a marrow_catalog::CatalogMetadata,
    path: &str,
) -> &'a marrow_catalog::CatalogEntry {
    catalog
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("catalog entry `{path}`"))
}

#[test]
fn two_chained_default_applies_advance_epochs_and_backfill_each_step()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-lifecycle-two-defaults", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, &store, BASELINE);
    let place = root_place(&accepted, "books")?;
    let store_id = store_id_of(&place)?;
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    write(&root, "src/books.mw", ADD_PAGES);
    let with_pages = recheck_against_store_snapshot(&root, &store);
    let pages_id = proposal_catalog_id(&with_pages, "books::Book::pages")?;
    let first = apply_program(&with_pages, &store);
    assert_eq!(first.receipt.catalog_epoch, 2);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0))
    );

    write(&root, "src/books.mw", ADD_RATING);
    let with_rating = recheck_against_store_snapshot(&root, &store);
    let rating_id = proposal_catalog_id(&with_rating, "books::Book::rating")?;
    let second = apply_program(&with_rating, &store);
    assert_eq!(second.receipt.catalog_epoch, 3);
    assert_eq!(
        read_scalar(&store, &store_id, 1, &pages_id, INT),
        Some(Scalar::Int(0)),
        "the first epoch's backfill remains readable after the second apply"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &rating_id, INT),
        Some(Scalar::Int(5))
    );

    Ok(())
}

#[test]
fn retired_member_path_stays_reserved_when_later_source_reuses_it()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-lifecycle-retire-reuse", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, &store, WITH_SUBTITLE);
    let place = root_place(&accepted, "books")?;
    let subtitle_id = member_catalog_id(&place, "subtitle")?;
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.member(1, "subtitle", Scalar::Str("Desert".into()));

    write(&root, "src/books.mw", RETIRE_SUBTITLE);
    let retiring = recheck_against_store_snapshot(&root, &store);
    let approval = Approval {
        retires: vec![(CatalogId::new(subtitle_id.clone())?, 1)],
    };
    let w = witness(&retiring, &store);
    apply(&w, &retiring, &store, true, Some(&approval)).expect("retire apply");

    let retired_catalog = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog after retire");
    let retired = retired_catalog
        .entries
        .iter()
        .find(|entry| entry.stable_id == subtitle_id)
        .expect("retired subtitle entry");
    assert_eq!(
        retired.lifecycle,
        marrow_catalog::CatalogLifecycle::Reserved
    );

    write(&root, "src/books.mw", REUSE_SUBTITLE);
    let (report, _program) = check_against_store_snapshot(&root, &store);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "reusing a retired path must fail closed: {:#?}",
        report.diagnostics
    );

    Ok(())
}

#[test]
fn rename_chain_preserves_one_stable_id_and_accumulates_aliases()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-lifecycle-rename-chain", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, &store, BASELINE);
    let place = root_place(&accepted, "books")?;
    let title_id = member_catalog_id(&place, "title")?;

    write(&root, "src/books.mw", RENAME_TITLE_TO_NAME);
    let renamed_once = recheck_against_store_snapshot(&root, &store);
    apply_program(&renamed_once, &store);
    let first_catalog = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog after first rename");
    let name = catalog_entry(&first_catalog, "books::Book::name");
    assert_eq!(name.stable_id, title_id);
    assert!(
        name.aliases
            .iter()
            .any(|alias| alias == "books::Book::title")
    );

    write(&root, "src/books.mw", RENAME_NAME_TO_LABEL);
    let renamed_twice = recheck_against_store_snapshot(&root, &store);
    let outcome = apply_program(&renamed_twice, &store);
    assert_eq!(outcome.receipt.catalog_epoch, 3);

    let second_catalog = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog after second rename");
    let label = catalog_entry(&second_catalog, "books::Book::label");
    assert_eq!(label.stable_id, title_id);
    assert!(
        label
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::title")
    );
    assert!(
        label
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::name")
    );

    Ok(())
}

/// A whole-resource rename over a populated deep tree preserves every descendant's
/// stable id and keeps its stored cells attached. The
/// resource, its top-level field, its group, and the leaf nested inside the group all
/// carry their identity forward under the new resource name, and the seeded `title` and
/// `meta.note` cells read back unchanged under the identical stable ids. Data is keyed on
/// member stable ids, never on the source path, so the rename moves no cell key.
#[test]
fn a_deep_tree_rename_preserves_every_descendant_stable_id_and_its_stored_data()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-lifecycle-deep-rename", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, &store, DEEP_BASELINE);
    let place = root_place(&accepted, "books")?;
    let store_id = store_id_of(&place)?;

    let baseline_catalog = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("baseline catalog");
    let book_id = catalog_entry(&baseline_catalog, "books::Book")
        .stable_id
        .clone();
    let title_id = member_catalog_id(&place, "title")?;
    let meta_id = group_member_catalog_id(&place, "meta")?;
    let note_id = nested_member_catalog_id(&place, "meta", "note")?;

    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Mort".into()));
    seed.nested_member_by_id(1, &meta_id, &note_id, Scalar::Str("Discworld".into()));

    write(&root, "src/books.mw", DEEP_RENAME);
    let renamed = recheck_against_store_snapshot(&root, &store);
    apply_program(&renamed, &store);

    let renamed_catalog = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog after deep rename");
    assert_eq!(
        catalog_entry(&renamed_catalog, "books::Volume").stable_id,
        book_id,
        "the resource carries its id forward"
    );
    assert_eq!(
        catalog_entry(&renamed_catalog, "books::Volume::title").stable_id,
        title_id,
        "the top-level field carries its id forward"
    );
    assert_eq!(
        catalog_entry(&renamed_catalog, "books::Volume::meta").stable_id,
        meta_id,
        "the group carries its id forward"
    );
    assert_eq!(
        catalog_entry(&renamed_catalog, "books::Volume::meta::note").stable_id,
        note_id,
        "the nested leaf carries its id forward"
    );

    assert_eq!(
        read_scalar(&store, &store_id, 1, &title_id, ScalarType::Str),
        Some(Scalar::Str("Mort".into())),
        "the top-level field keeps its stored cell after the rename"
    );
    assert_eq!(
        read_nested_scalar(&store, &store_id, 1, &meta_id, &note_id, ScalarType::Str),
        Some(Scalar::Str("Discworld".into())),
        "the nested leaf keeps its stored cell after the rename"
    );

    Ok(())
}

#[test]
fn second_epoch_witness_fails_closed_when_records_change_mid_chain()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("apply-lifecycle-mid-chain-drift", |_| {});
    let store = TreeStore::memory();
    let accepted = publish_baseline(&root, &store, BASELINE);
    let place = root_place(&accepted, "books")?;
    let store_id = store_id_of(&place)?;
    let seed = Seed {
        store: &store,
        place: &place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    write(&root, "src/books.mw", ADD_PAGES);
    let with_pages = recheck_against_store_snapshot(&root, &store);
    let pages_id = proposal_catalog_id(&with_pages, "books::Book::pages")?;
    apply_program(&with_pages, &store);

    write(&root, "src/books.mw", ADD_RATING);
    let with_rating = recheck_against_store_snapshot(&root, &store);
    let rating_id = proposal_catalog_id(&with_rating, "books::Book::rating")?;
    let stale_witness = witness(&with_rating, &store);

    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));
    seed.member_by_id(2, &pages_id, Scalar::Int(0));

    let result = apply(&stale_witness, &with_rating, &store, false, None);
    assert!(
        matches!(result, Err(ApplyError::Drift)),
        "expected mid-chain count drift, got {result:#?}"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 1, &rating_id, INT),
        None,
        "the stale apply does not backfill the old record"
    );
    assert_eq!(
        read_scalar(&store, &store_id, 2, &rating_id, INT),
        None,
        "the stale apply does not backfill the new record"
    );

    Ok(())
}
