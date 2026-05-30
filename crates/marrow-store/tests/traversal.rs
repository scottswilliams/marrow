//! Ordered traversal: child keys, roots, and scan return entries in Marrow
//! order regardless of insertion order. The store takes encoded paths, so each
//! call encodes its logical path first.

use marrow_store::backend::Backend;
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
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root("seq".into())]))
        .expect("clean store");
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
    let children = store.child_keys(&encode_path(&book)).expect("clean store");
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
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root("notes".into())]))
        .expect("clean store");
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
fn child_keys_round_trip_date_records() {
    let mut store = MemStore::new();
    let day = |y, m, d| marrow_store::value::date_days(y, m, d).expect("valid date");
    for (y, m, d) in [(2021, 6, 16), (1999, 12, 31), (2021, 6, 15)] {
        let path = [
            PathSegment::Root("events".into()),
            PathSegment::RecordKey(SavedKey::Date(day(y, m, d))),
            PathSegment::Field("note".into()),
        ];
        store.write(&encode_path(&path), b"x".to_vec());
    }
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root("events".into())]))
        .expect("clean store");
    assert_eq!(
        children,
        vec![
            ChildSegment::Key(SavedKey::Date(day(1999, 12, 31))),
            ChildSegment::Key(SavedKey::Date(day(2021, 6, 15))),
            ChildSegment::Key(SavedKey::Date(day(2021, 6, 16))),
        ]
    );
}

#[test]
fn child_keys_round_trip_duration_records() {
    let mut store = MemStore::new();
    for nanos in [1_500_000_000i128, -1_000_000_000, 0] {
        let path = [
            PathSegment::Root("spans".into()),
            PathSegment::RecordKey(SavedKey::Duration(nanos)),
            PathSegment::Field("note".into()),
        ];
        store.write(&encode_path(&path), b"x".to_vec());
    }
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root("spans".into())]))
        .expect("clean store");
    assert_eq!(
        children,
        vec![
            ChildSegment::Key(SavedKey::Duration(-1_000_000_000)),
            ChildSegment::Key(SavedKey::Duration(0)),
            ChildSegment::Key(SavedKey::Duration(1_500_000_000)),
        ]
    );
}

#[test]
fn child_keys_round_trip_instant_records_in_order() {
    use marrow_store::value::{SavedValue, ScalarType, decode_value};
    let at = |text: &str| match decode_value(text.as_bytes(), ScalarType::Instant) {
        Some(SavedValue::Instant(nanos)) => nanos,
        other => panic!("expected an instant, got {other:?}"),
    };
    let mut store = MemStore::new();
    for text in [
        "2026-05-28T12:00:00Z",
        "1969-12-31T23:59:59Z", // pre-epoch sorts first
        "1970-01-01T00:00:00Z",
    ] {
        let path = [
            PathSegment::Root("log".into()),
            PathSegment::RecordKey(SavedKey::Instant(at(text))),
            PathSegment::Field("note".into()),
        ];
        store.write(&encode_path(&path), b"x".to_vec());
    }
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root("log".into())]))
        .expect("clean store");
    assert_eq!(
        children,
        vec![
            ChildSegment::Key(SavedKey::Instant(at("1969-12-31T23:59:59Z"))),
            ChildSegment::Key(SavedKey::Instant(at("1970-01-01T00:00:00Z"))),
            ChildSegment::Key(SavedKey::Instant(at("2026-05-28T12:00:00Z"))),
        ]
    );
}

#[test]
fn child_keys_round_trip_bytes_records() {
    let mut store = MemStore::new();
    // An embedded 0x00 must survive the escaping and still order correctly.
    for bytes in [vec![0x02], vec![0x00, 0xFF], vec![0x01]] {
        let path = [
            PathSegment::Root("blobs".into()),
            PathSegment::RecordKey(SavedKey::Bytes(bytes)),
            PathSegment::Field("note".into()),
        ];
        store.write(&encode_path(&path), b"x".to_vec());
    }
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root("blobs".into())]))
        .expect("clean store");
    assert_eq!(
        children,
        vec![
            ChildSegment::Key(SavedKey::Bytes(vec![0x00, 0xFF])),
            ChildSegment::Key(SavedKey::Bytes(vec![0x01])),
            ChildSegment::Key(SavedKey::Bytes(vec![0x02])),
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
    assert_eq!(
        store.roots().expect("clean store"),
        vec!["books".to_string(), "seq".to_string()]
    );
}

#[test]
fn scan_returns_only_the_subtree_in_order() {
    let mut store = MemStore::new();
    store.write(&encode_path(&field(&seq(1), "title")), b"Dune".to_vec());
    store.write(&encode_path(&field(&seq(1), "author")), b"Herbert".to_vec());
    // A sibling record must not appear in the scan of seq(1).
    store.write(&encode_path(&seq(2)), b"other".to_vec());

    let page = store.scan(&encode_path(&seq(1)), usize::MAX);
    assert!(!page.truncated);
    let paths: Vec<Vec<u8>> = page.entries.into_iter().map(|(key, _)| key).collect();
    assert_eq!(
        paths,
        vec![
            encode_path(&field(&seq(1), "author")),
            encode_path(&field(&seq(1), "title")),
        ]
    );
}
