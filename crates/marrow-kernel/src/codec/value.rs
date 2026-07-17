//! Canonical saved-value encoding.
//!
//! Values are stored in a backend-independent canonical byte form, so backup,
//! diff, equality, and restore are stable. The bytes carry no type tag — the
//! type comes from the schema at read time — and are not order-preserving, since
//! the store orders by key rather than by value.

use marrow_codes::Code;
use marrow_temporal::{
    format_date, format_duration, format_instant, parse_date, parse_duration, parse_instant,
    supported_date_days, supported_instant_nanos,
};

use super::key::KeyScalar;

/// Version of the canonical value encoding, recorded in a store profile so a
/// reopen can refuse data it cannot decode. Advances only on an incompatible
/// byte-format change.
pub const VALUE_CODEC_VERSION: u32 = 0;

/// A decoded scalar value, the runtime representation shared by the VM, kernel,
/// and tooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeScalar {
    Bool(bool),
    Int(i64),
    Str(String),
    Bytes(Vec<u8>),
    /// A calendar date, held as days since the Unix epoch (1970-01-01).
    Date(i32),
    /// An elapsed span, held as a signed count of nanoseconds.
    Duration(i128),
    /// A UTC instant, held as a signed count of nanoseconds since the epoch.
    Instant(i128),
}

impl RuntimeScalar {
    /// This scalar's order-preserving key projection. The single home for that
    /// mapping; every current scalar type is key-eligible.
    pub fn as_key(&self) -> Result<Option<KeyScalar>, ValueError> {
        let key = match self {
            RuntimeScalar::Int(v) => KeyScalar::Int(*v),
            RuntimeScalar::Bool(v) => KeyScalar::Bool(*v),
            RuntimeScalar::Str(v) => KeyScalar::Str(v.clone()),
            RuntimeScalar::Bytes(v) => KeyScalar::Bytes(v.clone()),
            RuntimeScalar::Date(v) => KeyScalar::Date(*v),
            RuntimeScalar::Duration(v) => KeyScalar::Duration(*v),
            RuntimeScalar::Instant(v) => KeyScalar::Instant(*v),
        };
        validate_scalar_key(&key)?;
        Ok(Some(key))
    }

    /// This scalar's type discriminant.
    pub fn ty(&self) -> ScalarKind {
        match self {
            RuntimeScalar::Bool(_) => ScalarKind::Bool,
            RuntimeScalar::Int(_) => ScalarKind::Int,
            RuntimeScalar::Str(_) => ScalarKind::Str,
            RuntimeScalar::Bytes(_) => ScalarKind::Bytes,
            RuntimeScalar::Date(_) => ScalarKind::Date,
            RuntimeScalar::Duration(_) => ScalarKind::Duration,
            RuntimeScalar::Instant(_) => ScalarKind::Instant,
        }
    }
}

/// A value that cannot be encoded to canonical saved form. A `date`/`instant`
/// outside year 0001-9999 would format to a 5-7 digit year that [`decode_value`]
/// could never read back, so the codec rejects it to keep the round-trip exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    DateOutOfRange {
        days: i32,
    },
    InstantOutOfRange {
        nanos: i128,
    },
    /// A composite value's encoding exceeds a Law-9 size cap (a scalar leaf past
    /// [`MAX_LEAF_BYTES`] or the whole value past [`MAX_DURABLE_VALUE_BYTES`]), refused at
    /// encode before any engine write. Maps to the kernel's `value.range` fault.
    ValueTooLarge,
    /// A composite value carries a shape the storable durable value set excludes (a
    /// collection, an ordered map, unit, or an absent product leaf in a dense struct).
    /// Storable inline values are scalars, dense products, and sums only.
    Unstorable,
}

