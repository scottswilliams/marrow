//! Independent-decoder differential for the widened durable field-value codec (E03w slice E,
//! step 5). The module under `support/` is an independent strict decoder written from the
//! design brief alone — it never reads the production encoder. This test vendors that module
//! with only rustfmt whitespace normalization applied (the workspace fmt gate is mandatory);
//! its decode logic and decisions are byte-for-byte the author's, unaltered and verified by
//! its own embedded KATs. The harness wires only glue: it encodes a corpus of storable values
//! with the production `encode_domain`, decodes the bytes with the independent module, and
//! asserts value agreement. A disagreement is a finding to report, never a fix-in-place; the
//! vendored decode logic is not edited here.
//!
//! Including the module here also compiles and runs its own embedded KATs (its
//! `#[cfg(test)] mod tests`), so the brief's KAT byte strings are checked from the
//! independent side in this binary.

// The vendored oracle keeps the author's own form (dead_code is allowed inside the file).
// Its idioms are suppressed at this boundary rather than rewritten, so the independent
// decoder is never edited toward this workspace's lints.
#[path = "support/independent_decoder.rs"]
#[allow(clippy::doc_overindented_list_items, clippy::manual_range_contains)]
mod independent_decoder;

use independent_decoder as ind;
use marrow_kernel::codec::value::{
    RuntimeScalar, ScalarKind, ValueShape, encode_domain, encode_value,
};
use marrow_kernel::equality::ValueDomain;

/// Convert the production value shape into the independent decoder's schema shape. The
/// independent `Shape` carries no type index (a differential compares values, not the
/// image's `ty` handles), so the indices are dropped and only the structural shape crosses.
fn to_ind_shape(shape: &ValueShape) -> ind::Shape {
    match shape {
        ValueShape::Scalar(kind) => ind::Shape::Scalar(to_ind_kind(*kind)),
        ValueShape::Product { fields, .. } => {
            ind::Shape::Product(fields.iter().map(to_ind_shape).collect())
        }
        ValueShape::Sum { variants, .. } => ind::Shape::Sum(
            variants
                .iter()
                .map(|payload| payload.iter().map(to_ind_shape).collect())
                .collect(),
        ),
    }
}

fn to_ind_kind(kind: ScalarKind) -> ind::ScalarKind {
    match kind {
        ScalarKind::Bool => ind::ScalarKind::Bool,
        ScalarKind::Int => ind::ScalarKind::Int,
        ScalarKind::Str => ind::ScalarKind::Text,
        ScalarKind::Bytes => ind::ScalarKind::Bytes,
        ScalarKind::Date => ind::ScalarKind::Date,
        ScalarKind::Duration => ind::ScalarKind::Duration,
        ScalarKind::Instant => ind::ScalarKind::Instant,
    }
}

/// Whether the production value agrees, structurally and value-wise, with what the
/// independent decoder produced. The independent tree carries no `ty` handles (compared
/// structurally), and keeps temporal scalars as their raw canonical bytes (compared against
/// the production scalar codec's bytes), so agreement is value agreement, not handle equality.
fn agrees(domain: &ValueDomain, decoded: &ind::DecodedValue) -> bool {
    match (domain, decoded) {
        (ValueDomain::Scalar(scalar), _) => scalar_agrees(scalar, decoded),
        (ValueDomain::Product { fields, .. }, ind::DecodedValue::Product(items)) => {
            fields.len() == items.len()
                && fields.iter().zip(items).all(|(slot, item)| {
                    // A durable struct is dense: every leaf is present.
                    slot.as_ref().is_some_and(|value| agrees(value, item))
                })
        }
        (
            ValueDomain::Sum {
                variant, payload, ..
            },
            ind::DecodedValue::Sum {
                variant: dv,
                payload: dp,
            },
        ) => {
            u32::from(*variant) == *dv
                && payload.len() == dp.len()
                && payload.iter().zip(dp).all(|(a, b)| agrees(a, b))
        }
        _ => false,
    }
}

