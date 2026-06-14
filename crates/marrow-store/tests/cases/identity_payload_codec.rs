//! The canonical identity payload codec is the durable byte contract for identity
//! leaves and unique index entries: `encode_identity_payload` -> the exact bytes a
//! record's identity is stored under, and `decode_identity_payload_arity` reads them
//! back at the known arity. These are Tier-0 codec laws — encode/decode is a total
//! round trip over every key type, the arity boundary fails closed on a wrong count
//! or trailing bytes, and the encoded form is a stable fingerprint that a stored
//! payload can be compared against byte-for-byte. Identity-derived index
//! components prefix that payload with the referenced store's stable id.

use marrow_store::key::{
    SavedKey, decode_identity_index_key, decode_identity_payload_arity, encode_identity_index_key,
    encode_identity_payload,
};

/// One identity per scalar key type, plus the byte-escape edge (an embedded `0x00`),
/// so the round trip covers each `SavedKey` variant's encoder and decoder.
fn identities() -> Vec<Vec<SavedKey>> {
    vec![
        vec![],
        vec![SavedKey::Bool(false)],
        vec![SavedKey::Bool(true)],
        vec![SavedKey::Int(0)],
        vec![SavedKey::Int(i64::MIN)],
        vec![SavedKey::Int(i64::MAX)],
        vec![SavedKey::Date(0)],
        vec![SavedKey::Date(-719_162)],
        vec![SavedKey::Instant(i128::MIN)],
        vec![SavedKey::Instant(i128::MAX)],
        vec![SavedKey::Duration(0)],
        vec![SavedKey::Duration(-1)],
        vec![SavedKey::Str(String::new())],
        vec![SavedKey::Str("Dune".into())],
        // An embedded NUL exercises the escaped-byte-run terminator on both sides.
        vec![SavedKey::Str("a\u{0}b".into())],
        vec![SavedKey::Bytes(vec![])],
        vec![SavedKey::Bytes(vec![0x00, 0x00, 0x01, 0xff])],
        vec![
            SavedKey::Int(7),
            SavedKey::Str("title".into()),
            SavedKey::Bool(true),
        ],
    ]
}

#[test]
fn identity_payload_round_trips_at_its_known_arity() {
    for identity in identities() {
        let bytes = encode_identity_payload(&identity);
        let decoded = decode_identity_payload_arity(&bytes, identity.len())
            .expect("identity payload decodes at its own arity");
        assert_eq!(decoded, identity, "round trip lost identity {identity:?}");
    }
}

#[test]
fn decode_rejects_a_wrong_arity() {
    // A payload is addressed by its known key count; reading it at any other arity is
    // a fail-closed mismatch, not a silently truncated or over-read identity.
    let identity = vec![SavedKey::Int(7), SavedKey::Str("title".into())];
    let bytes = encode_identity_payload(&identity);

    // Too few keys leaves trailing bytes; too many runs past the payload end.
    assert_eq!(decode_identity_payload_arity(&bytes, 1), None);
    assert_eq!(decode_identity_payload_arity(&bytes, 3), None);
    // The exact arity is the only one that decodes.
    assert_eq!(
        decode_identity_payload_arity(&bytes, 2),
        Some(identity),
        "the declared arity is the one that reads back",
    );
}

#[test]
fn decode_rejects_trailing_bytes_after_a_complete_identity() {
    // A stored identity payload must be consumed exactly; a single appended byte is a
    // corrupt payload, not an identity the decoder rounds down to.
    let identity = vec![SavedKey::Int(1)];
    let mut bytes = encode_identity_payload(&identity);
    bytes.push(0x00);
    assert_eq!(decode_identity_payload_arity(&bytes, identity.len()), None);
}

#[test]
fn decode_rejects_truncated_key_bytes() {
    // Dropping the last byte of an int key's fixed 9-byte body leaves a key that cannot
    // be read; the decoder fails closed rather than fabricating a value.
    let mut bytes = encode_identity_payload(&[SavedKey::Int(42)]);
    bytes.pop();
    assert_eq!(decode_identity_payload_arity(&bytes, 1), None);
}

#[test]
fn an_empty_identity_round_trips_to_no_keys() {
    let bytes = encode_identity_payload(&[]);
    assert!(bytes.is_empty(), "an empty identity encodes to no bytes");
    assert_eq!(decode_identity_payload_arity(&bytes, 0), Some(vec![]));
    // A non-empty arity over empty bytes is still a fail-closed mismatch.
    assert_eq!(decode_identity_payload_arity(&bytes, 1), None);
}

