//! Independent strict decoder for Marrow's widened durable field-value codec.
//!
//! Implemented STRICTLY from the design brief
//! `marrow-orchestration/e03w-codec-brief.md` (§2 encoding rules, §4 Law-9
//! bounds, §A7 profile descriptor). It reads NO encoder source. Where the brief
//! is silent, the choice taken is documented inline as `AMBIGUITY:` and in
//! REPORT.md, always taking the brief's most literal reading.
//!
//! This module is self-contained: no external crates, no marrow imports. It
//! uses `std` only for `String`/`Vec` (the same shapes would hold over `alloc`).
//!
//! Decode model (brief §2):
//!   * Outer cell scalar : raw canonical scalar bytes, NO length prefix; the
//!                         cell boundary delimits it.
//!   * Nested scalar leaf: minimal-LEB128 byte-length prefix, then raw scalar
//!                         bytes (scalars are variable-length text and cannot
//!                         self-delimit inside a composite).
//!   * Product (struct)  : leaves in schema declaration order, recursively; no
//!                         count prefix, no presence bits (dense).
//!   * Sum (enum/Option/ : minimal-LEB128 variant index, then that variant's
//!         Result)         dense payload leaves recursively. Composite leaves
//!                         (product/sum) are NOT length-prefixed; they
//!                         self-delimit by walking the schema.
//!
//! Everything here is reject-not-normalize: any non-canonical byte sequence is
//! refused with a typed error.

#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Law-9 bounds (brief §4). Values inherited verbatim from the brief table.
// ---------------------------------------------------------------------------

/// value-shape nesting depth cap (`MAX_DURABLE_VALUE_DEPTH`, brief §4).
pub const MAX_DURABLE_VALUE_DEPTH: usize = 32;
/// struct leaf count cap (`MAX_STRUCT_LEAVES`, brief §4). The dense-composite leaf
/// count stays narrow and independent of the record field width.
pub const MAX_STRUCT_LEAVES: usize = 64;
/// enum variant count cap (`MAX_VARIANTS`, brief §4).
pub const MAX_VARIANTS: usize = 256;
/// enum per-variant payload leaf cap (`MAX_PAYLOAD_FIELDS`, brief §4).
pub const MAX_PAYLOAD_FIELDS: usize = 64;
/// per-scalar-leaf byte cap (`MAX_TEXT_BYTES` = 64 KiB, brief §4).
pub const MAX_TEXT_BYTES: u64 = 64 * 1024;
/// whole encoded field value cap (`MAX_DURABLE_VALUE_BYTES`, brief §2 law 3 /
/// §4: FIXED at 1 MiB = engine `MAX_VALUE_LEN` = `1<<20`, amendment A10).
pub const MAX_DURABLE_VALUE_BYTES: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Schema shape (the decoder's read-time input; brief §2 "type/shape comes from
// the schema at read time", descriptor spec §A7).
// ---------------------------------------------------------------------------

/// The frozen scalar value set (brief §1 table: `int bool string bytes date
/// instant duration`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    Bool,
    Text,
    Bytes,
    Date,
    Instant,
    Duration,
}

/// A recursive value shape. Product carries its ordered leaf shapes; Sum
/// carries, per variant index, that variant's ordered payload leaf shapes.
/// Both composites carry `ty`, the schema's nominal-type identity handle
/// (`type-index`, brief §A7 A12): it does NOT drive byte parsing — the structure
/// alone determines how value bytes are read — but it is identity-bearing
/// (`value_equality` compares it), so it is retained and copied into the decoded
/// value for a descriptor/identity cross-check.
///
/// `Option[T]` is `Sum { ty, variants: [ [], [T] ] }` (variant 0 = `none` empty
/// payload, variant 1 = `some` single leaf). `Result` is the analogous
/// `Sum { ty, variants: [ [ok_t], [err_t] ] }`. There is no separate Option arm
/// (brief §2 A2).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Scalar(ScalarKind),
    Product { ty: u16, leaves: Vec<Shape> },
    Sum { ty: u16, variants: Vec<Vec<Shape>> },
}

/// Convenience constructor for `Option[inner]` with nominal type index `ty`.
pub fn option_shape(ty: u16, inner: Shape) -> Shape {
    Shape::Sum {
        ty,
        variants: vec![vec![], vec![inner]],
    }
}

// ---------------------------------------------------------------------------
// Decoded value tree (owned).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum DecodedValue {
    Int(i64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    // Canonical byte forms fixed by brief §2 law 3 (amendments A10/A11): the
    // full internal form of every temporal is strictly validated on decode.
    Date {
        year: u16,
        month: u8,
        day: u8,
    }, // YYYY-MM-DD (year 0001-9999)
    // YYYY-MM-DDTHH:MM:SS[.fraction]Z ; nanos is the sub-second part (0 when the
    // canonical fraction is omitted).
    Instant {
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        min: u8,
        sec: u8,
        nanos: u32,
    },
    // [-]PT<seconds>[.fraction]S ; the value is i128 nanoseconds, so the
    // whole-seconds magnitude spans up to ~1.7e29 and needs u128 (brief §2 A12).
    // `negative` marks the sign; it is only set for a non-zero magnitude.
    Duration {
        negative: bool,
        seconds: u128,
        nanos: u32,
    },
    // `ty` is the nominal-type identity carried from the schema (§A7 A12),
    // retained so structurally identical composites of different types compare
    // unequal.
    Product {
        ty: u16,
        fields: Vec<DecodedValue>,
    },
    Sum {
        ty: u16,
        variant: u32,
        payload: Vec<DecodedValue>,
    },
}

