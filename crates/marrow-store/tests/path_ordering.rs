//! Saved paths must sort in Marrow order, independent of backend collation.

use marrow_store::path::{PathSegment, SavedKey, decode_key_value, encode_key_value, encode_path};

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

#[test]
fn date_keys_order_chronologically() {
    let day = |y, m, d| marrow_store::value::date_days(y, m, d).expect("valid date");
    let mut encoded: Vec<(Vec<u8>, i32)> = [
        day(2021, 6, 16),
        day(1900, 1, 1), // pre-epoch (negative days)
        day(1999, 12, 31),
        day(2021, 6, 15),
        day(1970, 1, 1), // the epoch itself
    ]
    .into_iter()
    .map(|days| {
        let path = [
            PathSegment::Root("events".into()),
            PathSegment::RecordKey(SavedKey::Date(days)),
        ];
        (encode_path(&path), days)
    })
    .collect();
    encoded.sort_by(|a, b| a.0.cmp(&b.0));
    let order: Vec<i32> = encoded.into_iter().map(|(_, days)| days).collect();
    assert_eq!(
        order,
        vec![
            day(1900, 1, 1),
            day(1970, 1, 1),
            day(1999, 12, 31),
            day(2021, 6, 15),
            day(2021, 6, 16),
        ]
    );
}

#[test]
fn duration_keys_order_by_signed_length() {
    let mut encoded: Vec<(Vec<u8>, i128)> = [1_500_000_000i128, -1_000_000_000, 0, 1, -250_000_000]
        .into_iter()
        .map(|nanos| {
            let path = [
                PathSegment::Root("spans".into()),
                PathSegment::RecordKey(SavedKey::Duration(nanos)),
            ];
            (encode_path(&path), nanos)
        })
        .collect();
    encoded.sort_by(|a, b| a.0.cmp(&b.0));
    let order: Vec<i128> = encoded.into_iter().map(|(_, nanos)| nanos).collect();
    assert_eq!(
        order,
        vec![-1_000_000_000, -250_000_000, 0, 1, 1_500_000_000]
    );
}

#[test]
fn a_key_value_round_trips_through_encode_decode() {
    // Each key variant survives encode then decode unchanged, and reports the
    // full byte length consumed so a concatenation of keys can be walked.
    for key in [
        SavedKey::Bool(true),
        SavedKey::Bool(false),
        SavedKey::Int(0),
        SavedKey::Int(-1),
        SavedKey::Int(i64::MIN),
        SavedKey::Int(i64::MAX),
        SavedKey::Str("isbn:0\u{0}1".into()),
        SavedKey::Bytes(vec![0x00, 0x01, 0xff]),
        SavedKey::Date(-25567),
        SavedKey::Duration(-1_000_000_000),
        SavedKey::Instant(1_700_000_000_000_000_000),
    ] {
        let bytes = encode_key_value(&key);
        assert_eq!(
            decode_key_value(&bytes),
            Some((key.clone(), bytes.len())),
            "round-trip {key:?}"
        );
    }
}

#[test]
fn concatenated_key_values_walk_one_at_a_time() {
    // A composite identity is a run of key values; the reported length lets the
    // next key start exactly where the previous one ended.
    let first = SavedKey::Int(42);
    let second = SavedKey::Str("fiction".into());
    let mut bytes = encode_key_value(&first);
    bytes.extend_from_slice(&encode_key_value(&second));

    let (a, used) = decode_key_value(&bytes).expect("first key");
    assert_eq!(a, first);
    let (b, rest) = decode_key_value(&bytes[used..]).expect("second key");
    assert_eq!(b, second);
    assert_eq!(used + rest, bytes.len(), "no trailing bytes");
}

#[test]
fn decode_key_value_rejects_an_unknown_type_tag() {
    assert_eq!(decode_key_value(&[0xfe, 0x00]), None);
    assert_eq!(decode_key_value(&[]), None);
}