#[test]
fn the_encoded_form_is_a_stable_byte_fingerprint() {
    // The codec is the durable on-disk shape, so its bytes are pinned exactly. A
    // change here is a storage-format change that orphans every record already keyed
    // under the old bytes, not a free refactor.

    // Bool keys: a one-byte tag then the 0/1 value byte.
    assert_eq!(
        encode_identity_payload(&[SavedKey::Bool(false)]),
        [0x01, 0x00]
    );
    assert_eq!(
        encode_identity_payload(&[SavedKey::Bool(true)]),
        [0x01, 0x01]
    );

    // Int keys: tag `0x02` then 8 order-preserving big-endian bytes (sign bit flipped),
    // so the stored order matches numeric order across the signed range.
    assert_eq!(
        encode_identity_payload(&[SavedKey::Int(0)]),
        [0x02, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    );
    assert_eq!(
        encode_identity_payload(&[SavedKey::Int(i64::MIN)]),
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    );
    assert_eq!(
        encode_identity_payload(&[SavedKey::Int(i64::MAX)]),
        [0x02, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
    );

    // String keys: tag `0x07`, the raw bytes, then the `00 00` run terminator.
    assert_eq!(
        encode_identity_payload(&[SavedKey::Str("hi".into())]),
        [0x07, b'h', b'i', 0x00, 0x00],
    );
    // An embedded NUL is escaped as `00 01` so it cannot be read as the terminator.
    assert_eq!(
        encode_identity_payload(&[SavedKey::Str("a\u{0}b".into())]),
        [0x07, b'a', 0x00, 0x01, b'b', 0x00, 0x00],
    );

    // A multi-key identity is the concatenation of its keys, in order, with no
    // separator: order-preserving tags make the boundary unambiguous.
    assert_eq!(
        encode_identity_payload(&[SavedKey::Bool(true), SavedKey::Str("x".into())]),
        [0x01, 0x01, 0x07, b'x', 0x00, 0x00],
    );
}

#[test]
fn identity_index_keys_prefix_the_referenced_store() {
    let books = "cat_00000000000000000000000000000001";
    let authors = "cat_00000000000000000000000000000002";
    let identity = [SavedKey::Int(7)];

    let book_key = encode_identity_index_key(books, &identity);
    let author_key = encode_identity_index_key(authors, &identity);

    assert_ne!(
        book_key, author_key,
        "same identity payload under different stores must not collide"
    );
    assert_eq!(
        decode_identity_index_key(&book_key, books, 1),
        Some(identity.to_vec())
    );
    assert_eq!(
        decode_identity_index_key(&book_key, authors, 1),
        None,
        "a foreign store prefix is not the same identity component"
    );
}

#[test]
fn identity_index_keys_preserve_identity_order_within_a_store() {
    let store = "cat_00000000000000000000000000000003";
    let one = encode_identity_index_key(store, &[SavedKey::Str("a".into())]);
    let two = encode_identity_index_key(store, &[SavedKey::Str("b".into())]);
    let composite_one = encode_identity_index_key(
        store,
        &[SavedKey::Int(1), SavedKey::Str("section-a".into())],
    );
    let composite_two = encode_identity_index_key(
        store,
        &[SavedKey::Int(1), SavedKey::Str("section-b".into())],
    );

    assert!(one < two);
    assert!(composite_one < composite_two);
}

#[test]
fn the_byte_order_of_int_identities_matches_numeric_order() {
    // The fingerprint claim above is load-bearing because the store is byte-ordered:
    // encoded int identities must sort in numeric order, so range scans over the
    // identity are correct. This pins that the encoding, not just the round trip, holds.
    let mut encoded: Vec<Vec<u8>> = [i64::MIN, -1, 0, 1, i64::MAX]
        .into_iter()
        .map(|n| encode_identity_payload(&[SavedKey::Int(n)]))
        .collect();
    let sorted = {
        let mut copy = encoded.clone();
        copy.sort();
        copy
    };
    assert_eq!(
        encoded, sorted,
        "int identity bytes must sort in numeric order"
    );
    encoded.dedup();
    assert_eq!(encoded.len(), 5, "distinct ints encode to distinct bytes");
}
