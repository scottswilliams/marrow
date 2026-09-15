//! Frozen known-answer vectors for the durable composite value codec.
//!
//! Each vector names a storable value, the shape it is read back under, and the exact
//! canonical cell bytes. Both directions are pinned: `encode_domain` must emit those
//! bytes, and `decode_domain` must return the same value from them. A representation
//! change therefore fails here as a byte diff against a committed constant rather than
//! as a self-consistent round trip.
//!
//! Scalar cell bytes are pinned by `codec_encoding.rs`; these vectors cover the
//! composite framing built on top of them — product leaf order, sum variant index,
//! nesting, and the `Option` sum — plus the framing's bounds.

use marrow_kernel::codec::value::{
    RuntimeScalar, ScalarKind, ShapeBuildError, ValueError, ValueShape, ValueShapeBuilder,
    decode_domain, encode_domain,
};
use marrow_kernel::equality::ValueDomain;

fn si(v: i64) -> ValueDomain {
    ValueDomain::Scalar(RuntimeScalar::Int(v))
}

fn ss(s: &str) -> ValueDomain {
    ValueDomain::Scalar(RuntimeScalar::Str(s.into()))
}

fn sc(kind: ScalarKind) -> ValueShape {
    ValueShape::scalar(kind)
}

/// A dense product shape of type index `ty` over the given leaf shapes, in declaration
/// order.
fn product_shape(ty: u16, leaves: impl IntoIterator<Item = ValueShape>) -> ValueShape {
    let mut builder = ValueShapeBuilder::new();
    builder.open_product(ty);
    for leaf in leaves {
        builder.shape(leaf);
    }
    builder.close();
    builder.finish().expect("a bounded shape builds")
}

/// A closed sum shape of type index `ty` over per-variant dense payload shapes, in
/// declaration order.
fn sum_shape(ty: u16, variants: impl IntoIterator<Item = Vec<ValueShape>>) -> ValueShape {
    let mut builder = ValueShapeBuilder::new();
    builder.open_sum(ty);
    for payload in variants {
        builder.open_variant();
        for leaf in payload {
            builder.shape(leaf);
        }
        builder.close();
    }
    builder.close();
    builder.finish().expect("a bounded shape builds")
}

fn opt_shape(inner: ValueShape) -> ValueShape {
    sum_shape(0, [vec![], vec![inner]])
}

fn none() -> ValueDomain {
    ValueDomain::Sum {
        ty: 0,
        variant: 0,
        payload: vec![],
    }
}

fn some(inner: ValueDomain) -> ValueDomain {
    ValueDomain::Sum {
        ty: 0,
        variant: 1,
        payload: vec![inner],
    }
}

/// One frozen vector: the value, the shape it decodes under, and its canonical bytes.
struct Vector {
    label: &'static str,
    value: ValueDomain,
    shape: ValueShape,
    bytes: &'static [u8],
}

fn vectors() -> Vec<Vector> {
    vec![
        Vector {
            label: "product of int and NUL-bearing text",
            value: ValueDomain::Product {
                ty: 3,
                fields: vec![Some(si(7)), Some(ss("a\u{0}b"))],
            },
            shape: product_shape(3, [sc(ScalarKind::Int), sc(ScalarKind::Str)]),
            bytes: &[0x01, b'7', 0x03, b'a', 0x00, b'b'],
        },
        Vector {
            label: "product of int and bool",
            value: ValueDomain::Product {
                ty: 1,
                fields: vec![
                    Some(si(-42)),
                    Some(ValueDomain::Scalar(RuntimeScalar::Bool(true))),
                ],
            },
            shape: product_shape(1, [sc(ScalarKind::Int), sc(ScalarKind::Bool)]),
            bytes: &[0x03, b'-', b'4', b'2', 0x01, b'1'],
        },
        Vector {
            label: "product with an empty text leaf",
            value: ValueDomain::Product {
                ty: 2,
                fields: vec![Some(ss("")), Some(si(0))],
            },
            shape: product_shape(2, [sc(ScalarKind::Str), sc(ScalarKind::Int)]),
            bytes: &[0x00, 0x01, b'0'],
        },
        Vector {
            label: "product of temporal leaves",
            value: ValueDomain::Product {
                ty: 6,
                fields: vec![
                    Some(ValueDomain::Scalar(RuntimeScalar::Date(0))),
                    Some(ValueDomain::Scalar(RuntimeScalar::Duration(1_500_000_000))),
                ],
            },
            shape: product_shape(6, [sc(ScalarKind::Date), sc(ScalarKind::Duration)]),
            bytes: &[
                0x0a, b'1', b'9', b'7', b'0', b'-', b'0', b'1', b'-', b'0', b'1', 0x06, b'P', b'T',
                b'1', b'.', b'5', b'S',
            ],
        },
        Vector {
            label: "closed sum, variant 2 with two payload leaves",
            value: ValueDomain::Sum {
                ty: 5,
                variant: 2,
                payload: vec![si(1), ss("x")],
            },
            shape: sum_shape(
                5,
                [
                    vec![],
                    vec![sc(ScalarKind::Int)],
                    vec![sc(ScalarKind::Int), sc(ScalarKind::Str)],
                ],
            ),
            bytes: &[0x02, 0x01, b'1', 0x01, b'x'],
        },
        Vector {
            label: "Option none",
            value: none(),
            shape: opt_shape(sc(ScalarKind::Int)),
            bytes: &[0x00],
        },
        Vector {
            label: "Option some",
            value: some(si(3)),
            shape: opt_shape(sc(ScalarKind::Int)),
            bytes: &[0x01, 0x01, b'3'],
        },
        Vector {
            label: "nested Option, some(none)",
            value: some(none()),
            shape: opt_shape(opt_shape(sc(ScalarKind::Int))),
            bytes: &[0x01, 0x00],
        },
        Vector {
            label: "nested Option, some(some)",
            value: some(some(si(7))),
            shape: opt_shape(opt_shape(sc(ScalarKind::Int))),
            bytes: &[0x01, 0x01, 0x01, b'7'],
        },
        Vector {
            label: "product whose second leaf is an Option[str]",
            value: ValueDomain::Product {
                ty: 3,
                fields: vec![Some(si(1)), Some(some(ss("z")))],
            },
            shape: product_shape(3, [sc(ScalarKind::Int), opt_shape(sc(ScalarKind::Str))]),
            bytes: &[0x01, b'1', 0x01, 0x01, b'z'],
        },
        Vector {
            label: "top-level scalar is the raw scalar codec",
            value: si(i64::MIN),
            shape: sc(ScalarKind::Int),
            bytes: b"-9223372036854775808",
        },
    ]
}

