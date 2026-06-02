//! A reusable conformance suite for [`Backend`] implementors.
//!
//! Every store — the in-memory store and any persistent backend — must satisfy
//! the same laws over Marrow-ordered encoded paths: value round-trips, the four
//! presence states, subtree deletes, ordered traversal, bounded scans, root
//! listing, bounded raw record copies, typed corruption errors, and transaction savepoints.
//! [`run_all`] drives every law against fresh stores from `make`; a backend's
//! test calls it with its own factory, so memory and native storage are held to
//! one contract.
//!
//! The laws panic on the first violation (naming the law) so a single `#[test]`
//! per backend reports failures with a clear message.

use crate::backend::{Backend, Presence, StoreError};
use crate::path::{ChildSegment, PathSegment, SavedKey, encode_path};

const RAW_COPY_SCAN_LIMIT: usize = 2;

/// Run every conformance law against fresh stores produced by `make`. `make` is
/// `FnMut` so a backend factory can vary state per store (e.g. a redb file name).
pub(crate) fn run_all<B: Backend>(mut make: impl FnMut() -> B) {
    values_round_trip(&mut make());
    presence_reports_four_states(&mut make());
    delete_removes_the_subtree(&mut make());
    delete_of_an_absent_path_is_a_no_op(&mut make());
    child_keys_list_integer_records_in_order(&mut make());
    child_count_matches_immediate_child_key_semantics(&mut make());
    max_int_record_key_returns_the_highest_integer_child(&mut make());
    max_int_record_key_ignores_non_integer_and_named_children(&mut make());
    max_int_record_key_handles_negative_keys(&mut make());
    max_int_record_key_handles_i64_extremes(&mut make());
    max_int_index_key_returns_the_highest_integer_position(&mut make());
    child_keys_dedup_records_with_multiple_descendants(&mut make());
    child_keys_list_field_names_in_order(&mut make());
    child_keys_round_trip_string_records(&mut make());
    child_keys_rev_is_the_exact_reverse_of_child_keys(&mut make());
    child_keys_rev_skips_a_deleted_hole(&mut make());
    next_sibling_skips_gaps_and_subtrees(&mut make());
    prev_sibling_mirrors_next_sibling(&mut make());
    next_sibling_of_last_is_none_and_prev_of_first_is_none(&mut make());
    first_child_and_last_child_match_the_edges(&mut make());
    edge_seeks_skip_a_trailing_named_child(&mut make());
    seeks_are_typed_across_key_types(&mut make());
    roots_are_ordered_and_deduped(&mut make());
    scan_returns_only_the_subtree_in_order(&mut make());
    scan_is_bounded_by_the_limit(&mut make());
    scan_after_resumes_inside_the_subtree(&mut make());
    bounded_raw_copy_reproduces_the_store(&mut make);
    a_corrupt_path_is_a_typed_error(&mut make());
    a_committed_transaction_keeps_its_writes(&mut make());
    a_rolled_back_transaction_discards_its_writes(&mut make());
    an_unbalanced_commit_or_rollback_is_a_no_op(&mut make());
    nested_transactions_are_savepoints(&mut make());
    an_inner_commit_then_outer_rollback_discards_everything(&mut make());
    three_level_nesting_with_a_middle_commit_and_outer_rollback(&mut make());
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

/// One encoded record-key child segment — the `after`/`before` shape the sibling
/// seeks take: the kind tag and the key, exactly as `encode_path` lays a single
/// `RecordKey` segment. (The runtime builds this from a terminal record key.)
fn record_seg(key: SavedKey) -> Vec<u8> {
    encode_path(&[PathSegment::RecordKey(key)])
}

fn collect_raw_records(store: &dyn Backend) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut records = Vec::new();
    let mut cursor = None;
    loop {
        let page = match cursor.as_deref() {
            Some(cursor) => store
                .scan_after(&[], cursor, RAW_COPY_SCAN_LIMIT)
                .expect("scan next raw record page"),
            None => store
                .scan(&[], RAW_COPY_SCAN_LIMIT)
                .expect("scan raw records"),
        };
        let next_cursor = page.entries.last().map(|(path, _)| path.clone());
        records.extend(page.entries);
        if !page.truncated {
            return records;
        }
        if let (Some(previous), Some(next)) = (cursor.as_ref(), next_cursor.as_ref()) {
            assert!(
                next > previous,
                "truncated scan did not advance the raw record cursor"
            );
        }
        cursor = next_cursor;
        assert!(cursor.is_some(), "truncated scan returned no record cursor");
    }
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

fn child_count_matches_immediate_child_key_semantics(store: &mut dyn Backend) {
    assert_eq!(
        store.child_count(&root("seq")).unwrap(),
        0,
        "an absent root has no children"
    );
    store.write(&root("seq"), b"root".to_vec()).unwrap();
    assert_eq!(
        store.child_count(&root("seq")).unwrap(),
        0,
        "the root's own value is not a child"
    );
    store
        .write(&keyed_field("seq", SavedKey::Int(1), "a"), b"x".to_vec())
        .unwrap();
    store
        .write(&keyed_field("seq", SavedKey::Int(1), "b"), b"y".to_vec())
        .unwrap();
    store
        .write(&keyed_field("seq", SavedKey::Int(2), "a"), b"z".to_vec())
        .unwrap();
    store
        .write(
            &encode_path(&[
                PathSegment::Root("seq".into()),
                PathSegment::Field("byName".into()),
                PathSegment::IndexKey(SavedKey::Str("fiction".into())),
            ]),
            b"idx".to_vec(),
        )
        .unwrap();

    let keys = store.child_keys(&root("seq")).unwrap();
    assert_eq!(
        store.child_count(&root("seq")).unwrap(),
        keys.len(),
        "child_count matches child_keys' collapsed immediate children"
    );
}

fn max_int_record_key_returns_the_highest_integer_child(store: &mut dyn Backend) {
    // Empty: no integer record key under the root.
    assert_eq!(
        store.max_int_record_key(&root("seq")).unwrap(),
        None,
        "an empty root has no highest integer key"
    );
    for n in [10, 2, 100, 1] {
        store
            .write(&keyed_field("seq", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    assert_eq!(
        store.max_int_record_key(&root("seq")).unwrap(),
        Some(100),
        "the highest integer record key, bounded"
    );
    // The bounded op must agree with the full child-key walk.
    let from_walk = store
        .child_keys(&root("seq"))
        .unwrap()
        .into_iter()
        .filter_map(|child| match child {
            ChildSegment::Key(SavedKey::Int(value)) => Some(value),
            _ => None,
        })
        .max();
    assert_eq!(
        store.max_int_record_key(&root("seq")).unwrap(),
        from_walk,
        "the bounded op agrees with the full walk"
    );
}

fn max_int_record_key_ignores_non_integer_and_named_children(store: &mut dyn Backend) {
    // An integer record key and a string record key: only the integer counts.
    store
        .write(&keyed_field("mix", SavedKey::Int(5), "v"), b"x".to_vec())
        .unwrap();
    store
        .write(
            &keyed_field("mix", SavedKey::Str("z".into()), "v"),
            b"x".to_vec(),
        )
        .unwrap();
    assert_eq!(
        store.max_int_record_key(&root("mix")).unwrap(),
        Some(5),
        "a string record key is ignored"
    );
    // A root with only a non-integer record key has no highest integer key.
    store
        .write(
            &keyed_field("strs", SavedKey::Str("only".into()), "v"),
            b"x".to_vec(),
        )
        .unwrap();
    assert_eq!(
        store.max_int_record_key(&root("strs")).unwrap(),
        None,
        "only non-integer record keys yields None"
    );
}

fn max_int_record_key_handles_negative_keys(store: &mut dyn Backend) {
    // Negative integer record keys sort below positive ones; the highest wins.
    for n in [-3, 2] {
        store
            .write(&keyed_field("neg", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    assert_eq!(
        store.max_int_record_key(&root("neg")).unwrap(),
        Some(2),
        "the highest of a negative and a positive key"
    );
}

fn max_int_record_key_handles_i64_extremes(store: &mut dyn Backend) {
    // The full signed band, including both extremes: i64::MAX must win and decode
    // back exactly, not wrap or saturate. The sign-flipped big-endian key bodies
    // span all-zero (i64::MIN) to all-one (i64::MAX), so the bounded band lookup
    // must read the whole width to land on the right end.
    for n in [i64::MIN, -1, 0, 1, i64::MAX] {
        store
            .write(&keyed_field("ext", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    assert_eq!(
        store.max_int_record_key(&root("ext")).unwrap(),
        Some(i64::MAX),
        "the highest integer record key is i64::MAX, decoded without wrap"
    );
    // A root holding only i64::MIN must report it, not None or a wrapped value:
    // its all-zero body is the band's low edge and the lone entry.
    store
        .write(
            &keyed_field("floor", SavedKey::Int(i64::MIN), "v"),
            b"x".to_vec(),
        )
        .unwrap();
    assert_eq!(
        store.max_int_record_key(&root("floor")).unwrap(),
        Some(i64::MIN),
        "a lone i64::MIN record key is itself the highest"
    );
}

fn max_int_index_key_returns_the_highest_integer_position(store: &mut dyn Backend) {
    // Positions inside a keyed child layer are index keys, not record keys, so
    // the highest-position op must read the index-key band. `^seq(1).items(pos)`.
    let layer_prefix = encode_path(&[
        PathSegment::Root("seq".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
        PathSegment::ChildLayer("items".into()),
    ]);
    assert_eq!(
        store.max_int_index_key(&layer_prefix).unwrap(),
        None,
        "an empty layer has no highest position"
    );
    for pos in [1, 3, 2] {
        let entry = encode_path(&[
            PathSegment::Root("seq".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::ChildLayer("items".into()),
            PathSegment::IndexKey(SavedKey::Int(pos)),
        ]);
        store.write(&entry, b"x".to_vec()).unwrap();
    }
    assert_eq!(
        store.max_int_index_key(&layer_prefix).unwrap(),
        Some(3),
        "the highest integer index position, bounded"
    );
    // A record key under the same record's root is not an index position and must
    // not bleed into the layer's answer.
    assert_eq!(
        store
            .max_int_index_key(&encode_path(&[PathSegment::Root("seq".into())]))
            .unwrap(),
        None,
        "a root has record keys, not index positions"
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
    let page = store.scan(&book(1), 8).unwrap();
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

fn scan_after_resumes_inside_the_subtree(store: &mut dyn Backend) {
    store
        .write(&book_field(1, "author"), b"x".to_vec())
        .unwrap();
    store.write(&book_field(1, "title"), b"x".to_vec()).unwrap();
    store.write(&book(2), b"outside".to_vec()).unwrap();

    let first = store.scan(&book(1), 1).unwrap();
    assert_eq!(first.entries.len(), 1);
    assert!(first.truncated);
    let cursor = first.entries[0].0.clone();

    let second = store.scan_after(&book(1), &cursor, 1).unwrap();
    assert_eq!(
        second.entries,
        vec![(book_field(1, "title"), b"x".to_vec())],
        "resume stays inside the requested subtree"
    );
    assert!(!second.truncated);
}

fn bounded_raw_copy_reproduces_the_store<B: Backend>(make: &mut impl FnMut() -> B) {
    let mut source = make();
    source.write(&book(1), b"whole".to_vec()).unwrap();
    source
        .write(&book_field(1, "title"), b"Dune".to_vec())
        .unwrap();
    source
        .write(&book_field(2, "title"), b"Sand".to_vec())
        .unwrap();

    let source_records = collect_raw_records(&source);
    assert_eq!(source_records.len(), 3);
    let mut copied = make();
    for (path, value) in &source_records {
        copied.write(path, value.clone()).unwrap();
    }
    assert_eq!(
        collect_raw_records(&copied),
        source_records,
        "raw records copied"
    );
    assert_eq!(
        copied.roots().unwrap(),
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

fn an_unbalanced_commit_or_rollback_is_a_no_op(store: &mut dyn Backend) {
    // With no open transaction, commit and rollback are no-ops: callers pair
    // begin with commit/rollback, so an extra one is a harmless misuse, not an
    // error. (One contract across backends; mem and redb agree.)
    store.commit().unwrap();
    store.rollback().unwrap();
    // The store still works normally afterward.
    store.write(&book(1), b"v".to_vec()).unwrap();
    assert_eq!(store.read(&book(1)).unwrap(), Some(b"v".to_vec()));
    // A balanced begin/commit after the stray calls still behaves.
    store.begin().unwrap();
    store.write(&book(2), b"w".to_vec()).unwrap();
    store.commit().unwrap();
    assert_eq!(store.read(&book(2)).unwrap(), Some(b"w".to_vec()));
    // And a trailing stray commit/rollback remains a no-op.
    store.commit().unwrap();
    store.rollback().unwrap();
    assert_eq!(store.read(&book(1)).unwrap(), Some(b"v".to_vec()));
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

fn an_inner_commit_then_outer_rollback_discards_everything(store: &mut dyn Backend) {
    // An inner commit is not durable on its own: it merely closes the inner
    // savepoint, leaving its writes riding the still-open outer transaction. So a
    // write before the inner begin, a write committed by the inner savepoint, and
    // a write after it must all vanish when the outer transaction rolls back.
    store.begin().unwrap(); // outer
    store.write(&book(1), b"A".to_vec()).unwrap();
    store.begin().unwrap(); // inner
    store.write(&book(2), b"B".to_vec()).unwrap();
    store.commit().unwrap(); // inner commit: B rides the open outer transaction
    store.write(&book(3), b"C".to_vec()).unwrap();
    store.rollback().unwrap(); // outer rollback discards A, B, and C
    assert_eq!(
        store.read(&book(1)).unwrap(),
        None,
        "A before the inner begin"
    );
    assert_eq!(
        store.read(&book(2)).unwrap(),
        None,
        "B committed by the inner savepoint"
    );
    assert_eq!(
        store.read(&book(3)).unwrap(),
        None,
        "C after the inner commit"
    );
}

fn three_level_nesting_with_a_middle_commit_and_outer_rollback(store: &mut dyn Backend) {
    // Three stacked savepoints: committing the innermost two folds their writes
    // outward, but the outermost rollback still discards the whole transaction, so
    // every level's write disappears.
    store.begin().unwrap(); // L1
    store.write(&book(1), b"A".to_vec()).unwrap();
    store.begin().unwrap(); // L2
    store.write(&book(2), b"B".to_vec()).unwrap();
    store.begin().unwrap(); // L3
    store.write(&book(3), b"C".to_vec()).unwrap();
    store.commit().unwrap(); // commit L3: C folds into L2
    store.commit().unwrap(); // commit L2: B and C fold into L1
    store.rollback().unwrap(); // rollback L1 discards A, B, and C
    assert_eq!(store.read(&book(1)).unwrap(), None, "L1 write");
    assert_eq!(
        store.read(&book(2)).unwrap(),
        None,
        "L2 write committed into L1"
    );
    assert_eq!(
        store.read(&book(3)).unwrap(),
        None,
        "L3 write committed into L1"
    );
}

fn child_keys_rev_is_the_exact_reverse_of_child_keys(store: &mut dyn Backend) {
    // A mix of key types under one root (booleans, integers, dates, strings)
    // exercises the typed key order in both directions. child_keys_rev must be the
    // exact reverse of child_keys — not an independently ordered list.
    for key in [
        SavedKey::Bool(true),
        SavedKey::Int(-3),
        SavedKey::Int(7),
        SavedKey::Date(19_000),
        SavedKey::Str("a".into()),
        SavedKey::Str("m".into()),
    ] {
        store
            .write(&keyed_field("rev", key, "v"), b"x".to_vec())
            .unwrap();
    }
    let forward = store.child_keys(&root("rev")).unwrap();
    let mut expected = forward.clone();
    expected.reverse();
    assert_eq!(
        store.child_keys_rev(&root("rev")).unwrap(),
        expected,
        "child_keys_rev is the exact reverse of child_keys"
    );
    // A record with several descendants still collapses to one child in reverse.
    store
        .write(&keyed_field("rev", SavedKey::Int(7), "w"), b"x".to_vec())
        .unwrap();
    let mut expected = store.child_keys(&root("rev")).unwrap();
    expected.reverse();
    assert_eq!(
        store.child_keys_rev(&root("rev")).unwrap(),
        expected,
        "a multi-descendant record collapses to one child in reverse too"
    );
}

fn child_keys_rev_skips_a_deleted_hole(store: &mut dyn Backend) {
    // Stored keys 1, 2, 3; delete the middle one. Both orders omit the hole — the
    // gap-skipping iteration invariant — and stay exact reverses of each other.
    for n in [1, 2, 3] {
        store
            .write(&keyed_field("holes", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    store
        .delete(&encode_path(&[
            PathSegment::Root("holes".into()),
            PathSegment::RecordKey(SavedKey::Int(2)),
        ]))
        .unwrap();
    assert_eq!(
        store.child_keys_rev(&root("holes")).unwrap(),
        vec![
            ChildSegment::Key(SavedKey::Int(3)),
            ChildSegment::Key(SavedKey::Int(1)),
        ],
        "the deleted middle key is absent from the reverse walk"
    );
}

fn next_sibling_skips_gaps_and_subtrees(store: &mut dyn Backend) {
    // Children 1, 2, 5 where 5 carries its own descendants. After deleting 2, the
    // next sibling of 1 is 5 (the hole at 2 is skipped) and the seek returns 5 the
    // child, never one of 5's grandchildren (its subtree is stepped over).
    for n in [1, 2] {
        store
            .write(&keyed_field("seq", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    // Give 5 a deep subtree: a field and a nested child-layer entry.
    store
        .write(&keyed_field("seq", SavedKey::Int(5), "v"), b"x".to_vec())
        .unwrap();
    store
        .write(
            &encode_path(&[
                PathSegment::Root("seq".into()),
                PathSegment::RecordKey(SavedKey::Int(5)),
                PathSegment::ChildLayer("items".into()),
                PathSegment::IndexKey(SavedKey::Int(99)),
                PathSegment::Field("v".into()),
            ]),
            b"x".to_vec(),
        )
        .unwrap();
    store
        .delete(&encode_path(&[
            PathSegment::Root("seq".into()),
            PathSegment::RecordKey(SavedKey::Int(2)),
        ]))
        .unwrap();
    assert_eq!(
        store
            .next_sibling(&root("seq"), &record_seg(SavedKey::Int(1)))
            .unwrap(),
        Some(ChildSegment::Key(SavedKey::Int(5))),
        "the next stored sibling, skipping the deleted 2 and 5's own subtree"
    );
    // The next sibling of 5 itself is none — 5 is now the last child.
    assert_eq!(
        store
            .next_sibling(&root("seq"), &record_seg(SavedKey::Int(5)))
            .unwrap(),
        None,
        "no sibling after the last child"
    );
}

fn prev_sibling_mirrors_next_sibling(store: &mut dyn Backend) {
    // Children 1, 2, 5 with 5 holding a subtree. The previous sibling of 5 is 2,
    // and of 2 is 1; the previous sibling of 1 is none. Each prev_sibling result
    // matches the next_sibling that points back at it.
    for n in [1, 2] {
        store
            .write(&keyed_field("seq", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    store
        .write(&keyed_field("seq", SavedKey::Int(5), "v"), b"x".to_vec())
        .unwrap();
    store
        .write(
            &encode_path(&[
                PathSegment::Root("seq".into()),
                PathSegment::RecordKey(SavedKey::Int(5)),
                PathSegment::Field("deep".into()),
            ]),
            b"x".to_vec(),
        )
        .unwrap();
    assert_eq!(
        store
            .prev_sibling(&root("seq"), &record_seg(SavedKey::Int(5)))
            .unwrap(),
        Some(ChildSegment::Key(SavedKey::Int(2))),
        "the previous sibling of 5 is 2, stepping over nothing in between"
    );
    assert_eq!(
        store
            .prev_sibling(&root("seq"), &record_seg(SavedKey::Int(2)))
            .unwrap(),
        Some(ChildSegment::Key(SavedKey::Int(1))),
        "the previous sibling of 2 is 1"
    );
    assert_eq!(
        store
            .prev_sibling(&root("seq"), &record_seg(SavedKey::Int(1)))
            .unwrap(),
        None,
        "no sibling before the first child"
    );
    // next and prev are inverses: next of 2 is 5, prev of 5 is 2.
    assert_eq!(
        store
            .next_sibling(&root("seq"), &record_seg(SavedKey::Int(2)))
            .unwrap(),
        Some(ChildSegment::Key(SavedKey::Int(5))),
        "next of 2 is 5"
    );
}

fn next_sibling_of_last_is_none_and_prev_of_first_is_none(store: &mut dyn Backend) {
    // A single child has no sibling either way.
    store
        .write(&keyed_field("solo", SavedKey::Int(42), "v"), b"x".to_vec())
        .unwrap();
    assert_eq!(
        store
            .next_sibling(&root("solo"), &record_seg(SavedKey::Int(42)))
            .unwrap(),
        None,
        "a lone child has no next sibling"
    );
    assert_eq!(
        store
            .prev_sibling(&root("solo"), &record_seg(SavedKey::Int(42)))
            .unwrap(),
        None,
        "a lone child has no previous sibling"
    );
}

fn first_child_and_last_child_match_the_edges(store: &mut dyn Backend) {
    // The edge seeks serve `next`/`prev`, which navigate key positions, so they
    // report the first/last *key* child. Empty: no edges. Then key children, all of
    // which are key positions, so the edges match `child_keys`' ends.
    assert_eq!(
        store.first_child(&root("edge")).unwrap(),
        None,
        "an empty parent has no first child"
    );
    assert_eq!(
        store.last_child(&root("edge")).unwrap(),
        None,
        "an empty parent has no last child"
    );
    for n in [3, 1, 2] {
        store
            .write(&keyed_field("edge", SavedKey::Int(n), "v"), b"x".to_vec())
            .unwrap();
    }
    let children = store.child_keys(&root("edge")).unwrap();
    assert_eq!(
        store.first_child(&root("edge")).unwrap().as_ref(),
        children.first(),
        "first_child is child_keys' first"
    );
    assert_eq!(
        store.last_child(&root("edge")).unwrap().as_ref(),
        children.last(),
        "last_child is child_keys' last"
    );
    // A record whose children are all named fields has no navigable key child, so
    // the edge seeks skip past its own value entry and its fields alike to `None` —
    // `next`/`prev` never address a field position.
    store.write(&book(7), b"whole".to_vec()).unwrap();
    store.write(&book_field(7, "title"), b"x".to_vec()).unwrap();
    assert_eq!(
        store.first_child(&book(7)).unwrap(),
        None,
        "first_child skips the record's value entry and its named fields"
    );
    assert_eq!(
        store.last_child(&book(7)).unwrap(),
        None,
        "last_child likewise finds no key child among named fields"
    );
}

fn edge_seeks_skip_a_trailing_named_child(store: &mut dyn Backend) {
    // A keyed root that also declares an index stores the index as a *named* child
    // sorting after the record-key children. `next`/`prev` navigate key positions
    // only, so the edge seeks must skip the index: stepping past the last record key
    // is `None` (the catchable edge), `last_child` is that last record key — not the
    // index name — and `first_child` is the first record key.
    for id in [1, 2, 4] {
        store
            .write(&keyed_field("idx", SavedKey::Int(id), "v"), b"x".to_vec())
            .unwrap();
    }
    // The trailing named child: `^idx.byShelf("a")` with its own key subtree, the
    // exact shape a declared index lays down after the record keys.
    let index_entry = encode_path(&[
        PathSegment::Root("idx".into()),
        PathSegment::Index("byShelf".into()),
        PathSegment::IndexKey(SavedKey::Str("a".into())),
    ]);
    store.write(&index_entry, b"x".to_vec()).unwrap();

    // `next` past the last record key (4) is the catchable edge, not the index name.
    assert_eq!(
        store
            .next_sibling(&root("idx"), &record_seg(SavedKey::Int(4)))
            .unwrap(),
        None,
        "next past the last record skips the trailing index name"
    );
    // `last_child` is the last record key, stepping back over the index name.
    assert_eq!(
        store.last_child(&root("idx")).unwrap(),
        Some(ChildSegment::Key(SavedKey::Int(4))),
        "last_child is the last record key, not the index name"
    );
    // `first_child` is the first record key (it sorts before the index name).
    assert_eq!(
        store.first_child(&root("idx")).unwrap(),
        Some(ChildSegment::Key(SavedKey::Int(1))),
        "first_child is the first record key"
    );
}

fn seeks_are_typed_across_key_types(store: &mut dyn Backend) {
    // Every order-preserving key type seeks correctly: each backend orders the raw
    // encoded bytes, so the typed key order falls out for free. For each type write
    // two adjacent keys and assert next/prev step between them.
    let pairs: [(SavedKey, SavedKey); 5] = [
        (SavedKey::Int(-5), SavedKey::Int(9)),
        (SavedKey::Date(100), SavedKey::Date(20_000)),
        (SavedKey::Duration(1_000), SavedKey::Duration(2_000_000_000)),
        (
            SavedKey::Instant(1_000),
            SavedKey::Instant(1_700_000_000_000_000_000),
        ),
        (
            SavedKey::Bytes(vec![0x01]),
            SavedKey::Bytes(vec![0x01, 0x02]),
        ),
    ];
    for (lo, hi) in pairs {
        // A fresh root per type so unrelated key types never share a parent.
        let name = format!("typed-{}", lo.wire_tag());
        store
            .write(&keyed_field(&name, lo.clone(), "v"), b"x".to_vec())
            .unwrap();
        store
            .write(&keyed_field(&name, hi.clone(), "v"), b"x".to_vec())
            .unwrap();
        assert_eq!(
            store
                .next_sibling(&root(&name), &record_seg(lo.clone()))
                .unwrap(),
            Some(ChildSegment::Key(hi.clone())),
            "next steps from the lower {} key to the higher",
            lo.wire_tag()
        );
        assert_eq!(
            store
                .prev_sibling(&root(&name), &record_seg(hi.clone()))
                .unwrap(),
            Some(ChildSegment::Key(lo.clone())),
            "prev steps from the higher {} key to the lower",
            lo.wire_tag()
        );
    }
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
        store.scan(&book(1), 8).unwrap().entries.len(),
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