// ---------------------------------------------------------------------------
// Errors (typed, reject-not-normalize).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// A LEB128 group had a redundant high `0x00` terminator (brief §2 Law 2).
    NonMinimalVarint,
    /// LEB128 ran off the end of the buffer (continuation bit with no successor).
    VarintTruncated,
    /// LEB128 magnitude exceeds `u64`.
    VarintOverflow,
    /// A scalar/leaf read ran past the end of the buffer.
    Truncated,
    /// Bytes remained after the whole value was decoded (brief §2 Law 5).
    TrailingBytes,
    /// Sum variant index was `>=` the schema's declared variant count (brief §6).
    VariantOutOfRange { index: u64, variant_count: usize },
    /// A scalar leaf length exceeded `MAX_TEXT_BYTES` (checked before allocation).
    LeafLengthOverCap { len: u64 },
    /// The whole encoded value exceeded `MAX_DURABLE_VALUE_BYTES`.
    ValueBytesOverCap { len: usize },
    /// Value-shape nesting exceeded `MAX_DURABLE_VALUE_DEPTH` at decode.
    DepthExceeded,
    /// A `Text` (or `Int`) leaf was not valid UTF-8.
    InvalidUtf8,
    /// An `Int` scalar was not the strict canonical decimal form.
    NonCanonicalInt,
    /// A `Bool` scalar was not the canonical single ASCII byte `'0'`/`'1'`.
    NonCanonicalBool,
    /// A `date`/`instant`/`duration` scalar was not its canonical form (§2 A10).
    NonCanonicalTemporal(&'static str),
    /// The supplied schema shape itself violated a Law-9 bound.
    ShapeInvalid(&'static str),
    /// Descriptor byte stream (§A7) carried an unknown discriminant / kind tag.
    BadDescriptor(&'static str),
}

// ---------------------------------------------------------------------------
// Cursor + minimal LEB128 (brief §2 A9).
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn take_rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }

    /// Read an unsigned minimal LEB128 (brief §2 A9): unsigned base-128, low
    /// group first, high bit = continuation. The shortest byte sequence for the
    /// magnitude is the sole canonical form; a redundant high `0x00` group is
    /// rejected, never normalized.
    fn read_varint(&mut self) -> Result<u64, DecodeError> {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        let mut count: usize = 0;
        loop {
            if self.pos >= self.buf.len() {
                return Err(DecodeError::VarintTruncated);
            }
            let byte = self.buf[self.pos];
            self.pos += 1;
            count += 1;

            if shift >= 64 {
                return Err(DecodeError::VarintOverflow);
            }
            let low7 = (byte & 0x7f) as u64;
            // At shift 63 only bit 63 (value 1 in the group) fits in u64.
            if shift == 63 && low7 > 1 {
                return Err(DecodeError::VarintOverflow);
            }
            result |= low7 << shift;

            if byte & 0x80 == 0 {
                // Terminating byte. Minimality: a multi-byte encoding whose
                // highest (last) group is zero is redundant.
                if count > 1 && byte == 0x00 {
                    return Err(DecodeError::NonMinimalVarint);
                }
                return Ok(result);
            }
            shift += 7;
        }
    }

    fn read_u16_be(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
}

// ---------------------------------------------------------------------------
// Shape validation (Law-9 caps on the schema, brief §4 verifier tier).
// ---------------------------------------------------------------------------

/// Reject a schema shape that itself violates a structural Law-9 bound before
/// any bytes are consumed.
///
/// Depth convention (brief §A7 A12): the outermost composite value is level 1;
/// each nested composite adds one level; a scalar leaf does not count as a level
/// (it is read in place) and a top-level scalar value is uncounted. The cap
/// `MAX_DURABLE_VALUE_DEPTH = 32` therefore admits 32 nested composite levels —
/// the deepest admitted shape is 32 composites over one scalar leaf; the 33rd
/// composite is refused. The `depth` argument is the level of the composite
/// currently being checked; scalars carry no level.
pub fn validate_shape(shape: &Shape) -> Result<(), DecodeError> {
    fn go(shape: &Shape, depth: usize) -> Result<(), DecodeError> {
        match shape {
            // Scalars are free: a top-level or leaf scalar occupies no level.
            Shape::Scalar(_) => Ok(()),
            Shape::Product { leaves, .. } => {
                if depth > MAX_DURABLE_VALUE_DEPTH {
                    return Err(DecodeError::ShapeInvalid(
                        "depth exceeds MAX_DURABLE_VALUE_DEPTH",
                    ));
                }
                if leaves.len() > MAX_STRUCT_LEAVES {
                    return Err(DecodeError::ShapeInvalid(
                        "leaf count exceeds MAX_STRUCT_LEAVES",
                    ));
                }
                for l in leaves {
                    go(l, depth + 1)?;
                }
                Ok(())
            }
            Shape::Sum { variants, .. } => {
                if depth > MAX_DURABLE_VALUE_DEPTH {
                    return Err(DecodeError::ShapeInvalid(
                        "depth exceeds MAX_DURABLE_VALUE_DEPTH",
                    ));
                }
                if variants.len() > MAX_VARIANTS {
                    return Err(DecodeError::ShapeInvalid(
                        "variant count exceeds MAX_VARIANTS",
                    ));
                }
                for v in variants {
                    if v.len() > MAX_PAYLOAD_FIELDS {
                        return Err(DecodeError::ShapeInvalid(
                            "payload count exceeds MAX_PAYLOAD_FIELDS",
                        ));
                    }
                    for l in v {
                        go(l, depth + 1)?;
                    }
                }
                Ok(())
            }
        }
    }
    go(shape, 1)
}

// ---------------------------------------------------------------------------
// Decode entry point.
// ---------------------------------------------------------------------------

/// Strictly decode `bytes` as a single durable field-value cell against
/// `shape`. Rejects any non-canonical framing per the brief §2 canonical-form
/// laws; never normalizes.
pub fn decode(bytes: &[u8], shape: &Shape) -> Result<DecodedValue, DecodeError> {
    validate_shape(shape)?;
    if bytes.len() > MAX_DURABLE_VALUE_BYTES {
        return Err(DecodeError::ValueBytesOverCap { len: bytes.len() });
    }
    let mut cur = Cursor::new(bytes);
    // Outer cell: a scalar consumes the WHOLE remaining cell with no length
    // prefix (brief §2 outer-scalar rule) and is uncounted for depth; a
    // composite is the outermost value at level 1.
    let value = match shape {
        Shape::Scalar(kind) => {
            let raw = cur.take_rest();
            if raw.len() as u64 > MAX_TEXT_BYTES {
                return Err(DecodeError::LeafLengthOverCap {
                    len: raw.len() as u64,
                });
            }
            interpret_scalar(*kind, raw)?
        }
        _ => decode_node(&mut cur, shape, 1)?,
    };
    // Law 5: the decoder must consume the whole cell.
    if cur.pos != bytes.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(value)
}

/// A node reached inside a composite (or the outermost composite). A scalar here
/// is length-prefixed and free of depth; a product/sum self-delimits by schema
/// walk and occupies level `depth`.
fn decode_node(cur: &mut Cursor, shape: &Shape, depth: usize) -> Result<DecodedValue, DecodeError> {
    match shape {
        Shape::Scalar(kind) => {
            // Scalar leaves do not count toward depth (brief §A7 A12).
            let len = cur.read_varint()?;
            // Law-9: bound the length BEFORE allocating / slicing.
            if len > MAX_TEXT_BYTES {
                return Err(DecodeError::LeafLengthOverCap { len });
            }
            let raw = cur.take(len as usize)?;
            interpret_scalar(*kind, raw)
        }
        Shape::Product { ty, leaves } => {
            if depth > MAX_DURABLE_VALUE_DEPTH {
                return Err(DecodeError::DepthExceeded);
            }
            let mut fields = Vec::with_capacity(leaves.len());
            for leaf in leaves {
                fields.push(decode_node(cur, leaf, depth + 1)?);
            }
            Ok(DecodedValue::Product { ty: *ty, fields })
        }
        Shape::Sum { ty, variants } => {
            if depth > MAX_DURABLE_VALUE_DEPTH {
                return Err(DecodeError::DepthExceeded);
            }
            let idx = cur.read_varint()?;
            if idx as usize >= variants.len() {
                return Err(DecodeError::VariantOutOfRange {
                    index: idx,
                    variant_count: variants.len(),
                });
            }
            let payload_shapes = &variants[idx as usize];
            let mut payload = Vec::with_capacity(payload_shapes.len());
            for leaf in payload_shapes {
                payload.push(decode_node(cur, leaf, depth + 1)?);
            }
            Ok(DecodedValue::Sum {
                ty: *ty,
                variant: idx as u32,
                payload,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Scalar interpretation (strict canonical forms).
// ---------------------------------------------------------------------------

fn interpret_scalar(kind: ScalarKind, raw: &[u8]) -> Result<DecodedValue, DecodeError> {
    match kind {
        ScalarKind::Int => parse_canonical_int(raw).map(DecodedValue::Int),
        ScalarKind::Text => match core_str(raw) {
            Ok(s) => Ok(DecodedValue::Text(s.to_string())),
            Err(_) => Err(DecodeError::InvalidUtf8),
        },
        ScalarKind::Bytes => Ok(DecodedValue::Bytes(raw.to_vec())),
        ScalarKind::Bool => parse_canonical_bool(raw),
        ScalarKind::Date => parse_canonical_date(raw),
        ScalarKind::Instant => parse_canonical_instant(raw),
        ScalarKind::Duration => parse_canonical_duration(raw),
    }
}

fn core_str(raw: &[u8]) -> Result<&str, ()> {
    std::str::from_utf8(raw).map_err(|_| ())
}

/// Strict canonical decimal `int` (brief §2: `int` = decimal text; KATs
/// `int(5) = [0x35]`, `int(7) = [0x37]`).
///
/// AMBIGUITY: the brief calls the scalar forms "the existing strict canonical
/// scalar forms (value.rs:158)" without defining them. Literal canonical
/// decimal taken here: optional single leading `-`, then digits, with no
/// leading zero (except the single digit `0`), no `-0`, no `+`, non-empty.
fn parse_canonical_int(raw: &[u8]) -> Result<i64, DecodeError> {
    let s = core_str(raw).map_err(|_| DecodeError::InvalidUtf8)?;
    if s.is_empty() {
        return Err(DecodeError::NonCanonicalInt);
    }
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DecodeError::NonCanonicalInt);
    }
    if digits.len() > 1 && digits.as_bytes()[0] == b'0' {
        return Err(DecodeError::NonCanonicalInt); // leading zero
    }
    if neg && digits == "0" {
        return Err(DecodeError::NonCanonicalInt); // negative zero
    }
    s.parse::<i64>().map_err(|_| DecodeError::NonCanonicalInt)
}

/// `bool` = one ASCII byte, `'0'` (0x30) = false / `'1'` (0x31) = true
/// (brief §2 law 3, amendment A10).
fn parse_canonical_bool(raw: &[u8]) -> Result<DecodedValue, DecodeError> {
    match raw {
        [0x30] => Ok(DecodedValue::Bool(false)),
        [0x31] => Ok(DecodedValue::Bool(true)),
        _ => Err(DecodeError::NonCanonicalBool),
    }
}

/// Parse exactly `n` ASCII digits at `bytes` into a `u32`. Rejects any
/// non-digit or a wrong length. Zero-padding is required (fixed width), so no
/// leading-zero rule applies to fixed-width temporal fields.
fn fixed_digits(bytes: &[u8], n: usize) -> Result<u32, DecodeError> {
    if bytes.len() != n {
        return Err(DecodeError::NonCanonicalTemporal("field width"));
    }
    let mut v: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return Err(DecodeError::NonCanonicalTemporal("non-digit field"));
        }
        v = v * 10 + (b - b'0') as u32;
    }
    Ok(v)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap =
                (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
            if leap { 29 } else { 28 }
        }
        _ => 0,
    }
}

/// Validate a `YYYY-MM-DD` date, returning `(year, month, day)`. Years are
/// 0001-9999; `0000` is invalid (brief §2 law 3, A11).
fn parse_date_fields(raw: &[u8]) -> Result<(u16, u8, u8), DecodeError> {
    // Exactly 10 bytes: YYYY-MM-DD
    if raw.len() != 10 || raw[4] != b'-' || raw[7] != b'-' {
        return Err(DecodeError::NonCanonicalTemporal("date layout"));
    }
    let year = fixed_digits(&raw[0..4], 4)? as u16;
    let month = fixed_digits(&raw[5..7], 2)? as u8;
    let day = fixed_digits(&raw[8..10], 2)? as u8;
    if year < 1 {
        return Err(DecodeError::NonCanonicalTemporal("year range"));
    }
    if month < 1 || month > 12 {
        return Err(DecodeError::NonCanonicalTemporal("month range"));
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err(DecodeError::NonCanonicalTemporal("day range"));
    }
    Ok((year, month, day))
}

/// Canonical sub-second fraction shared by `instant` and `duration` (brief §2
/// law 3, A11). `digits` are the characters AFTER the `.`: 1-9 ASCII digits, the
/// last non-zero (trailing zeros trimmed), interpreted as nine-digit zero-padded
/// nanoseconds. Returns the nanosecond value.
fn parse_canonical_fraction(digits: &[u8]) -> Result<u32, DecodeError> {
    if digits.is_empty() || digits.len() > 9 {
        return Err(DecodeError::NonCanonicalTemporal("fraction width"));
    }
    if !digits.iter().all(|b| b.is_ascii_digit()) {
        return Err(DecodeError::NonCanonicalTemporal("fraction non-digit"));
    }
    if *digits.last().unwrap() == b'0' {
        return Err(DecodeError::NonCanonicalTemporal("fraction trailing zero"));
    }
    let mut v: u32 = 0;
    for &b in digits {
        v = v * 10 + (b - b'0') as u32;
    }
    // Right-pad to nine digits: ".5" => 500_000_000 ns.
    let nanos = v * 10u32.pow(9 - digits.len() as u32);
    Ok(nanos)
}

/// Parse an optional fraction *section*: either empty (fraction omitted, nanos
/// 0) or a `.`-led canonical fraction. A bare `.` or trailing zeros are rejected
/// by `parse_canonical_fraction`; an omitted-but-zero fraction is the only
/// canonical spelling of zero nanoseconds.
fn parse_fraction_section(sec: &[u8]) -> Result<u32, DecodeError> {
    if sec.is_empty() {
        return Ok(0);
    }
    if sec[0] != b'.' {
        return Err(DecodeError::NonCanonicalTemporal("fraction marker"));
    }
    parse_canonical_fraction(&sec[1..])
}

/// `date` = ASCII `YYYY-MM-DD` (brief §2 law 3, amendment A10).
fn parse_canonical_date(raw: &[u8]) -> Result<DecodedValue, DecodeError> {
    let (year, month, day) = parse_date_fields(raw)?;
    Ok(DecodedValue::Date { year, month, day })
}

/// `instant` = ASCII `YYYY-MM-DDTHH:MM:SS[.fraction]Z` (brief §2 law 3, A11).
fn parse_canonical_instant(raw: &[u8]) -> Result<DecodedValue, DecodeError> {
    // Minimum 20 bytes (no fraction): YYYY-MM-DDTHH:MM:SSZ. Must end with 'Z';
    // the fixed head is 19 bytes; anything between it and the trailing 'Z' is
    // the optional fraction section.
    if raw.len() < 20 || *raw.last().unwrap() != b'Z' {
        return Err(DecodeError::NonCanonicalTemporal("instant layout"));
    }
    if raw[10] != b'T' || raw[13] != b':' || raw[16] != b':' {
        return Err(DecodeError::NonCanonicalTemporal("instant layout"));
    }
    let (year, month, day) = parse_date_fields(&raw[0..10])?;
    let hour = fixed_digits(&raw[11..13], 2)? as u8;
    let min = fixed_digits(&raw[14..16], 2)? as u8;
    let sec = fixed_digits(&raw[17..19], 2)? as u8;
    if hour > 23 {
        return Err(DecodeError::NonCanonicalTemporal("hour range"));
    }
    if min > 59 {
        return Err(DecodeError::NonCanonicalTemporal("minute range"));
    }
    // No leap second (A11: parse rejects seconds > 59).
    if sec > 59 {
        return Err(DecodeError::NonCanonicalTemporal("second range"));
    }
    // The fraction section sits between index 19 and the trailing 'Z'.
    let nanos = parse_fraction_section(&raw[19..raw.len() - 1])?;
    Ok(DecodedValue::Instant {
        year,
        month,
        day,
        hour,
        min,
        sec,
        nanos,
    })
}

/// `duration` = ASCII `[-]PT<seconds>[.fraction]S` (brief §2 law 3, A11). The
/// sign is a canonical PREFIX before `PT` (never inside). `<seconds>` is decimal
/// digits with no leading zero except the single digit `0`. Zero is exactly
/// `PT0S`; `-PT0S` is rejected (no canonical negative zero) — but a negative
/// non-zero magnitude such as `-PT0.5S` is valid.
fn parse_canonical_duration(raw: &[u8]) -> Result<DecodedValue, DecodeError> {
    // Optional leading '-' sign, stripped before the 'PT' body.
    let (negative, body) = match raw.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, raw),
    };
    // Body: PT<seconds>[.fraction]S, minimum "PT0S" = 4 bytes.
    if body.len() < 4 || body[0] != b'P' || body[1] != b'T' || *body.last().unwrap() != b'S' {
        return Err(DecodeError::NonCanonicalTemporal("duration layout"));
    }
    let inner = &body[2..body.len() - 1]; // <seconds>[.fraction]
    // Split off the optional fraction at the first '.'.
    let (sec_digits, frac_section): (&[u8], &[u8]) = match inner.iter().position(|&b| b == b'.') {
        Some(dot) => (&inner[..dot], &inner[dot..]),
        None => (inner, &[]),
    };
    if sec_digits.is_empty() || !sec_digits.iter().all(|b| b.is_ascii_digit()) {
        return Err(DecodeError::NonCanonicalTemporal("duration count digits"));
    }
    if sec_digits.len() > 1 && sec_digits[0] == b'0' {
        return Err(DecodeError::NonCanonicalTemporal("duration leading zero"));
    }
    // The value is i128 nanoseconds; the whole-seconds magnitude spans up to
    // ~1.7e29 (floor(i128::MAX / 1e9)), so accumulate into u128, not u64
    // (brief §2 A12). checked_* mirrors production's `parse_duration`.
    let mut seconds: u128 = 0;
    for &b in sec_digits {
        seconds = seconds
            .checked_mul(10)
            .and_then(|v| v.checked_add((b - b'0') as u128))
            .ok_or(DecodeError::NonCanonicalTemporal("duration overflow"))?;
    }
    let nanos = parse_fraction_section(frac_section)?;
    // No canonical negative zero.
    if negative && seconds == 0 && nanos == 0 {
        return Err(DecodeError::NonCanonicalTemporal("duration negative zero"));
    }
    // The value is i128 nanoseconds; a text whose magnitude exceeds the i128
    // range is the canonical form of no value (production converts u128->i128
    // with a checked cast, special-casing i128::MIN = 2^127). Reject it.
    const NANOS_PER_SEC: u128 = 1_000_000_000;
    let magnitude = seconds
        .checked_mul(NANOS_PER_SEC)
        .and_then(|v| v.checked_add(nanos as u128))
        .ok_or(DecodeError::NonCanonicalTemporal("duration overflow"))?;
    let limit = if negative {
        1u128 << 127 // |i128::MIN|
    } else {
        i128::MAX as u128
    };
    if magnitude > limit {
        return Err(DecodeError::NonCanonicalTemporal(
            "duration magnitude range",
        ));
    }
    Ok(DecodedValue::Duration {
        negative,
        seconds,
        nanos,
    })
}