/// The encoder emits exactly the frozen bytes.
#[test]
fn composite_values_encode_to_their_frozen_bytes() {
    for Vector {
        label,
        value,
        bytes,
        ..
    } in vectors()
    {
        let produced = encode_domain(&value).expect("the vector value is storable");
        assert_eq!(produced, bytes, "encode drift for {label}");
    }
}

/// The decoder reads the frozen bytes back to exactly the vector value.
#[test]
fn frozen_bytes_decode_to_their_vector_value() {
    for Vector {
        label,
        value,
        shape,
        bytes,
    } in vectors()
    {
        assert_eq!(
            decode_domain(bytes, &shape),
            Some(value),
            "decode drift for {label}",
        );
    }
}

/// A leaf length prefix is minimal LEB128. A non-minimal spelling of the same length is a
/// forgery the decoder refuses rather than normalizes.
#[test]
fn a_non_minimal_leaf_length_is_refused() {
    let shape = product_shape(0, [sc(ScalarKind::Int), sc(ScalarKind::Int)]);
    assert!(decode_domain(&[0x01, b'6', 0x01, b'7'], &shape).is_some());
    assert_eq!(decode_domain(&[0x80, 0x00, 0x01, b'6'], &shape), None);
}

/// Trailing bytes past the shape's last leaf are a refusal, not a truncation.
#[test]
fn trailing_bytes_are_refused() {
    let shape = product_shape(0, [sc(ScalarKind::Int)]);
    assert_eq!(decode_domain(&[0x01, b'6', 0x00], &shape), None);
}

/// An instant past the supported year range is refused at encode, so no bytes ever reach
/// the store for the decoder to interpret.
#[test]
fn an_out_of_range_instant_is_refused_at_encode() {
    let over = ValueDomain::Scalar(RuntimeScalar::Instant(
        marrow_temporal::SUPPORTED_INSTANT_MAX_NANOS + 1,
    ));
    assert_eq!(
        encode_domain(&over),
        Err(ValueError::InstantOutOfRange {
            nanos: marrow_temporal::SUPPORTED_INSTANT_MAX_NANOS + 1,
        }),
    );
}

/// Depth is counted over composite levels only: a scalar leaf is free, so 32 nested
/// products over one scalar round-trip and the 33rd is refused on both the shape minter
/// and the encoder. Product framing is schema-delimited, so nesting contributes no bytes —
/// a 33-deep value encodes to the same bytes a 32-deep one does, which is why the byte
/// caps can never bound depth.
#[test]
fn thirty_two_composite_levels_are_admitted_and_thirty_three_are_refused() {
    let mut value = si(5);
    let mut shape = sc(ScalarKind::Int);
    for _ in 0..32 {
        value = ValueDomain::Product {
            ty: 0,
            fields: vec![Some(value)],
        };
        shape = product_shape(0, [shape]);
    }
    let bytes = encode_domain(&value).expect("32 composite levels encode");
    assert_eq!(bytes, [0x01, b'5']);
    assert_eq!(decode_domain(&bytes, &shape), Some(value.clone()));

    let deeper_value = ValueDomain::Product {
        ty: 0,
        fields: vec![Some(value)],
    };
    assert_eq!(
        encode_domain(&deeper_value),
        Err(ValueError::ValueTooDeep),
        "the encoder refuses a 33-deep composite",
    );

    let mut deeper = ValueShapeBuilder::new();
    for _ in 0..33 {
        deeper.open_product(0);
    }
    deeper.scalar(ScalarKind::Int);
    for _ in 0..33 {
        deeper.close();
    }
    assert_eq!(
        deeper.finish(),
        Err(ShapeBuildError::TooDeep),
        "the shape minter refuses a 33-deep composite shape",
    );
}

/// A product slot is dense: an absent leaf is not a storable inline value, because
/// optionality inside a struct is an `Option` sum.
#[test]
fn an_absent_product_slot_is_unstorable() {
    let value = ValueDomain::Product {
        ty: 0,
        fields: vec![Some(si(1)), None],
    };
    assert_eq!(encode_domain(&value), Err(ValueError::Unstorable));
}
