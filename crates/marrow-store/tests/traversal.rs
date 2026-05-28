//! Ordered traversal: child keys, roots, and scan return entries in Marrow
//! order regardless of insertion order. The store takes encoded paths, so each
//! call encodes its logical path first.

use marrow_store::mem::MemStore;
use marrow_store::path::{ChildSegment, PathSegment, SavedKey, encode_path};

/// The segments of `^seq(n)`.
fn seq(n: i64) -> Vec<PathSegment> {
    vec![
        PathSegment::Root("seq".into()),
        PathSegment::RecordKey(SavedKey::Int(n)),
    ]
}

/// `base` with `field` appended.
fn field(base: &[PathSegment], field: &str) -> Vec<PathSegment> {
    let mut path = base.to_vec();
    path.push(PathSegment::Field(field.into()));
    path
}

#[test]
fn child_keys_lists_integer_records_in_numeric_order() {
    let mut store = MemStore::new();
    // Insert out of order; each record carries one field below it.
    for n in [10, 2, 100, 1] {
        store.write(&encode_path(&field(&seq(n), "v")), b"x".to_vec());
    }
    let children = store.child_keys(&encode_path(&[PathSegment::Root("seq".into())]));
    assert_eq!(
        children,
        vec![
            ChildSegment::Key(SavedKey::Int(1)),
            ChildSegment::Key(SavedKey::Int(2)),
            ChildSegment::Key(SavedKey::Int(10)),
            ChildSegment::Key(SavedKey::Int(100)),
        ]
    );
}

#[test]
fn child_keys_lists_field_names_lexicographically() {
    let mut store = MemStore::new();
    let book = seq(1);
    for name in ["title", "author", "shelf"] {
        store.write(&encode_path(&field(&book, name)), b"x".to_vec());
    }
    let children = store.child_keys(&encode_path(&book));
    assert_eq!(
        children,
        vec![
            ChildSegment::Name("author".into()),
            ChildSegment::Name("shelf".into()),
            ChildSegment::Name("title".into()),
        ]
    );
}

#[test]
fn child_keys_round_trip_string_records() {
    let mut store = MemStore::new();
    for name in ["b", "a", "c"] {
        let path = [
            PathSegment::Root("notes".into()),
            PathSegment::RecordKey(SavedKey::Str(name.into())),
            PathSegment::Field("text".into()),
        ];
        store.write(&encode_path(&path), b"x".to_vec());
    }
    let children = store.child_keys(&encode_path(&[PathSegment::Root("notes".into())]));
    assert_eq!(
        children,
        vec![
            ChildSegment::Key(SavedKey::Str("a".into())),
            ChildSegment::Key(SavedKey::Str("b".into())),
            ChildSegment::Key(SavedKey::Str("c".into())),
        ]
    );
}

#[test]
fn roots_are_listed_in_order_without_duplicates() {
    let mut store = MemStore::new();
    store.write(&encode_path(&seq(1)), b"x".to_vec());
    store.write(&encode_path(&seq(2)), b"x".to_vec());
    store.write(
        &encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
        ]),
        b"x".to_vec(),
    );
    assert_eq!(store.roots(), vec!["books".to_string(), "seq".to_string()]);
}

#[test]
fn scan_returns_only_the_subtree_in_order() {
    let mut store = MemStore::new();
    store.write(&encode_path(&field(&seq(1), "title")), b"Dune".to_vec());
    store.write(&encode_path(&field(&seq(1), "author")), b"Herbert".to_vec());
    // A sibling record must not appear in the scan of seq(1).
    store.write(&encode_path(&seq(2)), b"other".to_vec());

    let paths: Vec<Vec<u8>> = store
        .scan(&encode_path(&seq(1)))
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        paths,
        vec![
            encode_path(&field(&seq(1), "author")),
            encode_path(&field(&seq(1), "title")),
        ]
    );
}
