//! Saved paths must sort in Marrow order, independent of backend collation.

use marrow_store::path::{PathSegment, SavedKey, encode_path};

/// Encode each integer as a `^books(key)` record path and return the keys in
/// encoded-byte order.
fn sorted_int_keys(keys: &[i64]) -> Vec<i64> {
    let mut encoded: Vec<(Vec<u8>, i64)> = keys
        .iter()
        .map(|&key| {
            let path = [
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(key)),
            ];
            (encode_path(&path), key)
        })
        .collect();
    encoded.sort_by(|a, b| a.0.cmp(&b.0));
    encoded.into_iter().map(|(_, key)| key).collect()
}

#[test]
fn integer_record_keys_order_numerically() {
    // Byte order must match numeric order, not decimal-text order: "10" must
    // not sort before "2".
    assert_eq!(sorted_int_keys(&[10, 2, 100, 1, 20]), [1, 2, 10, 20, 100]);
}

#[test]
fn negative_integer_keys_order_before_positive() {
    assert_eq!(sorted_int_keys(&[1, -1, 0, -10, 10]), [-10, -1, 0, 1, 10]);
}

#[test]
fn extreme_integer_keys_order_correctly() {
    assert_eq!(
        sorted_int_keys(&[i64::MAX, 0, i64::MIN, -1, 1]),
        [i64::MIN, -1, 0, 1, i64::MAX]
    );
}

#[test]
fn boolean_keys_order_false_before_true() {
    let f = encode_path(&[PathSegment::RecordKey(SavedKey::Bool(false))]);
    let t = encode_path(&[PathSegment::RecordKey(SavedKey::Bool(true))]);
    assert!(f < t, "false must sort before true");
}

#[test]
fn field_names_sort_lexicographically() {
    let author = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Field("author".into()),
    ]);
    let title = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Field("title".into()),
    ]);
    assert!(author < title, "author must sort before title");
}

#[test]
fn a_field_name_never_collides_with_a_record_key() {
    // A field named "1" and the integer record key 1 must encode distinctly,
    // and a record key (a path component) sorts before a named member.
    let key = encode_path(&[
        PathSegment::Root("x".into()),
        PathSegment::RecordKey(SavedKey::Int(1)),
    ]);
    let field = encode_path(&[
        PathSegment::Root("x".into()),
        PathSegment::Field("1".into()),
    ]);
    assert_ne!(key, field);
    assert!(key < field, "a record key sorts before a named member");
}

/// Encode each string as a `^notes(key)` record path and return the keys in
/// encoded-byte order.
fn sorted_str_keys(keys: &[&str]) -> Vec<String> {
    let mut encoded: Vec<(Vec<u8>, String)> = keys
        .iter()
        .map(|&key| {
            let path = [
                PathSegment::Root("notes".into()),
                PathSegment::RecordKey(SavedKey::Str(key.into())),
            ];
            (encode_path(&path), key.to_string())
        })
        .collect();
    encoded.sort_by(|a, b| a.0.cmp(&b.0));
    encoded.into_iter().map(|(_, key)| key).collect()
}

#[test]
fn string_keys_order_by_utf8_bytes() {
    assert_eq!(
        sorted_str_keys(&["title", "author", "b", "authors"]),
        ["author", "authors", "b", "title"]
    );
}

#[test]
fn a_shorter_string_key_sorts_before_one_that_extends_it() {
    // The 0x00 0x00 terminator makes "a" sort before "a\0" and "ab", and the
    // 0x00 0x01 escape keeps an embedded null from ending the key early.
    assert_eq!(
        sorted_str_keys(&["ab", "a", "a\u{0}"]),
        ["a", "a\u{0}", "ab"]
    );
}