fn scalar_agrees(scalar: &RuntimeScalar, decoded: &ind::DecodedValue) -> bool {
    match (scalar, decoded) {
        (RuntimeScalar::Int(v), ind::DecodedValue::Int(w)) => v == w,
        (RuntimeScalar::Bool(v), ind::DecodedValue::Bool(w)) => v == w,
        (RuntimeScalar::Str(v), ind::DecodedValue::Text(w)) => v == w,
        (RuntimeScalar::Bytes(v), ind::DecodedValue::Bytes(w)) => v == w,
        // The A11 decoder parses temporal scalars into STRUCTURED fields (year/month/day,
        // etc.), validating their canonical form independently. Reconstruct the canonical
        // text from those fields and compare byte-for-byte to the production scalar codec's
        // canonical bytes for the same value: if the decoder misparsed any field the
        // reconstruction diverges, so this is a genuine value differential over the parse.
        (RuntimeScalar::Date(_), ind::DecodedValue::Date { .. })
        | (RuntimeScalar::Instant(_), ind::DecodedValue::Instant { .. })
        | (RuntimeScalar::Duration(_), ind::DecodedValue::Duration { .. }) => {
            reconstruct_temporal(decoded).as_bytes()
                == encode_value(scalar).unwrap_or_default().as_slice()
        }
        _ => false,
    }
}

/// The canonical sub-second fraction (matches `marrow_temporal::push_nanos_fraction`): a
/// `.` then the nine-digit zero-padded nanoseconds with trailing zeros trimmed, or empty
/// when zero.
fn frac(nanos: u32) -> String {
    if nanos == 0 {
        String::new()
    } else {
        format!(".{}", format!("{nanos:09}").trim_end_matches('0'))
    }
}

/// Reconstruct the canonical scalar text an independent decoder's STRUCTURED temporal value
/// implies, so it can be compared byte-for-byte to the production canonical encoding.
fn reconstruct_temporal(decoded: &ind::DecodedValue) -> String {
    match decoded {
        ind::DecodedValue::Date { year, month, day } => format!("{year:04}-{month:02}-{day:02}"),
        ind::DecodedValue::Instant {
            year,
            month,
            day,
            hour,
            min,
            sec,
            nanos,
        } => format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}{}Z",
            frac(*nanos),
        ),
        ind::DecodedValue::Duration {
            negative,
            seconds,
            nanos,
        } => format!(
            "{}PT{seconds}{}S",
            if *negative { "-" } else { "" },
            frac(*nanos),
        ),
        _ => String::from("\0not-a-temporal"),
    }
}

// --- corpus builders ---

