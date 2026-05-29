//! A reusable conformance suite for [`Backend`] implementors.
//!
//! Every store — the in-memory store and any persistent backend — must satisfy
//! the same laws over Marrow-ordered encoded paths: value round-trips, the four
//! presence states, subtree deletes, ordered traversal, bounded scans, root
//! listing, dump/restore, typed corruption errors, and transaction savepoints.
//! [`run_all`] drives every law against fresh stores from `make`; a backend's
//! test calls it with its own factory, so memory and native storage are held to
//! one contract.
//!
//! The laws panic on the first violation (naming the law) so a single `#[test]`
//! per backend reports failures with a clear message.

use crate::backend::Backend;
use crate::mem::{Presence, StoreError};
use crate::path::{ChildSegment, PathSegment, SavedKey, encode_path};

/// Run every conformance law against fresh stores produced by `make`. `make` is
/// `FnMut` so a backend factory can vary state per store (e.g. a redb file name).
pub fn run_all<B: Backend>(mut make: impl FnMut() -> B) {
    values_round_trip(&mut make());
    presence_reports_four_states(&mut make());
    delete_removes_the_subtree(&mut make());
    delete_of_an_absent_path_is_a_no_op(&mut make());
    child_keys_list_integer_records_in_order(&mut make());
    child_keys_dedup_records_with_multiple_descendants(&mut make());
    child_keys_list_field_names_in_order(&mut make());
    child_keys_round_trip_string_records(&mut make());
    roots_are_ordered_and_deduped(&mut make());
    scan_returns_only_the_subtree_in_order(&mut make());
    scan_is_bounded_by_the_limit(&mut make());
    dump_and_restore_reproduce_the_store(&mut make);
    a_corrupt_path_is_a_typed_error(&mut make());
    a_committed_transaction_keeps_its_writes(&mut make());
    a_rolled_back_transaction_discards_its_writes(&mut make());
    nested_transactions_are_savepoints(&mut make());
    a_transaction_sees_its_writes_in_traversal(&mut make());
}

/// The encoded path `^root`.
fn root(name: &str) -> Vec<u8> {
    encode_path(&[PathSegment::Root(name.into())])
}

/// The encoded path `^books(id)`.
fn book(id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
    ])
}

/// The encoded path `^books(id).field`.
fn book_field(id: i64, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field(field.into()),
    ])
}

/// The encoded path `^root(key).field`, for traversal laws over record keys.
fn keyed_field(root: &str, key: SavedKey, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root(root.into()),
        PathSegment::RecordKey(key),
        PathSegment::Field(field.into()),
    ])
}

fn values_round_trip(store: &mut dyn Backend) {
    assert_eq!(store.read(&book_field(1, "title")).unwrap(), None, "absent");
    store
        .write(&book_field(1, "title"), b"draft".to_vec())
        .unwrap();
    assert_eq!(
        store.read(&book_field(1, "title")).unwrap(),
        Some(b"draft".to_vec()),
        "write then read"
    );
    store
        .write(&book_field(1, "title"), b"final".to_vec())
        .unwrap();
    assert_eq!(
        store.read(&book_field(1, "title")).unwrap(),
        Some(b"final".to_vec()),
        "write replaces"
    );
}

fn presence_reports_four_states(store: &mut dyn Backend) {
    assert_eq!(store.presence(&book(1)).unwrap(), Presence::Absent);
    store.write(&book(1), b"whole".to_vec()).unwrap();
    assert_eq!(store.presence(&book(1)).unwrap(), Presence::ValueOnly);
    store
        .write(&book_field(1, "title"), b"Dune".to_vec())
        .unwrap();
    assert_eq!(
        store.presence(&book(1)).unwrap(),
        Presence::ValueAndChildren
    );
    store
        .write(&book_field(2, "title"), b"Sand".to_vec())
        .unwrap();
    assert_eq!(store.presence(&book(2)).unwrap(), Presence::ChildrenOnly);
}

fn delete_removes_the_subtree(store: &mut dyn Backend) {
    store.write(&book(1), b"whole".to_vec()).unwrap();
    store
        .write(&book_field(1, "title"), b"Dune".to_vec())
        .unwrap();
    store
        .write(&book_field(2, "title"), b"Other".to_vec())
        .unwrap();
    store.delete(&book(1)).unwrap();
    assert_eq!(store.presence(&book(1)).unwrap(), Presence::Absent);
    assert_eq!(store.read(&book_field(1, "title")).unwrap(), None);
    assert_eq!(
        store.read(&book_field(2, "title")).unwrap(),
        Some(b"Other".to_vec()),
        "a sibling record is untouched"
    );
}

