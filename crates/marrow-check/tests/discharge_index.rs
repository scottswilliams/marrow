mod support;
mod support_discharge;

use marrow_catalog::CatalogEntryKind;
use marrow_check::check_project;
use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

use support::catalog::write_catalog;
use support::{TempProject, config, temp_project, write};
use support_discharge::*;

fn composite_index_project(name: &str) -> TempProject {
    temp_project(name, |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required a: string\n\
             \x20   required b: string\n\
             \x20   index byPair(a, b) unique\n\
             pub fn add(a: string, b: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    })
}

/// Retiring a member whose source is gone, with populated records, is a destructive
/// decision. The verdict names the exact catalog id and the populated count.
#[test]
fn retire_of_populated_member_requires_scoped_approval() {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             evolve\n\
             \x20   retire Book.subtitle\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            6,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let store_id = CatalogId::new(accepted_catalog_id(&place.store_catalog_id, "store")).unwrap();
    let subtitle = CatalogId::new(subtitle_id.clone()).unwrap();
    for (id, value) in [(1, "A"), (2, "B")] {
        store.write_node(&store_id, &[SavedKey::Int(id)]).unwrap();
        store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(subtitle.clone())],
                encode_value(&Scalar::Str(value.into())).unwrap(),
            )
            .unwrap();
    }

    let result = witness(&program, &store);

    match verdict_for(&result, &subtitle_id) {
        Verdict::DestructiveDecisionRequired { populated } => assert_eq!(*populated, 2),
        other => panic!("expected destructive decision, got {other:#?}"),
    }
}

/// A new unique index over clean (collision-free) data discharges to a derived
/// rebuild.
#[test]
fn new_unique_index_over_clean_data_rebuilds() {
    let root = temp_project("discharge-index-clean", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.index_entry("byIsbn", Scalar::Str("111".into()), 1);
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));
    seed.index_entry("byIsbn", Scalar::Str("222".into()), 2);

    let result = witness(&program, &store);

    let index_id = index_catalog_id(&place, "byIsbn");
    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::DerivedRebuild),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.index_collisions, 0);
}

/// A unique index over colliding data fails activation and the witness counts the
/// collisions.
#[test]
fn new_unique_index_over_colliding_data_fails() {
    let root = temp_project("discharge-index-collide", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Two records claim the same unique key: a collision the index cannot publish.
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("dup".into()));
    seed.index_entry("byIsbn", Scalar::Str("dup".into()), 1);
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("dup".into()));
    seed.index_entry("byIsbn", Scalar::Str("dup".into()), 2);

    let (witness, diagnostics) = preview(&program, &store).expect("preview");

    let byisbn_id = index_catalog_id(&place, "byIsbn");
    assert!(!witness.is_activatable(), "{witness:#?}");
    assert!(witness.counts.index_collisions > 0, "{witness:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == byisbn_id),
        "{diagnostics:#?}"
    );
}

/// Dropping a sparse source field that nothing else depends on is a legal no-op. The
/// accepted entry lingers as data under its stable id, so the verdict is a no-op, not
/// an error and not a distinct deprecation outcome.
#[test]
fn dropped_sparse_field_is_no_op_not_error() {
    let subtitle_id = hex_id(4);
    let root = temp_project("discharge-dropped-sparse-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            11,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::subtitle",
                    &subtitle_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root);
    let store = TreeStore::memory();

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &subtitle_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == subtitle_id),
        "{diagnostics:#?}"
    );
}

/// A dropped source field a unique index still reads is not a silent deprecation;
/// discharge requires a retire intent. The accepted catalog keeps a member `isbn`
/// and an index `byIsbn(isbn)`; current source drops the member but keeps the index,
/// so the proposal carries the lingering member with the index still reading it.
#[test]
fn dropped_field_an_index_needs_requires_retire() {
    let isbn_id = hex_id(4);
    let root = temp_project("discharge-dropped-field-needs-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(title: string, isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            12,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::isbn",
                    &isbn_id,
                ),
                entry(
                    CatalogEntryKind::StoreIndex,
                    "books::^books::byIsbn",
                    &hex_id(5),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    // The accepted catalog already matches source, so this first check is clean and
    // binds `isbn`. Now drop the `isbn` member from source while keeping the index,
    // so the index reads a member current source no longer declares.
    checked(&root);
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   index byIsbn(isbn) unique\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check");
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    // The dropped member an index still reads is its own typed failure: a retire is
    // required, distinct from a plain missing-required-member repair. The verdict
    // carries the index's catalog identity (its accepted stable id), prose-free; the
    // developer-facing name surfaces only in the diagnostic.
    match verdict_for(&result, &isbn_id) {
        Verdict::RepairRequired {
            reason: RepairReason::RetireRequired { index },
        } => assert_eq!(index.as_str(), hex_id(5)),
        other => panic!("expected RetireRequired, got {other:#?}"),
    }
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == isbn_id),
        "{diagnostics:#?}"
    );
}

