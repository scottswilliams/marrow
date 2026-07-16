//! Independent-decoder differential for the widened durable field-value codec (E03w slice E,
//! finalized in slice F). The module under `support/` is an independent strict decoder written
//! from the design brief alone — it never reads the production encoder. This test vendors that
//! module with only rustfmt whitespace normalization applied (the workspace fmt gate is
//! mandatory); its decode logic and decisions are byte-for-byte the author's, unaltered and
//! verified by its own embedded KATs. The harness wires only glue: it encodes a corpus of
//! storable values with the production `encode_domain`, decodes the bytes with the independent
//! module, and asserts value agreement. A disagreement is a finding to report, never a
//! fix-in-place; the vendored decode logic is not edited here.
//!
//! Including the module here also compiles and runs its own embedded KATs (its
//! `#[cfg(test)] mod tests`), so the brief's KAT byte strings — including the §A7 18-byte
//! descriptor KAT — are checked from the independent side in this binary.
//!
//! Slice-F reconciliation: the decoder is now the A12-final version (i128-scale durations,
//! depth = 32 composites leaves-free, big-endian descriptor tags, identity-bearing
//! type-index). The two prior pinned encoder/decoder divergences (duration `u64` seconds,
//! depth off-by-one) are resolved and are now agreement coverage below.

// The vendored oracle keeps the author's own form (dead_code is allowed inside the file).
// Its idioms are suppressed at this boundary rather than rewritten, so the independent
// decoder is never edited toward this workspace's lints.
#[path = "support/independent_decoder.rs"]
#[allow(clippy::doc_overindented_list_items, clippy::manual_range_contains)]
mod independent_decoder;

use independent_decoder as ind;
use marrow_kernel::codec::value::{
    RuntimeScalar, ScalarKind, ValueShape, decode_domain, encode_domain, encode_value,
};
use marrow_kernel::equality::ValueDomain;

