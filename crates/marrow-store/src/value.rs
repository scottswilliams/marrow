//! Canonical saved-value encoding.
//!
//! Values are stored in a backend-independent canonical byte form, so backup,
//! diff, equality, and restore are stable. The bytes carry no type tag — the
//! type comes from the schema at read time — and are not order-preserving, since
//! the store orders by tree-cell key rather than by value.

use crate::Decimal;
use crate::key::SavedKey;

/// Version of the canonical value encoding, recorded in a backup so a restore can
/// refuse data it cannot decode. Advances only on an incompatible byte-format change.
pub const VALUE_CODEC_VERSION: u32 = 0;

/// A decoded scalar value, shared by the store, runtime, and tooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scalar {
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
    /// An exact base-10 decimal, already in canonical form.
    Decimal(Decimal),
}

/// A saved scalar; the name the store and write planner read a [`Scalar`] under.
pub type SavedValue = Scalar;

impl Scalar {
    /// This scalar's order-preserving key projection, or `None` for a decimal, which
    /// has no order-preserving key encoding. The single home for that mapping.
    pub fn as_key(&self) -> Result<Option<SavedKey>, ValueError> {
        let key = match self {
            Scalar::Int(v) => SavedKey::Int(*v),
            Scalar::Bool(v) => SavedKey::Bool(*v),
            Scalar::Str(v) => SavedKey::Str(v.clone()),
            Scalar::Bytes(v) => SavedKey::Bytes(v.clone()),
            Scalar::Date(v) => SavedKey::Date(*v),
            Scalar::Duration(v) => SavedKey::Duration(*v),
            Scalar::Instant(v) => SavedKey::Instant(*v),
            Scalar::Decimal(_) => return Ok(None),
        };
        validate_scalar_key(&key)?;
        Ok(Some(key))
    }