#[test]
fn dropped_field_ignores_same_named_index_on_another_resource() {
    let book_subtitle_id = hex_id(5);
    let root = temp_project("discharge-dropped-field-index-owner", |root| {
        write(
            root,
            "src/media.mw",
            "module media\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             resource Movie at ^movies(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   index bySubtitle(subtitle) unique\n",
        );
        let accepted = accepted_catalog(
            12,
            "media::Book",
            "media::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Book::subtitle",
                    &book_subtitle_id,
                ),
                entry(CatalogEntryKind::Resource, "media::Movie", &hex_id(6)),
                entry(CatalogEntryKind::Store, "media::^movies", &hex_id(7)),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Movie::title",
                    &hex_id(8),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "media::Movie::subtitle",
                    &hex_id(9),
                ),
                entry(
                    CatalogEntryKind::StoreIndex,
                    "media::^movies::bySubtitle",
                    &hex_id(10),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    checked(&root);
    write(
        &root,
        "src/media.mw",
        "module media\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         resource Movie at ^movies(id: int)\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         \x20   index bySubtitle(subtitle) unique\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check");
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &book_subtitle_id), Verdict::NoOp),
        "{result:#?}"
    );
    // Dropping `Book.subtitle` must not be blamed on `Movie`'s `bySubtitle` index: no
    // diagnostic carries that index's catalog id (`hex_id(10)`).
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == hex_id(10)),
        "{diagnostics:#?}"
    );
}

/// Dropping a source index while keeping the member it covered is an index-subtree
/// deletion, not a silent no-op: the index binding is gone but its cells would linger.
/// The accepted catalog carries a member `isbn` and an index `byIsbn(isbn)`; current
/// source keeps `isbn` and drops the index, so discharge classifies the dropped index
/// id as `IndexDropped` and tags it as a changed index id apply deletes.
#[test]
fn dropped_index_is_index_dropped() {
    let index_id = hex_id(5);
    let root = temp_project("discharge-drop-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(title: string, isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            13,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    &hex_id(3),
                ),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::isbn",
                    &hex_id(4),
                ),
                entry(
                    CatalogEntryKind::StoreIndex,
                    "books::^books::byIsbn",
                    &index_id,
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    // The accepted catalog matches source, so this first check is clean. Then drop the
    // index from source while keeping `isbn`, so only the index binding disappears.
    checked(&root);
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required isbn: string\n\
         pub fn add(title: string, isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let (_report, program) = check_project(&root, &config()).expect("check");
    let store = TreeStore::memory();
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::IndexDropped),
        "{:#?}",
        result.verdicts
    );
    assert!(
        result
            .changed_index_catalog_ids
            .iter()
            .any(|id| id.as_str() == index_id),
        "dropped index id is tagged as a changed index id: {:#?}",
        result.changed_index_catalog_ids
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A composite unique index over distinct full key tuples that happen to share
/// their first key column is collision-free: the discharge must derive the full
/// `(a, b)` tuple per record, not descend the first column alone.
#[test]
fn composite_unique_index_distinct_tuples_rebuild() {
    let root = composite_index_project("discharge-composite-clean");
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "a", Scalar::Str("shared".into()));
    seed.member(1, "b", Scalar::Str("one".into()));
    seed.record(2);
    seed.member(2, "a", Scalar::Str("shared".into()));
    seed.member(2, "b", Scalar::Str("two".into()));

    let result = witness(&program, &store);

    let index_id = index_catalog_id(&place, "byPair");
    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::DerivedRebuild),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.index_collisions, 0);
}

/// A composite unique index over a real duplicate full tuple `(a, b)` is a
/// collision the index cannot publish, even when the records also share their
/// first column. The verdict fails closed.
#[test]
fn composite_unique_index_duplicate_tuple_collides() {
    let root = composite_index_project("discharge-composite-collide");
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "a", Scalar::Str("same".into()));
    seed.member(1, "b", Scalar::Str("same".into()));
    seed.record(2);
    seed.member(2, "a", Scalar::Str("same".into()));
    seed.member(2, "b", Scalar::Str("same".into()));

    let result = witness(&program, &store);

    let index_id = index_catalog_id(&place, "byPair");
    assert!(
        matches!(
            verdict_for(&result, &index_id),
            Verdict::RepairRequired { .. }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(result.counts.index_collisions > 0, "{:#?}", result.counts);
}

/// A newly-declared single-column unique index has no index cells yet: the
/// discharge must derive each record's prospective key from its member value, not
/// from a (nonexistent) index entry. Distinct member values rebuild cleanly.
#[test]
fn new_unique_index_no_cells_clean_rebuilds() {
    let root = temp_project("discharge-new-index-clean", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Records exist with distinct member values, but no index cells were ever
    // written: the index is being declared over a pre-existing store.
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));

    let result = witness(&program, &store);

    let index_id = index_catalog_id(&place, "byIsbn");
    assert!(
        matches!(verdict_for(&result, &index_id), Verdict::DerivedRebuild),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.index_collisions, 0);
}

/// A newly-declared unique index over pre-existing records whose member values
/// collide fails closed, even though the store holds no index cells: the
/// prospective keys are derived from the records themselves.
#[test]
fn new_unique_index_no_cells_duplicate_collides() {
    let root = temp_project("discharge-new-index-collide", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("dup".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("dup".into()));

    let result = witness(&program, &store);

    let index_id = index_catalog_id(&place, "byIsbn");
    assert!(
        matches!(
            verdict_for(&result, &index_id),
            Verdict::RepairRequired { .. }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(result.counts.index_collisions > 0, "{:#?}", result.counts);
}