// ---------------------------------------------------------------------------
// Profile descriptor parsing (brief §A7, exact facts from A12).
//   shape := 0x00 <scalar-kind-tag>
//          | 0x01 <type-index u16> <leaf-count u16> shape*
//          | 0x02 <type-index u16> <variant-count u16> ( <payload-count u16> shape* )*
//
// Facts (A12, verified first-hand against profile.rs):
//   * scalar-kind tag bytes (disjoint from the shape discriminants 0x00/0x01/0x02):
//     bool=0x01, int=0x02, string=0x03, bytes=0x04, date=0x05, duration=0x06,
//     instant=0x07.
//   * every u16 (type-index, leaf-count, variant-count, payload-count) is
//     big-endian.
//   * type-index is the node's nominal-type identity handle: it does not drive
//     parsing but IS identity-bearing (value_equality compares it), so it is
//     retained into Shape::Product/Sum { ty } rather than discarded.
// ---------------------------------------------------------------------------

fn scalar_kind_from_tag(tag: u8) -> Result<ScalarKind, DecodeError> {
    Ok(match tag {
        0x01 => ScalarKind::Bool,
        0x02 => ScalarKind::Int,
        0x03 => ScalarKind::Text, // "string"
        0x04 => ScalarKind::Bytes,
        0x05 => ScalarKind::Date,
        0x06 => ScalarKind::Duration,
        0x07 => ScalarKind::Instant,
        _ => return Err(DecodeError::BadDescriptor("unknown scalar-kind-tag")),
    })
}

