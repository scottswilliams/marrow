//! Canonical saved-value encoding.
//!
//! Values are stored in a backend-independent canonical byte form, so backup,
//! diff, equality, and restore are stable. The bytes carry no type tag — the
//! type comes from the schema at read time — and are not order-preserving, since
//! the store orders by key rather than by value.

use marrow_codes::Code;

use super::civil::{
    NANOS_PER_DAY, NANOS_PER_SEC, civil_from_days, date_parts, supported_date_days,
    supported_instant_nanos,
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
    DateOutOfRange { days: i32 },
    InstantOutOfRange { nanos: i128 },
}

impl ValueError {
    /// The stable dotted code a tool reports for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::DateOutOfRange { .. } | Self::InstantOutOfRange { .. } => {
                Code::ValueRange.as_str()
            }
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
        RuntimeScalar::Date(days) => format_date(*days)?.into_bytes(),
        RuntimeScalar::Duration(nanos) => format_duration(*nanos).into_bytes(),
        RuntimeScalar::Instant(nanos) => format_instant(*nanos)?.into_bytes(),
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

/// Formats days-since-epoch as `YYYY-MM-DD`, erroring outside year 0001-9999 since a
/// wider year is not the 4-digit form `decode_value` reads.
fn format_date(days: i32) -> Result<String, ValueError> {
    let parts = date_parts(days).ok_or(ValueError::DateOutOfRange { days })?;
    Ok(format!(
        "{:04}-{:02}-{:02}",
        parts.year, parts.month, parts.day
    ))
}

/// Parses fixed-width canonical `YYYY-MM-DD` to days-since-epoch; unpadded fields,
/// stray separators, and impossible dates are all rejected.
fn parse_date(bytes: &[u8]) -> Option<i32> {
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = parse_digit_field(&bytes[0..4])?;
    let month = parse_digit_field(&bytes[5..7])?;
    let day = parse_digit_field(&bytes[8..10])?;
    super::civil::date_days(year as i32, month, day)
}

/// Parses an all-ASCII-digit field as a `u32`; any non-digit byte rejects it, so
/// unpadded or signed fields never parse.
fn parse_digit_field(slice: &[u8]) -> Option<u32> {
    if slice.iter().all(u8::is_ascii_digit) {
        std::str::from_utf8(slice).ok()?.parse().ok()
    } else {
        None
    }
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

/// Appends the canonical sub-second fraction of `nanos_fraction` (in `[0, 10^9)`):
/// nothing when zero, else `.` and the nine-digit fraction with trailing zeros
/// trimmed. Shared so the duration and instant codecs format fractions identically.
fn push_nanos_fraction(out: &mut String, nanos_fraction: u32) {
    if nanos_fraction > 0 {
        out.push('.');
        out.push_str(format!("{nanos_fraction:09}").trim_end_matches('0'));
    }
}

/// Parses a canonical sub-second fraction (the digits after the `.`) to nanoseconds.
/// The one canonical form is one to nine ASCII digits with no trailing zero, so an
/// empty, over-long, trailing-zero, or non-digit fraction is rejected. Single owner
/// of that rule for the duration and instant codecs.
fn parse_canonical_fraction(fraction: &[u8]) -> Option<i128> {
    if fraction.is_empty()
        || fraction.len() > 9
        || fraction.last() == Some(&b'0')
        || !fraction.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    format!("{:0<9}", std::str::from_utf8(fraction).ok()?)
        .parse()
        .ok()
}

/// Formats a signed nanosecond span as canonical `PT<seconds>S`: optional `-`, whole
/// seconds with no leading zeros, a trimmed fraction only when non-zero. Zero is `PT0S`.
fn format_duration(nanos: i128) -> String {
    let sign = if nanos < 0 { "-" } else { "" };
    let magnitude = nanos.unsigned_abs();
    let seconds = magnitude / NANOS_PER_SEC as u128;
    let fraction = (magnitude % NANOS_PER_SEC as u128) as u32;
    let mut out = format!("{sign}PT{seconds}");
    push_nanos_fraction(&mut out, fraction);
    out.push('S');
    out
}

/// Parses canonical `PT<seconds>S` to a signed nanosecond span, rejecting any
/// non-canonical spelling (leading zeros, a bad fraction, `-PT0S`, a missing
/// `PT`/`S`, or out-of-range magnitude).
fn parse_duration(bytes: &[u8]) -> Option<i128> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let body = rest.strip_prefix("PT")?.strip_suffix('S')?;
    let (seconds_text, fraction_text) = match body.split_once('.') {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (body, None),
    };

    if seconds_text.is_empty() || !seconds_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if seconds_text.len() > 1 && seconds_text.starts_with('0') {
        return None;
    }
    let seconds: i128 = seconds_text.parse().ok()?;

    let fraction_nanos: i128 = match fraction_text {
        None => 0,
        Some(fraction) => parse_canonical_fraction(fraction.as_bytes())?,
    };

    let magnitude = seconds
        .checked_mul(NANOS_PER_SEC)?
        .checked_add(fraction_nanos)?;
    if negative && magnitude == 0 {
        return None; // `-PT0S` has no canonical spelling; zero is `PT0S`
    }
    Some(if negative { -magnitude } else { magnitude })
}

/// Formats UTC nanoseconds-since-epoch as canonical `YYYY-MM-DDTHH:MM:SSZ`, with a
/// trimmed fraction only when non-zero. Errors outside year 0001-9999, like dates.
fn format_instant(nanos: i128) -> Result<String, ValueError> {
    if !supported_instant_nanos(nanos) {
        return Err(ValueError::InstantOutOfRange { nanos });
    }
    let days = nanos.div_euclid(NANOS_PER_DAY);
    let time_of_day = nanos.rem_euclid(NANOS_PER_DAY); // [0, NANOS_PER_DAY)
    let (year, month, day) = civil_from_days(days as i64);
    let total_seconds = (time_of_day / NANOS_PER_SEC) as u32; // [0, 86399]
    let fraction = (time_of_day % NANOS_PER_SEC) as u32;
    let (hours, minutes, seconds) = (
        total_seconds / 3600,
        (total_seconds % 3600) / 60,
        total_seconds % 60,
    );
    let mut out = format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}");
    push_nanos_fraction(&mut out, fraction);
    out.push('Z');
    Ok(out)
}

/// Parses canonical UTC `YYYY-MM-DDTHH:MM:SSZ` to nanoseconds since the epoch. Fixed
/// width through the seconds field, with an optional `.fraction` before the `Z`.
fn parse_instant(bytes: &[u8]) -> Option<i128> {
    if bytes.len() < 20 || bytes[10] != b'T' || *bytes.last()? != b'Z' {
        return None;
    }
    let days = i128::from(parse_date(&bytes[0..10])?);
    let time = &bytes[11..bytes.len() - 1]; // between `T` and `Z`
    if time.len() < 8 || time[2] != b':' || time[5] != b':' {
        return None;
    }
    let hours = parse_digit_field(&time[0..2])?;
    let minutes = parse_digit_field(&time[3..5])?;
    let seconds = parse_digit_field(&time[6..8])?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let fraction_nanos: i128 = if time.len() == 8 {
        0
    } else {
        if time[8] != b'.' {
            return None;
        }
        parse_canonical_fraction(&time[9..])?
    };
    let seconds_of_day = i128::from(hours * 3600 + minutes * 60 + seconds);
    Some(days * NANOS_PER_DAY + seconds_of_day * NANOS_PER_SEC + fraction_nanos)
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
