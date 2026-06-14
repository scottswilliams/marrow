//! Malformed tree-cell value bytes decode to `store.corruption` on every distinct
//! failure point of the catalog-backed enum-member codec, not just a single
//! all-`ff` blob. Each case truncates or corrupts one frame the decoder reads in
//! sequence — version, the enum-id length prefix, the enum-id body, the member-id —
//! plus trailing bytes after a complete value, so each decode branch is exercised.
//!
//! Cell read paths that decode bytes injected at a physical key (malformed node
//! markers, malformed reference payloads, malformed commit metadata, truncated key
//! frames) are covered by the store's in-crate conformance tests because seeding a
//! corrupt physical cell needs the crate-private engine and key-construction
//! substrate, which the public crate surface intentionally does not expose.

use crate::common;
use common::catalog_id;
use marrow_store::tree::{TreeEnumMember, decode_tree_enum_member, encode_tree_enum_member};

/// A well-formed encoded enum-member value to truncate or extend per case.
fn encoded_member() -> Vec<u8> {
    let member = TreeEnumMember::new(
        catalog_id("00000000000000000000000000000001"),
        catalog_id("00000000000000000000000000000002"),
    );
    encode_tree_enum_member(&member).expect("encode enum member")
}

fn assert_corruption(bytes: &[u8]) {
    let error = decode_tree_enum_member(bytes).expect_err("malformed enum-member bytes");
    assert_eq!(error.code(), "store.corruption", "{error:?}");
}

#[test]
fn a_well_formed_enum_member_round_trips() {
    // The positive anchor: the bytes the corruption cases mutate decode cleanly, so a
    // corruption verdict below is the mutation's doing, not a broken fixture.
    let bytes = encoded_member();
    let decoded = decode_tree_enum_member(&bytes).expect("decode enum member");
    assert_eq!(
        decoded.enum_id(),
        &catalog_id("00000000000000000000000000000001")
    );
    assert_eq!(
        decoded.member_id(),
        &catalog_id("00000000000000000000000000000002")
    );
}

#[test]
fn empty_value_bytes_are_corruption() {
    // No version byte to read at all.
    assert_corruption(&[]);
}

#[test]
fn an_unknown_version_byte_is_corruption() {
    // The first byte is the value-codec version; a non-zero version is a value written
    // under a profile this decoder does not speak.
    assert_corruption(&[0xff]);
}

#[test]
fn a_truncated_enum_id_length_prefix_is_corruption() {
    // The version is valid, but the four-byte big-endian length prefix for the enum id
    // is cut short, so the length frame itself cannot be read.
    let bytes = encoded_member();
    assert_corruption(&bytes[..3]);
}

#[test]
fn a_truncated_enum_id_body_is_corruption() {
    // The enum-id length prefix is intact but promises more catalog-id bytes than the
    // value actually carries, so the body read runs past the end.
    let bytes = encoded_member();
    assert_corruption(&bytes[..7]);
}

#[test]
fn a_missing_member_id_is_corruption() {
    // A complete enum id with no following member-id frame: the decoder reads the enum
    // id, then has nothing left for the second required catalog id.
    let full = encoded_member();
    // The enum id occupies a 4-byte length prefix plus its body; cut the value off
    // exactly where the member-id frame would begin.
    let enum_id_len = u32::from_be_bytes([full[1], full[2], full[3], full[4]]) as usize;
    let after_enum_id = 1 + 4 + enum_id_len;
    assert_corruption(&full[..after_enum_id]);
}

#[test]
fn a_truncated_member_id_body_is_corruption() {
    // Both length prefixes are present but the member-id body is cut short.
    let bytes = encoded_member();
    assert_corruption(&bytes[..bytes.len() - 1]);
}

#[test]
fn trailing_bytes_after_a_complete_member_are_corruption() {
    // A complete, valid enum member followed by stray bytes is malformed: the codec is
    // exact and rejects a value that does not end where its frames do.
    let mut bytes = encoded_member();
    bytes.push(0x00);
    assert_corruption(&bytes);
}
