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
}

/// The type to decode saved bytes as. A typed read knows this from the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    Int,
    Str,
    Bytes,
    ErrorCode,
    Date,
}

/// Encode a value to its canonical saved bytes: `bool` as `0`/`1`, `int` as
/// decimal text, strings and error codes as UTF-8, bytes verbatim, dates as
/// `YYYY-MM-DD` (docs/language/types.md).
pub fn encode_value(value: &SavedValue) -> Vec<u8> {
    match value {
        SavedValue::Bool(value) => vec![if *value { b'1' } else { b'0' }],
        SavedValue::Int(value) => value.to_string().into_bytes(),
        SavedValue::Str(text) | SavedValue::ErrorCode(text) => text.as_bytes().to_vec(),
        SavedValue::Bytes(bytes) => bytes.clone(),
        SavedValue::Date(days) => format_date(*days).into_bytes(),
    }
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

/// Format days-since-epoch as the canonical `YYYY-MM-DD`.
fn format_date(days: i32) -> String {
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}")
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