fn scalar_kind_to_tag(kind: ScalarKind) -> u8 {
    match kind {
        ScalarKind::Bool => 0x01,
        ScalarKind::Int => 0x02,
        ScalarKind::Text => 0x03,
        ScalarKind::Bytes => 0x04,
        ScalarKind::Date => 0x05,
        ScalarKind::Duration => 0x06,
        ScalarKind::Instant => 0x07,
    }
}

/// Parse a §A7 value-shape descriptor into a `Shape`. Rejects trailing bytes.
pub fn parse_descriptor(bytes: &[u8]) -> Result<Shape, DecodeError> {
    let mut cur = Cursor::new(bytes);
    let shape = parse_descriptor_node(&mut cur)?;
    if cur.pos != bytes.len() {
        return Err(DecodeError::BadDescriptor("trailing descriptor bytes"));
    }
    Ok(shape)
}

fn parse_descriptor_node(cur: &mut Cursor) -> Result<Shape, DecodeError> {
    let disc = cur.take(1)?[0];
    match disc {
        0x00 => {
            let tag = cur.take(1)?[0];
            Ok(Shape::Scalar(scalar_kind_from_tag(tag)?))
        }
        0x01 => {
            let ty = cur.read_u16_be()?;
            let leaf_count = cur.read_u16_be()? as usize;
            let mut leaves = Vec::with_capacity(leaf_count);
            for _ in 0..leaf_count {
                leaves.push(parse_descriptor_node(cur)?);
            }
            Ok(Shape::Product { ty, leaves })
        }
        0x02 => {
            let ty = cur.read_u16_be()?;
            let variant_count = cur.read_u16_be()? as usize;
            let mut variants = Vec::with_capacity(variant_count);
            for _ in 0..variant_count {
                let payload_count = cur.read_u16_be()? as usize;
                let mut payload = Vec::with_capacity(payload_count);
                for _ in 0..payload_count {
                    payload.push(parse_descriptor_node(cur)?);
                }
                variants.push(payload);
            }
            Ok(Shape::Sum { ty, variants })
        }
        _ => Err(DecodeError::BadDescriptor("unknown shape discriminant")),
    }
}