impl ValueError {
    /// The stable dotted code a tool reports for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::DateOutOfRange { .. }
            | Self::InstantOutOfRange { .. }
            | Self::ValueTooLarge
            | Self::Unstorable => Code::ValueRange.as_str(),
        }
    }
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DateOutOfRange { days } => {
                write!(f, "date day {days} is outside the year 0001-9999 range")
            }
            Self::InstantOutOfRange { nanos } => {
                write!(f, "instant {nanos}ns is outside the year 0001-9999 range")
            }
            Self::ValueTooLarge => write!(f, "a durable value exceeds its encoded size cap"),
            Self::Unstorable => write!(f, "a value shape is not storable inline in a field"),
        }
    }
}

impl std::error::Error for ValueError {}

/// The type to decode saved bytes as. A typed read knows this from the verified
/// site. Distinct from the compiler's language-level scalar classification: this
/// is the runtime codec's discriminant over the full saved-value domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Bool,
    Int,
    Str,
    Bytes,
    Date,
    Duration,
    Instant,
}

impl ScalarKind {
    /// The canonical language spelling of this scalar type.
    pub fn name(self) -> &'static str {
        match self {
            ScalarKind::Bool => "bool",
            ScalarKind::Int => "int",
            ScalarKind::Str => "string",
            ScalarKind::Bytes => "bytes",
            ScalarKind::Date => "date",
            ScalarKind::Instant => "instant",
            ScalarKind::Duration => "duration",
        }
    }
}

/// Encodes a value to its canonical saved bytes: `bool` as `0`/`1`, `int` as decimal
/// text, strings as UTF-8, bytes verbatim, dates as `YYYY-MM-DD`, durations as
/// `PT<seconds>S`, instants as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// The canonical boundary: it emits only forms [`decode_value`] reads back, so a
/// `date`/`instant` outside year 0001-9999 is a typed [`ValueError`].
pub fn encode_value(value: &RuntimeScalar) -> Result<Vec<u8>, ValueError> {
    // A saved cell holds exactly one present scalar: the only cell discriminant is
    // the scalar type tag, never a null, optional, or tombstone value. Absence is
    // the lack of a cell at the data path, not an encoded marker. The closed
    // `RuntimeScalar` sum is the structural guarantee.
    Ok(match value {
        RuntimeScalar::Bool(value) => vec![if *value { b'1' } else { b'0' }],
        RuntimeScalar::Int(value) => value.to_string().into_bytes(),
        RuntimeScalar::Str(text) => text.as_bytes().to_vec(),
        RuntimeScalar::Bytes(bytes) => bytes.clone(),
        RuntimeScalar::Date(days) => format_date(*days)
            .ok_or(ValueError::DateOutOfRange { days: *days })?
            .into_bytes(),
        RuntimeScalar::Duration(nanos) => format_duration(*nanos).into_bytes(),
        RuntimeScalar::Instant(nanos) => format_instant(*nanos)
            .ok_or(ValueError::InstantOutOfRange { nanos: *nanos })?
            .into_bytes(),
    })
}

/// Decodes canonical saved bytes as `ty`, strictly: non-canonical bytes such as
/// `+1`, `01`, or a non-`0`/`1` boolean are rejected rather than normalized.
pub fn decode_value(bytes: &[u8], ty: ScalarKind) -> Option<RuntimeScalar> {
    match ty {
        ScalarKind::Bool => match bytes {
            b"0" => Some(RuntimeScalar::Bool(false)),
            b"1" => Some(RuntimeScalar::Bool(true)),
            _ => None,
        },
        ScalarKind::Int => Some(RuntimeScalar::Int(parse_canonical_int(bytes)?)),
        ScalarKind::Str => Some(RuntimeScalar::Str(String::from_utf8(bytes.to_vec()).ok()?)),
        ScalarKind::Bytes => Some(RuntimeScalar::Bytes(bytes.to_vec())),
        ScalarKind::Date => Some(RuntimeScalar::Date(parse_date(bytes)?)),
        ScalarKind::Duration => Some(RuntimeScalar::Duration(parse_duration(bytes)?)),
        ScalarKind::Instant => Some(RuntimeScalar::Instant(parse_instant(bytes)?)),
    }
}