fn si(v: i64) -> ValueDomain {
    ValueDomain::Scalar(RuntimeScalar::Int(v))
}
fn ss(s: &str) -> ValueDomain {
    ValueDomain::Scalar(RuntimeScalar::Str(s.into()))
}
fn opt_shape(inner: ValueShape) -> ValueShape {
    ValueShape::Sum {
        ty: 0,
        variants: vec![vec![], vec![inner]],
    }
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
fn sc(kind: ScalarKind) -> ValueShape {
    ValueShape::Scalar(kind)
}

/// A representative corpus of storable values paired with their shapes: every scalar kind
/// (incl. temporals and NUL-laden strings/bytes), products, user-enum-style sums, `Option`
/// none/some, nested `Option`, and a nested mix.
fn corpus() -> Vec<(ValueDomain, ValueShape)> {
    vec![
        (si(0), sc(ScalarKind::Int)),
        (si(-42), sc(ScalarKind::Int)),
        (si(i64::MIN), sc(ScalarKind::Int)),
        (
            ValueDomain::Scalar(RuntimeScalar::Bool(true)),
            sc(ScalarKind::Bool),
        ),
        (ss("hi\u{0}there"), sc(ScalarKind::Str)),
        (ss(""), sc(ScalarKind::Str)),
        (
            ValueDomain::Scalar(RuntimeScalar::Bytes(vec![0x00, 0xff, 0x00])),
            sc(ScalarKind::Bytes),
        ),
        (
            ValueDomain::Scalar(RuntimeScalar::Date(0)),
            sc(ScalarKind::Date),
        ),
        // A fractional instant (sub-second nanos exercise the canonical fraction).
        (
            ValueDomain::Scalar(RuntimeScalar::Instant(1_234)),
            sc(ScalarKind::Instant),
        ),
        // The min and max canonically-encodable instants (year-range endpoints).
        (
            ValueDomain::Scalar(RuntimeScalar::Instant(
                marrow_temporal::SUPPORTED_INSTANT_MIN_NANOS,
            )),
            sc(ScalarKind::Instant),
        ),
        (
            ValueDomain::Scalar(RuntimeScalar::Instant(
                marrow_temporal::SUPPORTED_INSTANT_MAX_NANOS,
            )),
            sc(ScalarKind::Instant),
        ),
        // Durations: negative whole-second, negative sub-second (-PT0.5S), positive
        // fractional (PT1.5S), and a large but decoder-valid magnitude (whole seconds
        // below u64::MAX, nanos within i128) to pin that large legitimate values round-trip.
        (
            ValueDomain::Scalar(RuntimeScalar::Duration(-9)),
            sc(ScalarKind::Duration),
        ),
        (
            ValueDomain::Scalar(RuntimeScalar::Duration(-500_000_000)),
            sc(ScalarKind::Duration),
        ),
        (
            ValueDomain::Scalar(RuntimeScalar::Duration(1_500_000_000)),
            sc(ScalarKind::Duration),
        ),
        (
            // 9e18 whole seconds (< u64::MAX ≈ 1.8e19), well within i128 nanoseconds.
            ValueDomain::Scalar(RuntimeScalar::Duration(
                9_000_000_000_000_000_000_000_000_000,
            )),
            sc(ScalarKind::Duration),
        ),
        // A product of mixed leaves.
        (
            ValueDomain::Product {
                ty: 3,
                fields: vec![Some(si(7)), Some(ss("a\u{0}b"))],
            },
            ValueShape::Product {
                ty: 3,
                fields: vec![sc(ScalarKind::Int), sc(ScalarKind::Str)],
            },
        ),
        // A user-enum-style sum: variant 2 with two payload leaves.
        (
            ValueDomain::Sum {
                ty: 5,
                variant: 2,
                payload: vec![si(1), ss("x")],
            },
            ValueShape::Sum {
                ty: 5,
                variants: vec![
                    vec![],
                    vec![sc(ScalarKind::Int)],
                    vec![sc(ScalarKind::Int), sc(ScalarKind::Str)],
                ],
            },
        ),
        // Option none / some / nested.
        (none(), opt_shape(sc(ScalarKind::Int))),
        (some(si(3)), opt_shape(sc(ScalarKind::Int))),
        (some(none()), opt_shape(opt_shape(sc(ScalarKind::Int)))),
        (some(some(si(7))), opt_shape(opt_shape(sc(ScalarKind::Int)))),
        // A nested mix: product whose second leaf is an Option[str].
        (
            ValueDomain::Product {
                ty: 3,
                fields: vec![Some(si(1)), Some(some(ss("z")))],
            },
            ValueShape::Product {
                ty: 3,
                fields: vec![sc(ScalarKind::Int), opt_shape(sc(ScalarKind::Str))],
            },
        ),
    ]
}

/// The differential: encode each corpus value with the production codec, decode with the
/// independent module, and assert value agreement and a clean whole-cell consume.
#[test]
fn production_encode_agrees_with_the_independent_decoder() {
    for (value, shape) in corpus() {
        let bytes = encode_domain(&value).expect("production encodes the storable value");
        let ind_shape = to_ind_shape(&shape);
        match ind::decode(&bytes, &ind_shape) {
            Ok(decoded) => assert!(
                agrees(&value, &decoded),
                "independent decode disagrees for {value:?}: got {decoded:?} from bytes {bytes:?}",
            ),
            Err(error) => panic!(
                "independent decoder rejected production bytes {bytes:?} for {value:?}: {error:?}",
            ),
        }
    }
}

/// The independent decoder must also *reject* what the production decoder rejects: a
/// forged non-canonical framing (a non-minimal LEB128 length inside a product) is refused,
/// not normalized — the reject-not-normalize law, checked from the independent side.
#[test]
fn the_independent_decoder_rejects_a_non_canonical_forgery() {
    let shape = ind::Shape::Product(vec![
        ind::Shape::Scalar(ind::ScalarKind::Int),
        ind::Shape::Scalar(ind::ScalarKind::Int),
    ]);
    // A non-minimal LEB128 length prefix (0x80 0x00) on the first leaf.
    assert!(ind::decode(&[0x80, 0x00, 0x01, b'6'], &shape).is_err());
}

/// Over-max instant: production refuses to encode an instant outside the year-0001..9999
/// range at encode time (no bytes reach the store), so the codec never emits a value the
/// decoder would have to handle. The production-side bound, asserted directly.
#[test]
fn production_refuses_an_out_of_range_instant() {
    let over = ValueDomain::Scalar(RuntimeScalar::Instant(
        marrow_temporal::SUPPORTED_INSTANT_MAX_NANOS + 1,
    ));
    assert!(
        encode_domain(&over).is_err(),
        "an instant past the supported year range must be refused at encode",
    );
}

/// PINNED DIVERGENCE (brief §2 A12 finding, for slice F). Production `duration` is backed by
/// `i128` nanoseconds, so its canonical whole-seconds field ranges to ~1.7e29 (30 digits);
/// the independent decoder parses whole-seconds into `u64` and rejects anything past
/// `u64::MAX ≈ 1.8e19`. This is a decoder-oracle limitation from the brief's prior silence on
/// magnitude, not a production defect: production encodes and round-trips `i128::MAX`, and the
/// decoder rejects those bytes. The test pins both sides so the divergence is conspicuous and
/// regression-caught; when the decoder widens its seconds accumulator to `u128`/`i128` this
/// flips to agreement and moves into the corpus above.
#[test]
fn extreme_duration_is_producible_but_beyond_the_decoder_u64() {
    let extreme = ValueDomain::Scalar(RuntimeScalar::Duration(i128::MAX));
    let bytes = encode_domain(&extreme).expect("production encodes an i128::MAX duration");
    // Production itself round-trips it (the value is legitimate).
    assert_eq!(
        marrow_kernel::codec::value::decode_domain(
            &bytes,
            &ValueShape::Scalar(ScalarKind::Duration)
        ),
        Some(extreme),
    );
    // The u64-seconds independent decoder rejects the ~30-digit whole-seconds field.
    assert!(
        ind::decode(&bytes, &ind::Shape::Scalar(ind::ScalarKind::Duration)).is_err(),
        "the decoder's u64 seconds accumulator must reject (not silently truncate) the extreme",
    );
}

/// PINNED DIVERGENCE (brief §A7 A12 depth-convention finding, for slice F). Production counts
/// `MAX_DURABLE_VALUE_DEPTH = 32` over *composite* levels only — a scalar leaf is free, so a
/// chain of exactly 32 nested products over one scalar is admitted. The independent decoder
/// counts every shape node (the scalar leaf included), so it admits only 31 composites and
/// rejects the 32-deep value. Ambiguity #3 from the decoder report; the brief now fixes the
/// convention. Pinned so the off-by-one is conspicuous until the decoder drops the scalar-leaf
/// level in slice F.
#[test]
fn depth_32_is_producible_but_the_decoder_stops_at_31() {
    // 32 nested products over one int scalar.
    let mut value = si(5);
    let mut shape = sc(ScalarKind::Int);
    let mut ind_shape = ind::Shape::Scalar(ind::ScalarKind::Int);
    for _ in 0..32 {
        value = ValueDomain::Product {
            ty: 0,
            fields: vec![Some(value)],
        };
        shape = ValueShape::Product {
            ty: 0,
            fields: vec![shape],
        };
        ind_shape = ind::Shape::Product(vec![ind_shape]);
    }
    let bytes = encode_domain(&value).expect("production encodes a depth-32 composite");
    assert_eq!(
        marrow_kernel::codec::value::decode_domain(&bytes, &shape),
        Some(value),
        "production admits and round-trips 32 composite levels",
    );
    assert!(
        ind::decode(&bytes, &ind_shape).is_err(),
        "the decoder counts the scalar leaf as a level and rejects the 32nd composite",
    );
}