/// Convert the production value shape into the independent decoder's schema shape. The
/// independent `Shape` now carries the same nominal `type-index` (§A7 A12) as the production
/// shape, so the identity handle crosses and is compared on both sides.
fn to_ind_shape(shape: &ValueShape) -> ind::Shape {
    match shape {
        ValueShape::Scalar(kind) => ind::Shape::Scalar(to_ind_kind(*kind)),
        ValueShape::Product { ty, fields } => ind::Shape::Product {
            ty: *ty,
            leaves: fields.iter().map(to_ind_shape).collect(),
        },
        ValueShape::Sum { ty, variants } => ind::Shape::Sum {
            ty: *ty,
            variants: variants
                .iter()
                .map(|payload| payload.iter().map(to_ind_shape).collect())
                .collect(),
        },
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
/// independent decoder produced. Composite nodes now carry the nominal `type-index` on both
/// sides, so the identity handle is compared too (a differential on the identity-bearing
/// field). Temporal scalars are kept by the decoder as their structured canonical fields and
/// reconstructed here, compared against the production scalar codec's canonical bytes.
fn agrees(domain: &ValueDomain, decoded: &ind::DecodedValue) -> bool {
    match (domain, decoded) {
        (ValueDomain::Scalar(scalar), _) => scalar_agrees(scalar, decoded),
        (
            ValueDomain::Product { ty, fields },
            ind::DecodedValue::Product {
                ty: dty,
                fields: items,
            },
        ) => {
            ty == dty
                && fields.len() == items.len()
                && fields.iter().zip(items).all(|(slot, item)| {
                    // A durable struct is dense: every leaf is present.
                    slot.as_ref().is_some_and(|value| agrees(value, item))
                })
        }
        (
            ValueDomain::Sum {
                ty,
                variant,
                payload,
            },
            ind::DecodedValue::Sum {
                ty: dty,
                variant: dv,
                payload: dp,
            },
        ) => {
            ty == dty
                && u32::from(*variant) == *dv
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
/// implies, so it can be compared byte-for-byte to the production canonical encoding. The
/// duration whole-seconds field is `u128` (A12), printed directly.
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
fn dur(nanos: i128) -> ValueDomain {
    ValueDomain::Scalar(RuntimeScalar::Duration(nanos))
}

/// A representative corpus of storable values paired with their shapes: every scalar kind
/// (incl. temporals and NUL-laden strings/bytes), products, user-enum-style sums, `Option`
/// none/some, nested `Option`, and a nested mix. The i128-magnitude duration endpoints
/// (`i128::MAX`/`i128::MIN`) are agreement cases now that the reconciled decoder accumulates
/// whole-seconds into `u128` (A12) — the former pinned `u64` divergence is resolved.
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
        // fractional (PT1.5S), a large mid-range magnitude, and the i128 endpoints — the
        // whole i128-nanosecond range is canonically encodable (A12), and the reconciled
        // decoder round-trips every one of these.
        (dur(-9), sc(ScalarKind::Duration)),
        (dur(-500_000_000), sc(ScalarKind::Duration)),
        (dur(1_500_000_000), sc(ScalarKind::Duration)),
        // 9e18 whole seconds (> u64::MAX ≈ 1.8e19 nanos here in value; still within i128).
        (
            dur(9_000_000_000_000_000_000_000_000_000),
            sc(ScalarKind::Duration),
        ),
        // The i128 endpoints: whole-seconds magnitude ~1.7e29 (needs u128 in the decoder).
        (dur(i128::MAX), sc(ScalarKind::Duration)),
        (dur(i128::MIN), sc(ScalarKind::Duration)),
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
    let shape = ind::Shape::Product {
        ty: 0,
        leaves: vec![
            ind::Shape::Scalar(ind::ScalarKind::Int),
            ind::Shape::Scalar(ind::ScalarKind::Int),
        ],
    };
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

/// RESOLVED DIVERGENCE (brief §2 A12 duration finding). Production `duration` is backed by
/// `i128` nanoseconds, so its canonical whole-seconds field ranges to ~1.7e29 (30 digits).
/// The reconciled independent decoder accumulates whole-seconds into `u128` and now agrees:
/// it decodes `i128::MAX`/`i128::MIN` durations to the same value production round-trips.
/// This is the former slice-E pinned `u64` divergence, now agreement (also covered by the
/// corpus endpoints; kept as an explicit named witness of the resolution).
#[test]
fn extreme_duration_round_trips_through_the_independent_decoder() {
    for value in [dur(i128::MAX), dur(i128::MIN)] {
        let bytes = encode_domain(&value).expect("production encodes an i128-endpoint duration");
        // Production itself round-trips it.
        assert_eq!(
            decode_domain(&bytes, &ValueShape::Scalar(ScalarKind::Duration)),
            Some(value.clone()),
        );
        // The reconciled u128-seconds independent decoder now agrees (no longer rejects).
        let decoded = ind::decode(&bytes, &ind::Shape::Scalar(ind::ScalarKind::Duration))
            .expect("the reconciled decoder accepts the i128-scale duration");
        assert!(
            agrees(&value, &decoded),
            "independent decode disagrees for {value:?}: got {decoded:?} from bytes {bytes:?}",
        );
    }
}

/// RESOLVED DIVERGENCE (brief §A7 A12 depth-convention finding). Production counts
/// `MAX_DURABLE_VALUE_DEPTH = 32` over *composite* levels only — a scalar leaf is free, so a
/// chain of exactly 32 nested products over one scalar is admitted, and the 33rd composite is
/// refused before descent. The reconciled independent decoder now uses the same convention:
/// it accepts 32 composites and rejects 33. Both the accept (agreement) and the reject
/// (both tiers refuse) are pinned.
#[test]
fn depth_32_accepted_and_33_rejected_by_both_tiers() {
    // 32 nested products over one int scalar: production round-trips, independent agrees.
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
        ind_shape = ind::Shape::Product {
            ty: 0,
            leaves: vec![ind_shape],
        };
    }
    let bytes = encode_domain(&value).expect("production encodes a depth-32 composite");
    assert_eq!(
        decode_domain(&bytes, &shape),
        Some(value.clone()),
        "production admits and round-trips 32 composite levels",
    );
    let decoded = ind::decode(&bytes, &ind_shape)
        .expect("the reconciled decoder admits 32 composite levels (scalar leaf free)");
    assert!(
        agrees(&value, &decoded),
        "independent decode disagrees at depth 32: got {decoded:?}",
    );

    // 33 nested products: production decode refuses (depth cap) and the independent decoder
    // refuses the shape before any value bytes are read.
    let value33 = ValueDomain::Product {
        ty: 0,
        fields: vec![Some(value)],
    };
    let shape33 = ValueShape::Product {
        ty: 0,
        fields: vec![shape],
    };
    let ind_shape33 = ind::Shape::Product {
        ty: 0,
        leaves: vec![ind_shape],
    };
    let bytes33 = encode_domain(&value33).expect("production encodes the depth-33 bytes");
    assert_eq!(
        decode_domain(&bytes33, &shape33),
        None,
        "production refuses to decode a 33-deep composite",
    );
    assert!(
        ind::decode(&bytes33, &ind_shape33).is_err(),
        "the independent decoder refuses the 33-deep shape before descent",
    );
}

/// Descriptor cross-check (brief §A7 18-byte KAT). The reconciled independent decoder's
/// `parse_descriptor` matches §A7; parsing the frozen 18-byte descriptor for
/// `{ int, Option[string] }` yields a `Shape` (carrying the nominal type indices 3 and 4)
/// that decodes the production-encoded value bytes of the matching value in agreement. This
/// ties production value bytes to an independently descriptor-derived shape, closing the
/// descriptor↔value seam from the independent side. (The independent module's own
/// `descriptor_bytes_kat` and production's `a7_descriptor_bytes_kat` independently pin the
/// same 18 bytes.)
#[test]
fn descriptor_derived_shape_decodes_production_value_bytes() {
    // Brief §A7 A12 printed KAT: Product{ ty:3, [ Scalar(int), Sum{ ty:4, [ [], [str] ] } ] }.
    let descriptor = [
        0x01, 0x00, 0x03, 0x00, 0x02, // product ty=3 leaf-count=2
        0x00, 0x02, //                   leaf0: scalar int
        0x02, 0x00, 0x04, 0x00, 0x02, // leaf1: sum ty=4 variant-count=2
        0x00, 0x00, //                     variant0: payload-count 0
        0x00, 0x01, 0x00, 0x03, //         variant1: payload-count 1, scalar string
    ];
    let shape = ind::parse_descriptor(&descriptor).expect("the §A7 descriptor parses");

    // The matching value: { int = 1, Option[string] = some("z") }, with the same type
    // indices the descriptor carries so the identity-bearing comparison holds.
    let value = ValueDomain::Product {
        ty: 3,
        fields: vec![
            Some(si(1)),
            Some(ValueDomain::Sum {
                ty: 4,
                variant: 1,
                payload: vec![ss("z")],
            }),
        ],
    };
    let bytes = encode_domain(&value).expect("production encodes the record value");
    let decoded =
        ind::decode(&bytes, &shape).expect("the descriptor-derived shape decodes the value bytes");
    assert!(
        agrees(&value, &decoded),
        "descriptor-derived decode disagrees: got {decoded:?} from bytes {bytes:?}",
    );
}