/// Parses the canonical int form, rejecting anything that would not round-trip
/// identically (`+1`, `01`, `-0`, whitespace).
fn parse_canonical_int(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: i64 = text.parse().ok()?;
    (value.to_string() == text).then_some(value)
}

pub fn validate_scalar_key(key: &KeyScalar) -> Result<(), ValueError> {
    match key {
        KeyScalar::Date(days) if !supported_date_days(*days) => {
            Err(ValueError::DateOutOfRange { days: *days })
        }
        KeyScalar::Instant(nanos) if !supported_instant_nanos(*nanos) => {
            Err(ValueError::InstantOutOfRange { nanos: *nanos })
        }
        _ => Ok(()),
    }
}

pub fn scalar_key_matches_type(key: &KeyScalar, expected: ScalarKind) -> bool {
    key.scalar_kind() == expected && validate_scalar_key(key).is_ok()
}

// --- Widened composite value codec ---
//
// A durable field value widens from a bare scalar to the closed acyclic storable set:
// scalars, dense products (`struct`/record), and sums (closed `enum`/`Option`/`Result`).
// Collections are never inline field payloads (they are keyed branches); a nominal-typed
// field is not yet admitted. The codec extends the one scalar codec above — a scalar leaf is
// still `encode_value`/`decode_value` — and frames composites within one field-leaf cell:
// a top-level scalar is raw (byte-identical to today); inside a composite each scalar leaf
// is minimal-LEB128 length-prefixed, a sum carries a minimal-LEB128 variant index, and a
// nested composite is schema-delimited. Bytes carry no type tag — the shape comes from the
// schema at read time (`ValueShape`) — so decode is shape-driven and strict: a non-minimal
// length, an out-of-range variant, an over-cap leaf, an over-deep shape, or trailing bytes
// are rejected, never normalized. One value, one encoding.

use super::varint::{decode_len, encode_len};
use crate::equality::ValueDomain;

/// The per-scalar-leaf encoded byte cap (mirrors the VM `run.text_limit`, 64 KiB).
pub const MAX_LEAF_BYTES: usize = 64 * 1024;
/// The whole-value encoded byte cap. Chosen (not inherited); it must stay `<=` the engine
/// `MAX_VALUE_LEN` so a value this codec admits always fits the engine and the codec's own
/// Law-9 fault fires first (see the slice-E design brief §4).
pub const MAX_DURABLE_VALUE_BYTES: usize = 1 << 20;
/// The value-shape nesting depth cap (mirrors `marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH`),
/// bounding decoder recursion before allocation.
pub const MAX_DURABLE_VALUE_DEPTH: usize = 32;

/// The schema-derived shape of a durable field value, driving the tagless decode. A scalar
/// carries its kind; a product carries its type index and per-leaf shapes in declaration
/// order; a sum carries its type index and, per variant in declaration order, that variant's
/// dense payload shapes. `Option`/`Result` are ordinary sums (fixed indices `none=0/some=1`,
/// `ok=0/err=1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueShape {
    Scalar(ScalarKind),
    Product {
        ty: u16,
        fields: Vec<ValueShape>,
    },
    Sum {
        ty: u16,
        variants: Vec<Vec<ValueShape>>,
    },
}

/// Encode a storable value to its canonical cell bytes. A top-level scalar is the raw scalar
/// codec (byte-identical to `encode_value`); a composite is framed. Refuses a value outside
/// the storable set ([`ValueError::Unstorable`]) or past a size cap
/// ([`ValueError::ValueTooLarge`]) before returning any bytes.
pub fn encode_domain(value: &ValueDomain) -> Result<Vec<u8>, ValueError> {
    let bytes = match value {
        ValueDomain::Scalar(scalar) => encode_value(scalar)?,
        ValueDomain::Product { .. } | ValueDomain::Sum { .. } => {
            let mut out = Vec::new();
            write_composite(value, &mut out)?;
            out
        }
        // An entry identity is not a storable cell value on this slice — the durable
        // codec of a stored identity is a separately reserved decision. Rejecting it here
        // is the encoder half of the no-identity-at-the-store-boundary contract.
        ValueDomain::Unit
        | ValueDomain::List { .. }
        | ValueDomain::Map { .. }
        | ValueDomain::Identity { .. } => {
            return Err(ValueError::Unstorable);
        }
    };
    if bytes.len() > MAX_DURABLE_VALUE_BYTES {
        return Err(ValueError::ValueTooLarge);
    }
    Ok(bytes)
}

