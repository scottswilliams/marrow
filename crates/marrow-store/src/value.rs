//! Canonical saved-value encoding.
//!
//! Saved values are stored in their canonical Marrow byte form
//! (docs/language/types.md): the bytes do not depend on the backend, so backup,
//! diff, traversal, equality, and restore are stable. Unlike keys, values are
//! not order-preserving — the store orders by path, not by value — so the
//! encoding optimizes for a clear canonical round-trip. A value's type comes
//! from the schema at read time, so the bytes carry no type tag.

/// A scalar saved value in decoded form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedValue {
    Bool(bool),
    Int(i64),
    Str(String),
    Bytes(Vec<u8>),
    ErrorCode(String),
    /// A calendar date, held as days since the Unix epoch (1970-01-01).
    Date(i32),
    /// An elapsed span, held as a signed count of nanoseconds.
    Duration(i128),
    /// A UTC instant, held as a signed count of nanoseconds since the epoch.
    Instant(i128),
    /// An exact base-10 decimal with value `coefficient * 10^(-scale)`, kept
    /// value-canonical (no trailing-zero scale).
    Decimal {
        coefficient: i128,
        scale: u32,
    },
}

/// A value that cannot be encoded to its canonical saved form. Today the only
/// such case is a `date`/`instant` whose calendar year falls outside the
/// documented 0001-9999 range (docs/language/types.md): formatting it would
/// produce a 5-7 digit year that [`decode_value`] could never read back, so the
/// codec rejects it rather than break the round-trip / one-canonical-form
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    /// A date's day count lies outside year 0001-9999.
    DateOutOfRange { days: i32 },
    /// An instant's calendar day lies outside year 0001-9999.
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
pub enum ValueType {
    Bool,
    Int,
    Str,
    Bytes,
    ErrorCode,
    Date,
    Duration,
    Instant,
    Decimal,
}

impl ValueType {
    /// The [`ValueType`] a scalar type name denotes, or `None` for identity and
    /// other non-scalar types. This is the single source of truth for the
    /// scalar-name mapping shared by the runtime and the write planner.
    pub fn from_scalar_name(name: &str) -> Option<ValueType> {
        Some(match name {
            "bool" => ValueType::Bool,
            "int" => ValueType::Int,
            "string" => ValueType::Str,
            "bytes" => ValueType::Bytes,
            "ErrorCode" => ValueType::ErrorCode,
            "date" => ValueType::Date,
            "instant" => ValueType::Instant,
            "duration" => ValueType::Duration,
            "decimal" => ValueType::Decimal,
            _ => return None,
        })
    }
}

/// Encode a value to its canonical saved bytes: `bool` as `0`/`1`, `int` as
/// decimal text, strings and error codes as UTF-8, bytes verbatim, dates as
/// `YYYY-MM-DD`, durations as `PT<seconds>S`, instants as `YYYY-MM-DDTHH:MM:SSZ`
/// (docs/language/types.md).
///
/// This is the canonical boundary: it produces only forms [`decode_value`] reads
/// back. A `date`/`instant` outside year 0001-9999 is a typed [`ValueError`]
/// rather than a non-decodable 5-7 digit year.
pub fn encode_value(value: &SavedValue) -> Result<Vec<u8>, ValueError> {
    Ok(match value {
        SavedValue::Bool(value) => vec![if *value { b'1' } else { b'0' }],
        SavedValue::Int(value) => value.to_string().into_bytes(),
        SavedValue::Str(text) | SavedValue::ErrorCode(text) => text.as_bytes().to_vec(),
        SavedValue::Bytes(bytes) => bytes.clone(),
        SavedValue::Date(days) => format_date(*days)?.into_bytes(),
        SavedValue::Duration(nanos) => format_duration(*nanos).into_bytes(),
        SavedValue::Instant(nanos) => format_instant(*nanos)?.into_bytes(),
        SavedValue::Decimal { coefficient, scale } => {
            format_decimal(*coefficient, *scale).into_bytes()
        }
    })
}

/// Decode canonical saved bytes as `ty`, or `None` if the bytes are not a valid
/// canonical form for that type. The check is strict, so non-canonical bytes
/// (e.g. `+1`, `01`, or a non-`0`/`1` boolean) are rejected rather than
/// silently normalized.
pub fn decode_value(bytes: &[u8], ty: ValueType) -> Option<SavedValue> {
    match ty {
        ValueType::Bool => match bytes {
            b"0" => Some(SavedValue::Bool(false)),
            b"1" => Some(SavedValue::Bool(true)),
            _ => None,
        },
        ValueType::Int => Some(SavedValue::Int(parse_canonical_int(bytes)?)),
        ValueType::Str => Some(SavedValue::Str(String::from_utf8(bytes.to_vec()).ok()?)),
        ValueType::Bytes => Some(SavedValue::Bytes(bytes.to_vec())),
        ValueType::ErrorCode => Some(SavedValue::ErrorCode(
            String::from_utf8(bytes.to_vec()).ok()?,
        )),
        ValueType::Date => Some(SavedValue::Date(parse_date(bytes)?)),
        ValueType::Duration => Some(SavedValue::Duration(parse_duration(bytes)?)),
        ValueType::Instant => Some(SavedValue::Instant(parse_instant(bytes)?)),
        ValueType::Decimal => parse_decimal(bytes),
    }
}