fn delete_of_an_absent_path_is_a_no_op(store: &mut dyn Backend) {
    store
        .write(&book_field(2, "title"), b"Other".to_vec())
        .unwrap();
    store.delete(&book(1)).unwrap();
    assert_eq!(
        store.read(&book_field(2, "title")).unwrap(),
        Some(b"Other".to_vec())
    );
}

fn child_keys_list_integer_records_in_order(store: &mut dyn Backend) {
    for n in [10, 2, 100, 1] {
        store
            .write(&keyed_field("seq", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    assert_eq!(
        store.child_keys(&root("seq")).unwrap(),
        vec![
            ChildSegment::Key(SavedKey::Int(1)),
            ChildSegment::Key(SavedKey::Int(2)),
            ChildSegment::Key(SavedKey::Int(10)),
            ChildSegment::Key(SavedKey::Int(100)),
        ]
    );
}

fn child_keys_dedup_records_with_multiple_descendants(store: &mut dyn Backend) {
    // A record can have several descendants (^seq(1).a, ^seq(1).b); the parent's
    // child_keys must collapse those to one entry per immediate child. With one
    // descendant per record the dedup branch never fires, so this law gives ^seq(1)
    // two fields before listing ^seq's children.
    store
        .write(&keyed_field("seq", SavedKey::Int(1), "a"), b"x".to_vec())
        .unwrap();
    store
        .write(&keyed_field("seq", SavedKey::Int(1), "b"), b"x".to_vec())
        .unwrap();
    store
        .write(&keyed_field("seq", SavedKey::Int(2), "a"), b"x".to_vec())
        .unwrap();
    assert_eq!(
        store.child_keys(&root("seq")).unwrap(),
        vec![
            ChildSegment::Key(SavedKey::Int(1)),
            ChildSegment::Key(SavedKey::Int(2)),
        ],
        "a record with multiple descendants appears once among its parent's children"
    );
}

fn child_keys_list_field_names_in_order(store: &mut dyn Backend) {
    for name in ["title", "author", "shelf"] {
        store.write(&book_field(1, name), b"x".to_vec()).unwrap();
    }
    assert_eq!(
        store.child_keys(&book(1)).unwrap(),
        vec![
            ChildSegment::Name("author".into()),
            ChildSegment::Name("shelf".into()),
            ChildSegment::Name("title".into()),
        ]
    );
}

fn child_keys_round_trip_string_records(store: &mut dyn Backend) {
    for name in ["b", "a", "c"] {
        store
            .write(
                &keyed_field("notes", SavedKey::Str(name.into()), "text"),
                b"x".to_vec(),
            )
            .unwrap();
    }
    assert_eq!(
        store.child_keys(&root("notes")).unwrap(),
        vec![
            ChildSegment::Key(SavedKey::Str("a".into())),
            ChildSegment::Key(SavedKey::Str("b".into())),
            ChildSegment::Key(SavedKey::Str("c".into())),
        ]
    );
}

fn roots_are_ordered_and_deduped(store: &mut dyn Backend) {
    store
        .write(&keyed_field("seq", SavedKey::Int(1), "v"), b"x".to_vec())
        .unwrap();
    store
        .write(&keyed_field("seq", SavedKey::Int(2), "v"), b"x".to_vec())
        .unwrap();
    store.write(&book(1), b"x".to_vec()).unwrap();
    assert_eq!(
        store.roots().unwrap(),
        vec!["books".to_string(), "seq".to_string()]
    );
}

fn scan_returns_only_the_subtree_in_order(store: &mut dyn Backend) {
    store
        .write(&book_field(1, "title"), b"Dune".to_vec())
        .unwrap();
    store
        .write(&book_field(1, "author"), b"Herbert".to_vec())
        .unwrap();
    store.write(&book(2), b"other".to_vec()).unwrap(); // sibling must not appear
    let page = store.scan(&book(1), usize::MAX).unwrap();
    assert!(!page.truncated);
    let paths: Vec<Vec<u8>> = page.entries.into_iter().map(|(key, _)| key).collect();
    assert_eq!(
        paths,
        vec![book_field(1, "author"), book_field(1, "title")],
        "subtree only, in Marrow order"
    );
}

fn scan_is_bounded_by_the_limit(store: &mut dyn Backend) {
    for n in 1..=5 {
        store.write(&book_field(n, "title"), b"x".to_vec()).unwrap();
    }
    let page = store.scan(&[], 3).unwrap();
    assert_eq!(page.entries.len(), 3);
    assert!(page.truncated, "a limit below the total truncates");
    let page = store.scan(&[], 5).unwrap();
    assert_eq!(page.entries.len(), 5);
    assert!(!page.truncated, "a limit at the total does not");
}

fn dump_and_restore_reproduce_the_store<B: Backend>(make: &mut impl FnMut() -> B) {
    let mut source = make();
    source.write(&book(1), b"whole".to_vec()).unwrap();
    source
        .write(&book_field(1, "title"), b"Dune".to_vec())
        .unwrap();
    source
        .write(&book_field(2, "title"), b"Sand".to_vec())
        .unwrap();

    // Dump the portable path/value stream from the empty prefix, then replay it
    // into a fresh store.
    let dump = source.scan(&[], usize::MAX).unwrap();
    let mut restored = make();
    for (path, value) in &dump.entries {
        restored.write(path, value.clone()).unwrap();
    }
    assert_eq!(
        restored.scan(&[], usize::MAX).unwrap(),
        dump,
        "dump reproduced"
    );
    assert_eq!(
        restored.roots().unwrap(),
        source.roots().unwrap(),
        "roots reproduced"
    );
}

fn a_corrupt_path_is_a_typed_error(store: &mut dyn Backend) {
    // A key that is not a valid segment sequence: 0xFF is not a kind tag.
    store.write(&[0xFF], b"x".to_vec()).unwrap();
    assert!(
        matches!(store.roots(), Err(StoreError::CorruptPath { .. })),
        "a corrupt root is a typed error"
    );
}

fn a_committed_transaction_keeps_its_writes(store: &mut dyn Backend) {
    store.begin().unwrap();
    store.write(&book(1), b"1".to_vec()).unwrap();
    assert_eq!(
        store.read(&book(1)).unwrap(),
        Some(b"1".to_vec()),
        "read-your-writes inside the transaction"
    );
    store.commit().unwrap();
    assert_eq!(store.read(&book(1)).unwrap(), Some(b"1".to_vec()));
}

fn a_rolled_back_transaction_discards_its_writes(store: &mut dyn Backend) {
    store.write(&book(1), b"kept".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(&book(2), b"staged".to_vec()).unwrap();
    store.rollback().unwrap();
    assert_eq!(
        store.read(&book(2)).unwrap(),
        None,
        "staged write rolled back"
    );
    assert_eq!(
        store.read(&book(1)).unwrap(),
        Some(b"kept".to_vec()),
        "the pre-transaction value remains"
    );
}

fn nested_transactions_are_savepoints(store: &mut dyn Backend) {
    store.begin().unwrap();
    store.write(&book(1), b"outer".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(&book(2), b"inner".to_vec()).unwrap();
    store.rollback().unwrap(); // undo the inner savepoint only
    assert_eq!(store.read(&book(2)).unwrap(), None, "inner rolled back");
    assert_eq!(
        store.read(&book(1)).unwrap(),
        Some(b"outer".to_vec()),
        "outer write survives the inner rollback"
    );
    store.commit().unwrap();
    assert_eq!(store.read(&book(1)).unwrap(), Some(b"outer".to_vec()));
}

fn a_transaction_sees_its_writes_in_traversal(store: &mut dyn Backend) {
    store.begin().unwrap();
    store
        .write(&book_field(1, "title"), b"staged".to_vec())
        .unwrap();
    // Presence, child keys, and scans inside the transaction reflect the staged
    // write, not just point reads.
    assert_eq!(
        store.presence(&book(1)).unwrap(),
        Presence::ChildrenOnly,
        "presence sees the staged child"
    );
    assert_eq!(
        store.child_keys(&book(1)).unwrap(),
        vec![ChildSegment::Name("title".into())],
        "child_keys sees the staged field"
    );
    assert_eq!(
        store.scan(&book(1), usize::MAX).unwrap().entries.len(),
        1,
        "scan sees the staged entry"
    );
    store.rollback().unwrap();
    assert_eq!(
        store.presence(&book(1)).unwrap(),
        Presence::Absent,
        "rollback reverts traversal too"
    );
}