/// Write a composite value's leaves. A product writes each field leaf in order; a sum writes
/// its variant index then that variant's dense payload leaves.
fn write_composite(value: &ValueDomain, out: &mut Vec<u8>) -> Result<(), ValueError> {
    match value {
        ValueDomain::Product { fields, .. } => {
            for field in fields {
                // A dense durable struct has every leaf present; an absent slot is not a
                // storable inline value (optionality within a struct is an `Option` sum).
                let field = field.as_ref().ok_or(ValueError::Unstorable)?;
                write_member(field, out)?;
            }
            Ok(())
        }
        ValueDomain::Sum {
            variant, payload, ..
        } => {
            encode_len(u64::from(*variant), out);
            for leaf in payload {
                write_member(leaf, out)?;
            }
            Ok(())
        }
        _ => Err(ValueError::Unstorable),
    }
}

/// Write one member (leaf) of a composite: a scalar as a minimal-LEB128 length prefix then
/// its raw scalar bytes (capped per leaf); a nested composite schema-delimited (no prefix).
fn write_member(value: &ValueDomain, out: &mut Vec<u8>) -> Result<(), ValueError> {
    match value {
        ValueDomain::Scalar(scalar) => {
            let bytes = encode_value(scalar)?;
            if bytes.len() > MAX_LEAF_BYTES {
                return Err(ValueError::ValueTooLarge);
            }
            encode_len(bytes.len() as u64, out);
            out.extend_from_slice(&bytes);
            Ok(())
        }
        ValueDomain::Product { .. } | ValueDomain::Sum { .. } => write_composite(value, out),
        _ => Err(ValueError::Unstorable),
    }
}

/// Decode canonical cell bytes as the value of `shape`, strictly. A top-level scalar reads
/// the whole cell; a composite is shape-driven and must consume the whole cell with no
/// trailing bytes. Returns `None` on any malformed or non-canonical input.
pub fn decode_domain(bytes: &[u8], shape: &ValueShape) -> Option<ValueDomain> {
    match shape {
        ValueShape::Scalar(kind) => decode_value(bytes, *kind).map(ValueDomain::Scalar),
        ValueShape::Product { .. } | ValueShape::Sum { .. } => {
            let (value, used) = read_composite(bytes, shape, 1)?;
            (used == bytes.len()).then_some(value)
        }
    }
}

/// Read a composite value of `shape` from the front of `bytes`, returning it and the bytes
/// consumed. `depth` bounds nesting before allocation (Law 9).
fn read_composite(bytes: &[u8], shape: &ValueShape, depth: usize) -> Option<(ValueDomain, usize)> {
    if depth > MAX_DURABLE_VALUE_DEPTH {
        return None;
    }
    match shape {
        ValueShape::Product { ty, fields } => {
            let mut used = 0;
            let mut slots = Vec::with_capacity(fields.len());
            for field in fields {
                let (value, n) = read_member(&bytes[used..], field, depth)?;
                slots.push(Some(value));
                used += n;
            }
            Some((
                ValueDomain::Product {
                    ty: *ty,
                    fields: slots,
                },
                used,
            ))
        }
        ValueShape::Sum { ty, variants } => {
            let (index, mut used) = decode_len(bytes)?;
            let variant = usize::try_from(index).ok()?;
            let payload_shapes = variants.get(variant)?;
            let mut payload = Vec::with_capacity(payload_shapes.len());
            for leaf in payload_shapes {
                let (value, n) = read_member(&bytes[used..], leaf, depth)?;
                payload.push(value);
                used += n;
            }
            Some((
                ValueDomain::Sum {
                    ty: *ty,
                    variant: variant as u16,
                    payload,
                },
                used,
            ))
        }
        ValueShape::Scalar(_) => None,
    }
}