// ===========================================================================
// Tests: every KAT byte string printed in the brief, plus the forged-bytes
// rejection corpus (brief §6).
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn int() -> Shape {
        Shape::Scalar(ScalarKind::Int)
    }
    fn text() -> Shape {
        Shape::Scalar(ScalarKind::Text)
    }
    // ty is copied from the schema and does not affect value bytes; tests that
    // do not exercise identity use ty 0.
    fn prod(leaves: Vec<Shape>) -> Shape {
        Shape::Product { ty: 0, leaves }
    }
    fn opt(inner: Shape) -> Shape {
        option_shape(0, inner)
    }
    fn pval(fields: Vec<DecodedValue>) -> DecodedValue {
        DecodedValue::Product { ty: 0, fields }
    }
    fn sval(variant: u32, payload: Vec<DecodedValue>) -> DecodedValue {
        DecodedValue::Sum {
            ty: 0,
            variant,
            payload,
        }
    }

    // ---- KATs from the brief / task ----

    #[test]
    fn kat_scalar_int_5_outer_no_prefix() {
        // int(5) = [0x35]  ('5' as decimal text, outer cell, no length prefix)
        assert_eq!(decode(&[0x35], &int()), Ok(DecodedValue::Int(5)));
    }

    #[test]
    fn kat_scalar_int_7_outer() {
        assert_eq!(decode(&[0x37], &int()), Ok(DecodedValue::Int(7)));
    }

    #[test]
    fn kat_product_int5_str_ab() {
        // product{int 5, str "ab"} = [0x01,0x35, 0x02,0x61,0x62]
        // nested scalar leaves each carry a minimal-LEB128 length prefix.
        let shape = prod(vec![int(), text()]);
        let bytes = [0x01, 0x35, 0x02, 0x61, 0x62];
        assert_eq!(
            decode(&bytes, &shape),
            Ok(pval(vec![
                DecodedValue::Int(5),
                DecodedValue::Text("ab".to_string()),
            ]))
        );
    }

    #[test]
    fn kat_option_none() {
        // none = [0x00]  (sum variant index 0, empty payload)
        assert_eq!(decode(&[0x00], &opt(int())), Ok(sval(0, vec![])));
    }

    #[test]
    fn kat_option_some_none() {
        // some(none) = [0x01,0x00]  (nested Option; composite payload leaf is
        // NOT length-prefixed, self-delimits by schema walk)
        assert_eq!(
            decode(&[0x01, 0x00], &opt(opt(int()))),
            Ok(sval(1, vec![sval(0, vec![])]))
        );
    }

    #[test]
    fn kat_option_some_some_7() {
        // some(some(7)) = [0x01,0x01, 0x01,0x37]
        // outer some=0x01, inner some=0x01, then int 7 as a length-prefixed
        // scalar leaf: len 1 (0x01) then '7' (0x37).
        assert_eq!(
            decode(&[0x01, 0x01, 0x01, 0x37], &opt(opt(int()))),
            Ok(sval(1, vec![sval(1, vec![DecodedValue::Int(7)])]))
        );
    }

    #[test]
    fn empty_struct_payload() {
        // struct with no leaves: empty cell, dense, consumes nothing.
        assert_eq!(decode(&[], &prod(vec![])), Ok(pval(vec![])));
    }

    #[test]
    fn nested_bytes_leaf() {
        // Bytes leaf preserves raw bytes verbatim (length-prefixed).
        let shape = prod(vec![Shape::Scalar(ScalarKind::Bytes)]);
        assert_eq!(
            decode(&[0x02, 0xFF, 0x00], &shape),
            Ok(pval(vec![DecodedValue::Bytes(vec![0xFF, 0x00])]))
        );
    }

    #[test]
    fn type_index_is_identity_bearing() {
        // ty is carried from the schema into the decoded value ...
        let shape = Shape::Product {
            ty: 42,
            leaves: vec![int()],
        };
        assert_eq!(
            decode(&[0x01, 0x35], &shape),
            Ok(DecodedValue::Product {
                ty: 42,
                fields: vec![DecodedValue::Int(5)]
            })
        );
        // ... and structurally identical composites of different type identity
        // are unequal values (matches equality::value_equality).
        assert_ne!(
            DecodedValue::Product {
                ty: 1,
                fields: vec![]
            },
            DecodedValue::Product {
                ty: 2,
                fields: vec![]
            }
        );
    }

    // ---- Forged-bytes corpus: MUST be rejected, never normalized (brief §6) ----

    #[test]
    fn reject_non_minimal_varint_in_sum_index() {
        // variant index 1 encoded non-minimally as [0x81,0x00].
        assert_eq!(
            decode(&[0x81, 0x00], &opt(int())),
            Err(DecodeError::NonMinimalVarint)
        );
    }

    #[test]
    fn reject_non_minimal_varint_in_leaf_length() {
        // scalar leaf length 1 encoded non-minimally as [0x81,0x00].
        assert_eq!(
            decode(&[0x81, 0x00, 0x35], &prod(vec![int()])),
            Err(DecodeError::NonMinimalVarint)
        );
    }

    #[test]
    fn reject_truncation_mid_leaf() {
        // leaf claims length 5 but only 2 bytes follow.
        assert_eq!(
            decode(&[0x05, 0x61, 0x62], &prod(vec![text()])),
            Err(DecodeError::Truncated)
        );
    }

    #[test]
    fn reject_varint_truncated() {
        // continuation bit set at end of buffer.
        assert_eq!(
            decode(&[0x81], &opt(int())),
            Err(DecodeError::VarintTruncated)
        );
    }

    #[test]
    fn reject_trailing_bytes() {
        // some(some(7)) plus one stray byte.
        assert_eq!(
            decode(&[0x01, 0x01, 0x01, 0x37, 0xFF], &opt(opt(int()))),
            Err(DecodeError::TrailingBytes)
        );
    }

    #[test]
    fn reject_variant_out_of_range() {
        // Option has 2 variants; index 2 is out of range.
        assert_eq!(
            decode(&[0x02], &opt(int())),
            Err(DecodeError::VariantOutOfRange {
                index: 2,
                variant_count: 2
            })
        );
    }

    #[test]
    fn reject_over_cap_leaf_length_before_alloc() {
        // leaf length prefix = 65537 (> MAX_TEXT_BYTES) => refused before any
        // allocation; no payload bytes even present.
        // 65537 = [0x81,0x80,0x04] minimal LEB128.
        assert_eq!(
            decode(&[0x81, 0x80, 0x04], &prod(vec![text()])),
            Err(DecodeError::LeafLengthOverCap { len: 65537 })
        );
    }

    #[test]
    fn reject_non_canonical_int_leading_zero() {
        // outer int "05" is non-canonical.
        assert_eq!(
            decode(&[0x30, 0x35], &int()),
            Err(DecodeError::NonCanonicalInt)
        );
    }

    #[test]
    fn reject_non_canonical_int_negative_zero() {
        // "-0"
        assert_eq!(
            decode(&[0x2D, 0x30], &int()),
            Err(DecodeError::NonCanonicalInt)
        );
    }

    #[test]
    fn accept_canonical_negative_int() {
        // "-5"
        assert_eq!(decode(&[0x2D, 0x35], &int()), Ok(DecodedValue::Int(-5)));
    }

    #[test]
    fn accept_int_i64_bounds() {
        // int is backed by i64 (§2 A12): both extremes decode.
        assert_eq!(
            decode(b"9223372036854775807", &int()),
            Ok(DecodedValue::Int(i64::MAX))
        );
        assert_eq!(
            decode(b"-9223372036854775808", &int()),
            Ok(DecodedValue::Int(i64::MIN))
        );
        // one past i64::MAX overflows -> rejected.
        assert_eq!(
            decode(b"9223372036854775808", &int()),
            Err(DecodeError::NonCanonicalInt)
        );
    }

    #[test]
    fn reject_invalid_utf8_text_leaf() {
        assert_eq!(
            decode(&[0x01, 0xFF], &prod(vec![text()])),
            Err(DecodeError::InvalidUtf8)
        );
    }

    #[test]
    fn depth_32_accepted_33_rejected() {
        // A12 convention: scalar leaves are free; the deepest admitted shape is
        // 32 nested composites over one scalar leaf.
        // innermost scalar int 5 = length-prefixed [0x01,0x35]; wrappers add nothing.
        let mut shape = int();
        for _ in 0..32 {
            shape = prod(vec![shape]);
        }
        assert!(decode(&[0x01, 0x35], &shape).is_ok());

        // 33 nested composites: the 33rd is refused before descent.
        let mut shape33 = int();
        for _ in 0..33 {
            shape33 = prod(vec![shape33]);
        }
        assert_eq!(
            decode(&[0x01, 0x35], &shape33),
            Err(DecodeError::ShapeInvalid(
                "depth exceeds MAX_DURABLE_VALUE_DEPTH"
            ))
        );
    }

    #[test]
    fn reject_value_bytes_over_cap() {
        let big = vec![0x35u8; MAX_DURABLE_VALUE_BYTES + 1];
        assert_eq!(
            decode(&big, &int()),
            Err(DecodeError::ValueBytesOverCap {
                len: MAX_DURABLE_VALUE_BYTES + 1
            })
        );
    }

    #[test]
    fn reject_bad_variant_shape_too_many_variants() {
        // A sum with 257 variants violates MAX_VARIANTS.
        let variants: Vec<Vec<Shape>> = (0..257).map(|_| vec![]).collect();
        assert_eq!(
            validate_shape(&Shape::Sum { ty: 0, variants }),
            Err(DecodeError::ShapeInvalid(
                "variant count exceeds MAX_VARIANTS"
            ))
        );
    }

    // ---- Bool canonical form (§2 law 3 A10: ASCII '0'/'1') ----

    #[test]
    fn bool_ascii_zero_one_canonical() {
        let shape = prod(vec![Shape::Scalar(ScalarKind::Bool)]);
        // len 1, byte '1' (0x31) = true
        assert_eq!(
            decode(&[0x01, 0x31], &shape),
            Ok(pval(vec![DecodedValue::Bool(true)]))
        );
        // len 1, byte '0' (0x30) = false
        assert_eq!(
            decode(&[0x01, 0x30], &shape),
            Ok(pval(vec![DecodedValue::Bool(false)]))
        );
        // 0x00/0x01 (the pre-A10 guess) is now non-canonical.
        assert_eq!(
            decode(&[0x01, 0x00], &shape),
            Err(DecodeError::NonCanonicalBool)
        );
        // length-2 bool is non-canonical.
        assert_eq!(
            decode(&[0x02, 0x30, 0x31], &shape),
            Err(DecodeError::NonCanonicalBool)
        );
    }

    // ---- Temporal canonical forms (§2 law 3 A10/A11/A12) ----

    fn date() -> Shape {
        Shape::Scalar(ScalarKind::Date)
    }
    fn instant() -> Shape {
        Shape::Scalar(ScalarKind::Instant)
    }
    fn duration() -> Shape {
        Shape::Scalar(ScalarKind::Duration)
    }

    #[test]
    fn date_accept_and_reject() {
        // "2026-07-16"
        assert_eq!(
            decode(b"2026-07-16", &date()),
            Ok(DecodedValue::Date {
                year: 2026,
                month: 7,
                day: 16
            })
        );
        // leap day accepted (2024 is leap): "2024-02-29"
        assert_eq!(
            decode(b"2024-02-29", &date()),
            Ok(DecodedValue::Date {
                year: 2024,
                month: 2,
                day: 29
            })
        );
        // minimum year 0001 accepted; zero-padded fields are canonical.
        assert_eq!(
            decode(b"0001-01-01", &date()),
            Ok(DecodedValue::Date {
                year: 1,
                month: 1,
                day: 1
            })
        );
        // year 0000 invalid (A11).
        assert_eq!(
            decode(b"0000-01-01", &date()),
            Err(DecodeError::NonCanonicalTemporal("year range"))
        );
        // Feb 29 in non-leap 2023 rejected.
        assert_eq!(
            decode(b"2023-02-29", &date()),
            Err(DecodeError::NonCanonicalTemporal("day range"))
        );
        // month 13 out of range.
        assert_eq!(
            decode(b"2026-13-01", &date()),
            Err(DecodeError::NonCanonicalTemporal("month range"))
        );
        // day 00 out of range.
        assert_eq!(
            decode(b"2026-07-00", &date()),
            Err(DecodeError::NonCanonicalTemporal("day range"))
        );
        // wrong separator / layout.
        assert_eq!(
            decode(b"2026/07/16", &date()),
            Err(DecodeError::NonCanonicalTemporal("date layout"))
        );
        // wrong width (2-digit year).
        assert_eq!(
            decode(b"26-07-16", &date()),
            Err(DecodeError::NonCanonicalTemporal("date layout"))
        );
    }

    #[test]
    fn instant_accept_and_reject() {
        // No fraction: "2026-07-16T13:45:07Z"
        assert_eq!(
            decode(b"2026-07-16T13:45:07Z", &instant()),
            Ok(DecodedValue::Instant {
                year: 2026,
                month: 7,
                day: 16,
                hour: 13,
                min: 45,
                sec: 7,
                nanos: 0,
            })
        );
        // Canonical fraction ".5" => 500_000_000 ns.
        assert_eq!(
            decode(b"2026-07-16T13:45:07.5Z", &instant()),
            Ok(DecodedValue::Instant {
                year: 2026,
                month: 7,
                day: 16,
                hour: 13,
                min: 45,
                sec: 7,
                nanos: 500_000_000,
            })
        );
        // Full 9-digit fraction.
        assert_eq!(
            decode(b"2026-07-16T13:45:07.123456789Z", &instant()),
            Ok(DecodedValue::Instant {
                year: 2026,
                month: 7,
                day: 16,
                hour: 13,
                min: 45,
                sec: 7,
                nanos: 123_456_789,
            })
        );
        // Trailing-zero fraction rejected (".50" is non-canonical; use ".5").
        assert_eq!(
            decode(b"2026-07-16T13:45:07.50Z", &instant()),
            Err(DecodeError::NonCanonicalTemporal("fraction trailing zero"))
        );
        // Empty fraction (bare '.') rejected.
        assert_eq!(
            decode(b"2026-07-16T13:45:07.Z", &instant()),
            Err(DecodeError::NonCanonicalTemporal("fraction width"))
        );
        // Over-long (>9 digit) fraction rejected.
        assert_eq!(
            decode(b"2026-07-16T13:45:07.1234567891Z", &instant()),
            Err(DecodeError::NonCanonicalTemporal("fraction width"))
        );
        // hour 24 out of range.
        assert_eq!(
            decode(b"2026-07-16T24:00:00Z", &instant()),
            Err(DecodeError::NonCanonicalTemporal("hour range"))
        );
        // second 60 (leap second) rejected.
        assert_eq!(
            decode(b"2026-07-16T13:45:60Z", &instant()),
            Err(DecodeError::NonCanonicalTemporal("second range"))
        );
        // missing trailing Z.
        assert_eq!(
            decode(b"2026-07-16T13:45:07 ", &instant()),
            Err(DecodeError::NonCanonicalTemporal("instant layout"))
        );
        // truncated (missing Z entirely -> too short).
        assert_eq!(
            decode(b"2026-07-16T13:45:07", &instant()),
            Err(DecodeError::NonCanonicalTemporal("instant layout"))
        );
        // invalid embedded date propagates.
        assert_eq!(
            decode(b"2026-00-16T13:45:07Z", &instant()),
            Err(DecodeError::NonCanonicalTemporal("month range"))
        );
    }

    #[test]
    fn duration_accept_and_reject() {
        // "PT0S" (canonical zero)
        assert_eq!(
            decode(b"PT0S", &duration()),
            Ok(DecodedValue::Duration {
                negative: false,
                seconds: 0,
                nanos: 0
            })
        );
        // "PT3600S"
        assert_eq!(
            decode(b"PT3600S", &duration()),
            Ok(DecodedValue::Duration {
                negative: false,
                seconds: 3600,
                nanos: 0
            })
        );
        // Fractional: "PT0.5S" => 0 s + 500_000_000 ns.
        assert_eq!(
            decode(b"PT0.5S", &duration()),
            Ok(DecodedValue::Duration {
                negative: false,
                seconds: 0,
                nanos: 500_000_000
            })
        );
        // Negative non-zero magnitude accepted: "-PT5S".
        assert_eq!(
            decode(b"-PT5S", &duration()),
            Ok(DecodedValue::Duration {
                negative: true,
                seconds: 5,
                nanos: 0
            })
        );
        // Negative fractional non-zero accepted: "-PT0.5S".
        assert_eq!(
            decode(b"-PT0.5S", &duration()),
            Ok(DecodedValue::Duration {
                negative: true,
                seconds: 0,
                nanos: 500_000_000
            })
        );
        // "-PT0S" rejected (no canonical negative zero).
        assert_eq!(
            decode(b"-PT0S", &duration()),
            Err(DecodeError::NonCanonicalTemporal("duration negative zero"))
        );
        // leading zero in whole-seconds count rejected.
        assert_eq!(
            decode(b"PT007S", &duration()),
            Err(DecodeError::NonCanonicalTemporal("duration leading zero"))
        );
        // sign inside PT (not a prefix) rejected.
        assert_eq!(
            decode(b"PT-5S", &duration()),
            Err(DecodeError::NonCanonicalTemporal("duration count digits"))
        );
        // trailing-zero fraction rejected.
        assert_eq!(
            decode(b"PT1.50S", &duration()),
            Err(DecodeError::NonCanonicalTemporal("fraction trailing zero"))
        );
        // bare '.' fraction rejected.
        assert_eq!(
            decode(b"PT1.S", &duration()),
            Err(DecodeError::NonCanonicalTemporal("fraction width"))
        );
        // empty count rejected (3 bytes -> caught by the layout/length guard,
        // since the shortest canonical duration "PT0S" is 4 bytes).
        assert_eq!(
            decode(b"PTS", &duration()),
            Err(DecodeError::NonCanonicalTemporal("duration layout"))
        );
        // missing PT prefix.
        assert_eq!(
            decode(b"T5S", &duration()),
            Err(DecodeError::NonCanonicalTemporal("duration layout"))
        );
        // missing S suffix.
        assert_eq!(
            decode(b"PT5", &duration()),
            Err(DecodeError::NonCanonicalTemporal("duration layout"))
        );
    }

    #[test]
    fn duration_i128_magnitude_bounds() {
        // Positive i128::MAX nanoseconds: floor(i128::MAX/1e9) whole seconds and
        // fraction .884105727 (brief §2 A12). Whole-seconds needs u128.
        assert_eq!(
            decode(b"PT170141183460469231731687303715.884105727S", &duration()),
            Ok(DecodedValue::Duration {
                negative: false,
                seconds: 170_141_183_460_469_231_731_687_303_715u128,
                nanos: 884_105_727,
            })
        );
        // Negative i128::MIN magnitude (= 2^127): fraction .884105728.
        assert_eq!(
            decode(b"-PT170141183460469231731687303715.884105728S", &duration()),
            Ok(DecodedValue::Duration {
                negative: true,
                seconds: 170_141_183_460_469_231_731_687_303_715u128,
                nanos: 884_105_728,
            })
        );
        // One nanosecond beyond i128::MAX (positive) is the canonical form of no
        // value -> rejected.
        assert_eq!(
            decode(b"PT170141183460469231731687303715.884105728S", &duration()),
            Err(DecodeError::NonCanonicalTemporal(
                "duration magnitude range"
            ))
        );
        // A u64-narrow decoder would diverge here: 2e19 whole seconds is well
        // within i128 and must decode (this is the slice-F u64 finding).
        assert_eq!(
            decode(b"PT20000000000000000000S", &duration()),
            Ok(DecodedValue::Duration {
                negative: false,
                seconds: 20_000_000_000_000_000_000u128, // > u64::MAX
                nanos: 0,
            })
        );
    }

    // ---- Descriptor parsing (§A7, exact A12 facts) ----

    fn write_descriptor(shape: &Shape, out: &mut Vec<u8>) {
        match shape {
            Shape::Scalar(k) => {
                out.push(0x00);
                out.push(scalar_kind_to_tag(*k));
            }
            Shape::Product { ty, leaves } => {
                out.push(0x01);
                out.extend_from_slice(&ty.to_be_bytes());
                out.extend_from_slice(&(leaves.len() as u16).to_be_bytes());
                for l in leaves {
                    write_descriptor(l, out);
                }
            }
            Shape::Sum { ty, variants } => {
                out.push(0x02);
                out.extend_from_slice(&ty.to_be_bytes());
                out.extend_from_slice(&(variants.len() as u16).to_be_bytes());
                for v in variants {
                    out.extend_from_slice(&(v.len() as u16).to_be_bytes());
                    for l in v {
                        write_descriptor(l, out);
                    }
                }
            }
        }
    }

    #[test]
    fn descriptor_bytes_kat() {
        // Brief §A7 A12 printed KAT: record { int, Option[string] }
        //   Product{ ty:3, [ Scalar(int), Sum{ ty:4, [ [], [Scalar(string)] ] } ] }
        // = the 18 bytes below (BE u16s; int tag 0x02, string tag 0x03).
        let bytes = [
            0x01, 0x00, 0x03, 0x00, 0x02, // product ty=3 leaf-count=2
            0x00, 0x02, //                   leaf0: scalar int
            0x02, 0x00, 0x04, 0x00, 0x02, // leaf1: sum ty=4 variant-count=2
            0x00, 0x00, //                     variant0: payload-count 0
            0x00, 0x01, 0x00, 0x03, //         variant1: payload-count 1, scalar string
        ];
        assert_eq!(bytes.len(), 18);
        let expected = Shape::Product {
            ty: 3,
            leaves: vec![
                Shape::Scalar(ScalarKind::Int),
                Shape::Sum {
                    ty: 4,
                    variants: vec![vec![], vec![Shape::Scalar(ScalarKind::Text)]],
                },
            ],
        };
        assert_eq!(parse_descriptor(&bytes), Ok(expected));
    }

    #[test]
    fn descriptor_empty_product_kat() {
        // Brief §A7 A12: Product{ ty:7, [] } -> 01 00 07 00 00 (5 bytes).
        assert_eq!(
            parse_descriptor(&[0x01, 0x00, 0x07, 0x00, 0x00]),
            Ok(Shape::Product {
                ty: 7,
                leaves: vec![]
            })
        );
    }

    #[test]
    fn descriptor_scalar_tag_bytes() {
        // A12 profile-descriptor scalar-kind tag bytes.
        for (tag, kind) in [
            (0x01u8, ScalarKind::Bool),
            (0x02, ScalarKind::Int),
            (0x03, ScalarKind::Text),
            (0x04, ScalarKind::Bytes),
            (0x05, ScalarKind::Date),
            (0x06, ScalarKind::Duration),
            (0x07, ScalarKind::Instant),
        ] {
            assert_eq!(parse_descriptor(&[0x00, tag]), Ok(Shape::Scalar(kind)));
        }
        // 0x00 is not a scalar-kind tag.
        assert_eq!(
            parse_descriptor(&[0x00, 0x00]),
            Err(DecodeError::BadDescriptor("unknown scalar-kind-tag"))
        );
    }

    #[test]
    fn descriptor_round_trip_preserves_type_index() {
        let shape = option_shape(
            9,
            Shape::Product {
                ty: 5,
                leaves: vec![int(), text()],
            },
        );
        let mut bytes = Vec::new();
        write_descriptor(&shape, &mut bytes);
        assert_eq!(parse_descriptor(&bytes), Ok(shape));
    }

    #[test]
    fn descriptor_rejects_trailing() {
        let mut bytes = Vec::new();
        write_descriptor(&int(), &mut bytes);
        bytes.push(0xEE);
        assert_eq!(
            parse_descriptor(&bytes),
            Err(DecodeError::BadDescriptor("trailing descriptor bytes"))
        );
    }
}
