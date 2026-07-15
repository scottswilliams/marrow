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