    /// This scalar's type discriminant.
    pub fn ty(&self) -> ScalarType {
        match self {
            Scalar::Bool(_) => ScalarType::Bool,
            Scalar::Int(_) => ScalarType::Int,
            Scalar::Str(_) => ScalarType::Str,
            Scalar::Bytes(_) => ScalarType::Bytes,
            Scalar::Date(_) => ScalarType::Date,
            Scalar::Duration(_) => ScalarType::Duration,
            Scalar::Instant(_) => ScalarType::Instant,
            Scalar::Decimal(_) => ScalarType::Decimal,
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
            Self::DateOutOfRange { .. } | Self::InstantOutOfRange { .. } => "value.range",
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

/// The type to decode saved bytes as. A typed read knows this from the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Bool,
    Int,
    Str,
    Bytes,
    Date,
    Duration,
    Instant,
    Decimal,
}

/// Canonical scalar spellings shared by schema, checker, runtime, and tools.
/// `ErrorCode` is a language spelling stored as a string; placing it after the
/// canonical `string` entry keeps the reverse lookup of `Str` as `string`.
const SCALAR_NAMES: [(&str, ScalarType); 9] = [
    ("bool", ScalarType::Bool),
    ("int", ScalarType::Int),
    ("string", ScalarType::Str),
    ("bytes", ScalarType::Bytes),
    ("ErrorCode", ScalarType::Str),
    ("date", ScalarType::Date),
    ("instant", ScalarType::Instant),
    ("duration", ScalarType::Duration),
    ("decimal", ScalarType::Decimal),
];

impl ScalarType {
    pub fn from_scalar_name(name: &str) -> Option<ScalarType> {
        SCALAR_NAMES
            .iter()
            .find(|(spelling, _)| *spelling == name)
            .map(|(_, ty)| *ty)
    }

    /// The canonical language spelling of this scalar type.
    pub fn name(self) -> &'static str {
        SCALAR_NAMES
            .iter()
            .find(|(_, ty)| *ty == self)
            .map(|(spelling, _)| *spelling)
            .expect("every scalar has a spelling")
    }
}

/// Encodes a value to its canonical saved bytes: `bool` as `0`/`1`, `int` as decimal
/// text, strings as UTF-8, bytes verbatim, dates as `YYYY-MM-DD`, durations as
/// `PT<seconds>S`, instants as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// The canonical boundary: it emits only forms [`decode_value`] reads back, so a
/// `date`/`instant` outside year 0001-9999 is a typed [`ValueError`].
pub fn encode_value(value: &SavedValue) -> Result<Vec<u8>, ValueError> {
    Ok(match value {
        SavedValue::Bool(value) => vec![if *value { b'1' } else { b'0' }],
        SavedValue::Int(value) => value.to_string().into_bytes(),
        SavedValue::Str(text) => text.as_bytes().to_vec(),
        SavedValue::Bytes(bytes) => bytes.clone(),
        SavedValue::Date(days) => format_date(*days)?.into_bytes(),
        SavedValue::Duration(nanos) => format_duration(*nanos).into_bytes(),
        SavedValue::Instant(nanos) => format_instant(*nanos)?.into_bytes(),
        SavedValue::Decimal(value) => value.to_text().into_bytes(),
    })
}

/// Decodes canonical saved bytes as `ty`, strictly: non-canonical bytes such as
/// `+1`, `01`, or a non-`0`/`1` boolean are rejected rather than normalized.
pub fn decode_value(bytes: &[u8], ty: ScalarType) -> Option<SavedValue> {
    match ty {
        ScalarType::Bool => match bytes {
            b"0" => Some(SavedValue::Bool(false)),
            b"1" => Some(SavedValue::Bool(true)),
            _ => None,
        },
        ScalarType::Int => Some(SavedValue::Int(parse_canonical_int(bytes)?)),
        ScalarType::Str => Some(SavedValue::Str(String::from_utf8(bytes.to_vec()).ok()?)),
        ScalarType::Bytes => Some(SavedValue::Bytes(bytes.to_vec())),
        ScalarType::Date => Some(SavedValue::Date(parse_date(bytes)?)),
        ScalarType::Duration => Some(SavedValue::Duration(parse_duration(bytes)?)),
        ScalarType::Instant => Some(SavedValue::Instant(parse_instant(bytes)?)),
        // Decode through the shared decimal codec, then enforce the
        // one-canonical-form invariant: a value is canonical iff it re-encodes to
        // the very bytes given, so non-canonical spellings (`1.50`, `01`, `-0`)
        // are rejected even though `Decimal::parse` would normalize them.
        ScalarType::Decimal => {
            let value = Decimal::parse(std::str::from_utf8(bytes).ok()?)?;
            (value.to_text().as_bytes() == bytes).then_some(SavedValue::Decimal(value))
        }
    }
}

/// Parses the canonical int form, rejecting anything that would not round-trip
/// identically (`+1`, `01`, `-0`, whitespace).
fn parse_canonical_int(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: i64 = text.parse().ok()?;
    (value.to_string() == text).then_some(value)
}

/// Days from the Unix epoch to `year-month-day` (years 0001-9999), or `None` if out
/// of range. Validates by reconstructing the date, so 2021-02-29 and the like fail.
pub fn date_days(year: i32, month: u32, day: u32) -> Option<i32> {
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let value = i32::try_from(days).ok()?;
    (civil_from_days(days) == (year, month, day)).then_some(value)
}

pub const SUPPORTED_DATE_MIN_DAYS: i32 = -719_162;
pub const SUPPORTED_DATE_MAX_DAYS: i32 = 2_932_896;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateParts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// The calendar components of a supported date day count.
pub fn date_parts(days: i32) -> Option<DateParts> {
    if !supported_date_days(days) {
        return None;
    }
    let (year, month, day) = civil_from_days(i64::from(days));
    Some(DateParts { year, month, day })
}

pub fn supported_date_days(days: i32) -> bool {
    (SUPPORTED_DATE_MIN_DAYS..=SUPPORTED_DATE_MAX_DAYS).contains(&days)
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
    date_days(year as i32, month, day)
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

/// Howard Hinnant's `days_from_civil`: proleptic-Gregorian date to days from the Unix
/// epoch. Valid for any real `month`/`day`; callers validate ranges.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month_part = (if month > 2 { month - 3 } else { month + 9 }) as i64;
    let day_of_year = (153 * month_part + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// Hinnant's `civil_from_days`, the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719468;
    let era = (if days >= 0 { days } else { days - 146096 }) / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_part + 2) / 5 + 1) as u32;
    let month = (if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    }) as u32;
    let year = year + i64::from(month <= 2);
    (year as i32, month, day)
}

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// Nanoseconds in a 24-hour day, the date<->nanos conversion factor. Owned here
/// alongside the date/instant codecs; the runtime imports it for the same model.
pub const NANOS_PER_DAY: i128 = 86_400 * NANOS_PER_SEC;

pub const SUPPORTED_INSTANT_MIN_NANOS: i128 = SUPPORTED_DATE_MIN_DAYS as i128 * NANOS_PER_DAY;
pub const SUPPORTED_INSTANT_MAX_NANOS: i128 =
    SUPPORTED_DATE_MAX_DAYS as i128 * NANOS_PER_DAY + (NANOS_PER_DAY - 1);

pub fn supported_instant_nanos(nanos: i128) -> bool {
    (SUPPORTED_INSTANT_MIN_NANOS..=SUPPORTED_INSTANT_MAX_NANOS).contains(&nanos)
}

pub fn validate_scalar_key(key: &SavedKey) -> Result<(), ValueError> {
    match key {
        SavedKey::Date(days) if !supported_date_days(*days) => {
            Err(ValueError::DateOutOfRange { days: *days })
        }
        SavedKey::Instant(nanos) if !supported_instant_nanos(*nanos) => {
            Err(ValueError::InstantOutOfRange { nanos: *nanos })
        }
        _ => Ok(()),
    }
}

pub fn scalar_key_matches_type(key: &SavedKey, expected: ScalarType) -> bool {
    key.scalar_type() == expected && validate_scalar_key(key).is_ok()
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