/// Read one member (leaf) of a composite of `shape` from the front of `bytes`: a scalar
/// reads its minimal-LEB128 length (capped) then that many raw scalar bytes; a nested
/// composite recurses one deeper.
fn read_member(bytes: &[u8], shape: &ValueShape, depth: usize) -> Option<(ValueDomain, usize)> {
    match shape {
        ValueShape::Scalar(kind) => {
            let (len, prefix) = decode_len(bytes)?;
            let len = usize::try_from(len).ok()?;
            if len > MAX_LEAF_BYTES {
                return None;
            }
            let leaf = bytes.get(prefix..prefix + len)?;
            let scalar = decode_value(leaf, *kind)?;
            Some((ValueDomain::Scalar(scalar), prefix + len))
        }
        ValueShape::Product { .. } | ValueShape::Sum { .. } => {
            read_composite(bytes, shape, depth + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeScalar, decode_value, encode_value};

    /// Every present scalar encodes to bytes that decode back under its own scalar
    /// type tag — the only cell discriminant. There is no null, optional, or
    /// tombstone cell value: absence is the lack of a cell, so the encode boundary
    /// only ever sees a present scalar.
    #[test]
    fn the_only_cell_discriminant_is_the_scalar_type_tag() {
        let values = [
            RuntimeScalar::Bool(true),
            RuntimeScalar::Int(-7),
            RuntimeScalar::Str("hello".into()),
            RuntimeScalar::Bytes(vec![0x00, 0xff]),
            RuntimeScalar::Date(0),
            RuntimeScalar::Duration(1_500_000_000),
            RuntimeScalar::Instant(0),
        ];
        for value in values {
            let bytes = encode_value(&value).expect("a present scalar encodes");
            assert_eq!(decode_value(&bytes, value.ty()), Some(value));
        }
    }
}

#[cfg(test)]
mod composite_codec {
    use super::{
        MAX_DURABLE_VALUE_DEPTH, MAX_LEAF_BYTES, RuntimeScalar, ScalarKind, ValueShape,
        decode_domain, decode_value, encode_domain, encode_value,
    };
    use crate::equality::{ValueDomain, value_equality};

    fn scalar(kind: ScalarKind) -> ValueShape {
        ValueShape::Scalar(kind)
    }
    fn di(v: i64) -> ValueDomain {
        ValueDomain::Scalar(RuntimeScalar::Int(v))
    }
    fn ds(s: &str) -> ValueDomain {
        ValueDomain::Scalar(RuntimeScalar::Str(s.into()))
    }
    /// An `Option`-shaped sum: variant 0 = none (empty payload), variant 1 = some(inner).
    fn opt_shape(inner: ValueShape) -> ValueShape {
        ValueShape::Sum {
            ty: 9,
            variants: vec![vec![], vec![inner]],
        }
    }
    fn none() -> ValueDomain {
        ValueDomain::Sum {
            ty: 9,
            variant: 0,
            payload: vec![],
        }
    }
    fn some(inner: ValueDomain) -> ValueDomain {
        ValueDomain::Sum {
            ty: 9,
            variant: 1,
            payload: vec![inner],
        }
    }

    /// A1/byte-identity KAT: a top-level scalar value encodes byte-for-byte as the existing
    /// scalar codec — the oracle-differential-preserving property. No length prefix, no tag.
    #[test]
    fn a_top_level_scalar_is_byte_identical_to_the_scalar_codec() {
        for value in [
            RuntimeScalar::Int(-42),
            RuntimeScalar::Str("hi\u{0}there".into()),
            RuntimeScalar::Bool(true),
            RuntimeScalar::Bytes(vec![0x00, 0xff]),
        ] {
            let raw = encode_value(&value).expect("scalar encodes");
            let domain =
                encode_domain(&ValueDomain::Scalar(value.clone())).expect("domain encodes");
            assert_eq!(
                domain, raw,
                "a top-level scalar carries no composite framing"
            );
            assert_eq!(
                decode_domain(&domain, &scalar(value.ty())),
                Some(ValueDomain::Scalar(value)),
            );
        }
    }

    /// A product (two int leaves) frames each leaf with a minimal length prefix, in schema
    /// order, and round-trips.
    #[test]
    fn a_product_frames_leaves_in_order_and_round_trips() {
        let shape = ValueShape::Product {
            ty: 3,
            fields: vec![scalar(ScalarKind::Int), scalar(ScalarKind::Str)],
        };
        let value = ValueDomain::Product {
            ty: 3,
            fields: vec![Some(di(5)), Some(ds("ab"))],
        };
        let bytes = encode_domain(&value).expect("product encodes");
        // len("5")=1, "5", len("ab")=2, "ab".
        assert_eq!(bytes, vec![0x01, b'5', 0x02, b'a', b'b']);
        assert_eq!(decode_domain(&bytes, &shape), Some(value));
    }

    /// A3: nested `Option` is an ordinary sum; `none`, `some(none)`, `some(some(v))` are three
    /// distinct values with three distinct encodings, each round-tripping.
    #[test]
    fn nested_option_is_three_distinct_values() {
        let shape = opt_shape(opt_shape(scalar(ScalarKind::Int)));
        let none_v = none();
        let some_none = some(none());
        let some_some = some(some(di(7)));

        let bs: Vec<_> = [&none_v, &some_none, &some_some]
            .iter()
            .map(|v| encode_domain(v).expect("encodes"))
            .collect();
        // Distinct encodings.
        assert_ne!(bs[0], bs[1]);
        assert_ne!(bs[1], bs[2]);
        assert_ne!(bs[0], bs[2]);
        // Canonical fingerprints: none = variant 0; some(none) = 1 then inner variant 0;
        // some(some(7)) = 1, 1, len("7")=1, "7".
        assert_eq!(bs[0], vec![0x00]);
        assert_eq!(bs[1], vec![0x01, 0x00]);
        assert_eq!(bs[2], vec![0x01, 0x01, 0x01, b'7']);
        for (v, b) in [
            (&none_v, &bs[0]),
            (&some_none, &bs[1]),
            (&some_some, &bs[2]),
        ] {
            assert_eq!(decode_domain(b, &shape).as_ref(), Some(v));
        }
    }

    /// A8: byte equality is value equality — for every pair in a corpus,
    /// `encode(a) == encode(b)` iff `value_equality(a, b)`, tested against the equality owner.
    #[test]
    fn byte_equality_conforms_to_value_domain_equality() {
        let corpus = [
            di(0),
            di(1),
            ds("a"),
            ds("a\u{0}"),
            none(),
            some(di(0)),
            some(di(1)),
            some(none()),
            ValueDomain::Product {
                ty: 3,
                fields: vec![Some(di(1)), Some(ds("x"))],
            },
            ValueDomain::Product {
                ty: 3,
                fields: vec![Some(di(1)), Some(ds("y"))],
            },
        ];
        for a in &corpus {
            for b in &corpus {
                let (ea, eb) = (encode_domain(a), encode_domain(b));
                if let (Ok(ea), Ok(eb)) = (ea, eb) {
                    assert_eq!(
                        ea == eb,
                        value_equality(a, b),
                        "byte-equality must match value equality for {a:?} vs {b:?}",
                    );
                }
            }
        }
    }

    /// Forged bytes are rejected, never normalized.
    #[test]
    fn forged_bytes_are_rejected() {
        let prod = ValueShape::Product {
            ty: 3,
            fields: vec![scalar(ScalarKind::Int), scalar(ScalarKind::Int)],
        };
        // Truncation: a leaf length says 2 but only 1 byte follows.
        assert_eq!(decode_domain(&[0x02, b'5'], &prod), None);
        // Trailing bytes after a complete value.
        assert_eq!(decode_domain(&[0x01, b'5', 0x01, b'6', 0xff], &prod), None);
        // Non-minimal length prefix (0x80 0x00 = non-minimal zero).
        assert_eq!(decode_domain(&[0x80, 0x00, 0x01, b'6'], &prod), None);
        // A non-canonical scalar leaf ("01" is not a canonical int).
        assert_eq!(decode_domain(&[0x02, b'0', b'1', 0x01, b'6'], &prod), None);

        // Out-of-range sum variant index (only 0/1 declared).
        let opt = opt_shape(scalar(ScalarKind::Int));
        assert_eq!(decode_domain(&[0x02], &opt), None);
    }

    /// Over-cap and over-depth are Law-9 refusals at encode and decode.
    #[test]
    fn over_cap_and_over_depth_are_refused() {
        // An over-`MAX_LEAF_BYTES` scalar leaf inside a product is refused at encode.
        let big = ValueDomain::Product {
            ty: 3,
            fields: vec![Some(ds(&"x".repeat(MAX_LEAF_BYTES + 1)))],
        };
        assert!(encode_domain(&big).is_err());

        // A decode shape nested past MAX_DURABLE_VALUE_DEPTH is refused before allocation.
        let mut shape = ValueShape::Scalar(ScalarKind::Int);
        for _ in 0..=MAX_DURABLE_VALUE_DEPTH + 1 {
            shape = ValueShape::Product {
                ty: 3,
                fields: vec![shape],
            };
        }
        // A minimal byte string cannot be over-deep-valid, but the decoder must refuse the
        // over-deep shape rather than recurse unbounded; feed it a byte and expect None.
        assert_eq!(decode_domain(&[0x00], &shape), None);
    }

    /// The full round-trip law over a mixed corpus: `decode(encode(v), shape) == v`.
    #[test]
    fn encode_decode_round_trips_the_storable_set() {
        let cases = [
            (di(-1), scalar(ScalarKind::Int)),
            (ds("hi"), scalar(ScalarKind::Str)),
            (some(di(3)), opt_shape(scalar(ScalarKind::Int))),
            (none(), opt_shape(scalar(ScalarKind::Int))),
            (
                ValueDomain::Product {
                    ty: 3,
                    fields: vec![Some(di(1)), Some(some(ds("z")))],
                },
                ValueShape::Product {
                    ty: 3,
                    fields: vec![scalar(ScalarKind::Int), opt_shape(scalar(ScalarKind::Str))],
                },
            ),
        ];
        for (value, shape) in cases {
            let bytes = encode_domain(&value).expect("encodes");
            assert_eq!(decode_domain(&bytes, &shape), Some(value));
        }
    }

    /// A collection, map, or unit value is not storable inline.
    #[test]
    fn non_storable_shapes_are_refused_at_encode() {
        assert!(encode_domain(&ValueDomain::Unit).is_err());
        assert!(
            encode_domain(&ValueDomain::List {
                idx: 0,
                items: vec![]
            })
            .is_err()
        );
        assert!(
            encode_domain(&ValueDomain::Map {
                idx: 0,
                entries: vec![]
            })
            .is_err()
        );
        // An entry identity is not a durable value: the encoder refuses it, the
        // store-boundary half of the no-identity-at-the-encoder contract.
        assert!(
            encode_domain(&ValueDomain::Identity {
                root: crate::equality::RootId(0),
                keys: vec![crate::codec::key::KeyScalar::Int(1)],
            })
            .is_err()
        );
        // Also proves the scalar codec is unchanged for a bare scalar leaf.
        assert_eq!(
            decode_value(b"5", ScalarKind::Int),
            Some(RuntimeScalar::Int(5))
        );
    }
}