/// Parse the exact canonical decimal form `encode_value` produces: an optional
/// `-` then digits, no `+`, no leading zeros. Rejects anything that would not
/// round-trip identically (`+1`, `01`, `-0`, whitespace).
fn parse_canonical_int(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: i64 = text.parse().ok()?;
    (value.to_string() == text).then_some(value)
}

/// The number of days from the Unix epoch (1970-01-01) to `year-month-day`, or
/// `None` if it is out of range or not a real calendar date. Years run
/// 0001–9999 (docs/language/types.md). Validates by reconstructing the date, so
/// impossible dates such as 2021-02-29 are rejected.
pub fn date_days(year: i32, month: u32, day: u32) -> Option<i32> {
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let value = i32::try_from(days).ok()?;
    (civil_from_days(days) == (year, month, day)).then_some(value)
}

/// Format days-since-epoch as the canonical `YYYY-MM-DD`, or a typed range error
/// when the date's year falls outside 0001-9999 (`decode_value` only reads a
/// 4-digit year, so an out-of-range year would never round-trip).
fn format_date(days: i32) -> Result<String, ValueError> {
    let (year, month, day) = civil_from_days(days as i64);
    if !(1..=9999).contains(&year) {
        return Err(ValueError::DateOutOfRange { days });
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// Parse the canonical `YYYY-MM-DD` form to days-since-epoch. The shape is
/// fixed-width (10 bytes, dashes at indices 4 and 7, digits elsewhere), so
/// unpadded fields, stray separators, and impossible dates are all rejected.
fn parse_date(bytes: &[u8]) -> Option<i32> {
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let field = |slice: &[u8]| -> Option<u32> {
        if slice.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(slice).ok()?.parse().ok()
        } else {
            None
        }
    };
    let year = field(&bytes[0..4])?;
    let month = field(&bytes[5..7])?;
    let day = field(&bytes[8..10])?;
    date_days(year as i32, month, day)
}

/// Days from the Unix epoch to a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`). Valid for any real `month`/`day`; callers validate ranges.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month_part = (if month > 2 { month - 3 } else { month + 9 }) as i64; // [0, 11]
    let day_of_year = (153 * month_part + 2) / 5 + day as i64 - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// The proleptic-Gregorian date for a day count from the Unix epoch (the inverse
/// of [`days_from_civil`], Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719468;
    let era = (if days >= 0 { days } else { days - 146096 }) / 146097;
    let day_of_era = days - era * 146097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_part = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * month_part + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    }) as u32; // [1, 12]
    let year = year + i64::from(month <= 2);
    (year as i32, month, day)
}

const NANOS_PER_SEC: i128 = 1_000_000_000;
const NANOS_PER_DAY: i128 = 86_400 * NANOS_PER_SEC;

/// Format a signed nanosecond span as the canonical `PT<seconds>S`: an optional
/// `-`, whole seconds with no leading zeros, and a trailing-zero-trimmed
/// fraction only when non-zero. Zero is `PT0S`.
fn format_duration(nanos: i128) -> String {
    let sign = if nanos < 0 { "-" } else { "" };
    let magnitude = nanos.unsigned_abs();
    let seconds = magnitude / NANOS_PER_SEC as u128;
    let fraction = (magnitude % NANOS_PER_SEC as u128) as u32;
    let mut out = format!("{sign}PT{seconds}");
    if fraction > 0 {
        out.push('.');
        out.push_str(format!("{fraction:09}").trim_end_matches('0'));
    }
    out.push('S');
    out
}

/// Parse the canonical `PT<seconds>S` form to a signed nanosecond span,
/// rejecting any non-canonical spelling (leading zeros, a trailing-zero or empty
/// fraction, `-PT0S`, a missing `PT`/`S`, or out-of-range magnitude).
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
        return None; // leading zero
    }
    let seconds: i128 = seconds_text.parse().ok()?;

    let fraction_nanos: i128 = match fraction_text {
        None => 0,
        Some(fraction) => {
            if fraction.is_empty()
                || fraction.len() > 9
                || fraction.ends_with('0')
                || !fraction.bytes().all(|b| b.is_ascii_digit())
            {
                return None; // empty, too long, trailing zero, or non-digit
            }
            format!("{fraction:0<9}").parse().ok()?
        }
    };

    let magnitude = seconds
        .checked_mul(NANOS_PER_SEC)?
        .checked_add(fraction_nanos)?;
    if negative && magnitude == 0 {
        return None; // `-PT0S` is not canonical
    }
    Some(if negative { -magnitude } else { magnitude })
}

