//! Conformance KAT (C04): the language comparison order of each temporal value type
//! agrees with the kernel key-codec byte order.
//!
//! A temporal value is a key scalar (`Map[date, V]` and durable temporal keys are
//! admitted), so the order the language `<`/`>` operators observe must be the exact
//! order an ordered-byte engine ranges over the encoded keys. The VM computes a
//! temporal comparison as the raw integer `cmp` (days for a date, nanoseconds for an
//! instant or duration); this test pins that that order equals the order of the
//! kernel's order-preserving key encoding, at the boundaries and across the domain.

use marrow_kernel::codec::key::{KeyScalar, encode_key_value};
use marrow_temporal::{
    SUPPORTED_DATE_MAX_DAYS, SUPPORTED_DATE_MIN_DAYS, SUPPORTED_INSTANT_MAX_NANOS,
    SUPPORTED_INSTANT_MIN_NANOS,
};

/// For a list of key scalars whose `KeyScalar` order is the language order, assert
/// that sorting by the encoded key bytes yields the identical sequence — so the
/// language `<` agrees byte-for-byte with the durable key order.
fn assert_language_order_is_byte_order(values: Vec<KeyScalar>) {
    let mut by_value = values.clone();
    by_value.sort();

    let mut by_bytes = values;
    by_bytes.sort_by_key(encode_key_value);

    assert_eq!(
        by_bytes, by_value,
        "encoded key bytes must sort in the language temporal order"
    );

    // Pairwise: the raw `cmp` (the VM's temporal comparison) equals the encoded-byte
    // `cmp` for every pair, including at the supported boundaries.
    for left in &by_value {
        for right in &by_value {
            assert_eq!(
                left.cmp(right),
                encode_key_value(left).cmp(&encode_key_value(right)),
                "order mismatch for {left:?} and {right:?}"
            );
        }
    }
}

#[test]
fn date_language_order_agrees_with_key_codec_order() {
    assert_language_order_is_byte_order(
        [
            SUPPORTED_DATE_MIN_DAYS,
            -719_162,
            -1,
            0,
            1,
            20_650,
            SUPPORTED_DATE_MAX_DAYS,
        ]
        .into_iter()
        .map(KeyScalar::Date)
        .collect(),
    );
}

#[test]
fn instant_language_order_agrees_with_key_codec_order() {
    assert_language_order_is_byte_order(
        [
            SUPPORTED_INSTANT_MIN_NANOS,
            -1,
            0,
            1,
            1_500_000_000,
            SUPPORTED_INSTANT_MAX_NANOS,
        ]
        .into_iter()
        .map(KeyScalar::Instant)
        .collect(),
    );
}

#[test]
fn duration_language_order_agrees_with_key_codec_order() {
    assert_language_order_is_byte_order(
        [
            i128::MIN,
            -90_000_000_000,
            -1,
            0,
            1,
            90_000_000_000,
            i128::MAX,
        ]
        .into_iter()
        .map(KeyScalar::Duration)
        .collect(),
    );
}