/// Format nanoseconds-since-epoch (UTC) as the canonical
/// `YYYY-MM-DDTHH:MM:SSZ`, including a trailing-zero-trimmed fraction only when
/// the sub-second part is non-zero. Returns a typed range error when the calendar
/// day falls outside year 0001-9999, matching the date boundary.
fn format_instant(nanos: i128) -> Result<String, ValueError> {
    let days = nanos.div_euclid(NANOS_PER_DAY);
    let time_of_day = nanos.rem_euclid(NANOS_PER_DAY); // [0, NANOS_PER_DAY)
    let (year, month, day) = civil_from_days(days as i64);
    if !(1..=9999).contains(&year) {
        return Err(ValueError::InstantOutOfRange { nanos });
    }
    let total_seconds = (time_of_day / NANOS_PER_SEC) as u32; // [0, 86399]
    let fraction = (time_of_day % NANOS_PER_SEC) as u32;
    let (hours, minutes, seconds) = (
        total_seconds / 3600,
        (total_seconds % 3600) / 60,
        total_seconds % 60,
    );
    let mut out = format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}");
    if fraction > 0 {
        out.push('.');
        out.push_str(format!("{fraction:09}").trim_end_matches('0'));
    }
    out.push('Z');
    Ok(out)
}

/// Parse the canonical `YYYY-MM-DDTHH:MM:SSZ` (UTC) form to nanoseconds since the
/// epoch. The shape is fixed-width through the seconds field, with an optional
/// `.fraction` before the `Z`; anything non-canonical is rejected.
fn parse_instant(bytes: &[u8]) -> Option<i128> {
    if bytes.len() < 20 || bytes[10] != b'T' || *bytes.last()? != b'Z' {
        return None;
    }
    let days = i128::from(parse_date(&bytes[0..10])?);
    let time = &bytes[11..bytes.len() - 1]; // between `T` and `Z`
    if time.len() < 8 || time[2] != b':' || time[5] != b':' {
        return None;
    }
    let field = |slice: &[u8]| -> Option<u32> {
        if slice.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(slice).ok()?.parse().ok()
        } else {
            None
        }
    };
    let hours = field(&time[0..2])?;
    let minutes = field(&time[3..5])?;
    let seconds = field(&time[6..8])?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let fraction_nanos: i128 = if time.len() == 8 {
        0
    } else {
        if time[8] != b'.' {
            return None;
        }
        let fraction = &time[9..];
        if fraction.is_empty()
            || fraction.len() > 9
            || fraction.last() == Some(&b'0')
            || !fraction.iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        format!("{:0<9}", std::str::from_utf8(fraction).ok()?)
            .parse()
            .ok()?
    };
    let seconds_of_day = i128::from(hours * 3600 + minutes * 60 + seconds);
    Some(days * NANOS_PER_DAY + seconds_of_day * NANOS_PER_SEC + fraction_nanos)
}

/// Format a stored decimal (`coefficient * 10^(-scale)`) as canonical decimal
/// text by routing through the shared [`Decimal`](crate::Decimal) codec, so the
/// saved form and decimal arithmetic share one normalization and one spelling.
/// A [`SavedValue::Decimal`] is only ever built from an in-envelope value (decoded
/// via [`parse_decimal`] or from a `Decimal`), so the construction always succeeds.
fn format_decimal(coefficient: i128, scale: u32) -> String {
    crate::Decimal::from_parts(coefficient, scale)
        .expect("a stored decimal is within the envelope")
        .to_text()
}

/// Parse canonical decimal text to a value-canonical [`SavedValue::Decimal`].
/// Rejects leading zeros, trailing-zero or empty fractions, `-0`, exponents, and
/// anything outside the decimal envelope.
fn parse_decimal(bytes: &[u8]) -> Option<SavedValue> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (integer, fraction) = match rest.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (rest, None),
    };

    if integer.is_empty() || !integer.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if integer.len() > 1 && integer.starts_with('0') {
        return None; // leading zero
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty()
            || fraction.ends_with('0')
            || !fraction.bytes().all(|b| b.is_ascii_digit()))
    {
        return None; // empty, trailing-zero, or non-digit fraction
    }

    let scale = fraction.map_or(0, str::len) as u32;
    let digits = match fraction {
        Some(fraction) => format!("{integer}{fraction}"),
        None => integer.to_string(),
    };
    let magnitude: i128 = digits.parse().ok()?; // out-of-range coefficient -> None
    if negative && magnitude == 0 {
        return None; // `-0` is not canonical
    }
    if crate::decimal::significant_digits(magnitude) > crate::decimal::MAX_DIGITS
        || scale > crate::decimal::MAX_DIGITS
    {
        return None;
    }
    let coefficient = if negative { -magnitude } else { magnitude };
    Some(SavedValue::Decimal { coefficient, scale })
}
