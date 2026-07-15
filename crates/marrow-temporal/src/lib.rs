//! The pure temporal scalar domain.
//!
//! One owner of the calendar, the supported range, the canonical text codec, and
//! the closed arithmetic floor for the three temporal value types:
//!
//! - `date` — a proleptic-Gregorian day, held as days since the Unix epoch
//!   (1970-01-01), restricted to years 0001-9999; canonical text `YYYY-MM-DD`.
//! - `instant` — a UTC instant, held as signed nanoseconds since the epoch, in the
//!   same year range; canonical text `YYYY-MM-DDTHH:MM:SS[.fraction]Z`.
//! - `duration` — a signed elapsed span of nanoseconds over the whole `i128` range;
//!   canonical text `[-]PT<seconds>[.fraction]S` (zero is `PT0S`).
//!
//! The codec is strict: every parser reads back exactly what its formatter writes
//! and rejects any non-canonical spelling. The crate has no dependency on a clock,
//! a timezone database, a locale, a store, or the program image, so the storeless
//! compiler (which validates temporal literals and folds them to constants) and the
//! runtime (which encodes saved values and evaluates temporal arithmetic) consume
//! the same implementation and cannot drift.

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// Nanoseconds in a 24-hour day, the date<->nanos conversion factor.
pub const NANOS_PER_DAY: i128 = 86_400 * NANOS_PER_SEC;

/// The inclusive supported day range: years 0001-9999, the span the fixed-width
/// canonical `YYYY-MM-DD` form reads back exactly.
pub const SUPPORTED_DATE_MIN_DAYS: i32 = -719_162;
pub const SUPPORTED_DATE_MAX_DAYS: i32 = 2_932_896;

/// The inclusive supported instant range, the nanosecond span covering the same
/// calendar years.
pub const SUPPORTED_INSTANT_MIN_NANOS: i128 = SUPPORTED_DATE_MIN_DAYS as i128 * NANOS_PER_DAY;
pub const SUPPORTED_INSTANT_MAX_NANOS: i128 =
    SUPPORTED_DATE_MAX_DAYS as i128 * NANOS_PER_DAY + (NANOS_PER_DAY - 1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateParts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
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

pub fn supported_instant_nanos(nanos: i128) -> bool {
    (SUPPORTED_INSTANT_MIN_NANOS..=SUPPORTED_INSTANT_MAX_NANOS).contains(&nanos)
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

// --- canonical text codec (raw scalars) ---

/// Formats days-since-epoch as canonical `YYYY-MM-DD`, or `None` outside year
/// 0001-9999 (a wider year is not the 4-digit form the parser reads).
pub fn format_date(days: i32) -> Option<String> {
    let parts = date_parts(days)?;
    Some(format!(
        "{:04}-{:02}-{:02}",
        parts.year, parts.month, parts.day
    ))
}

/// Parses fixed-width canonical `YYYY-MM-DD` to days-since-epoch; unpadded fields,
/// stray separators, and impossible dates are all rejected.
pub fn parse_date(bytes: &[u8]) -> Option<i32> {
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
/// empty, over-long, trailing-zero, or non-digit fraction is rejected.
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

/// Formats a signed nanosecond span as canonical `[-]PT<seconds>[.fraction]S`: whole
/// seconds with no leading zeros, a trimmed fraction only when non-zero. Zero is `PT0S`.
pub fn format_duration(nanos: i128) -> String {
    let sign = if nanos < 0 { "-" } else { "" };
    let magnitude = nanos.unsigned_abs();
    let seconds = magnitude / NANOS_PER_SEC as u128;
    let fraction = (magnitude % NANOS_PER_SEC as u128) as u32;
    let mut out = format!("{sign}PT{seconds}");
    push_nanos_fraction(&mut out, fraction);
    out.push('S');
    out
}

/// Parses canonical `[-]PT<seconds>[.fraction]S` to a signed nanosecond span,
/// rejecting any non-canonical spelling (leading zeros, a bad fraction, `-PT0S`, a
/// missing `PT`/`S`, or out-of-range magnitude).
pub fn parse_duration(bytes: &[u8]) -> Option<i128> {
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
    let seconds: u128 = seconds_text.parse().ok()?;

    let fraction_nanos: u128 = match fraction_text {
        None => 0,
        Some(fraction) => parse_canonical_fraction(fraction.as_bytes())? as u128,
    };

    // The magnitude is computed as an unsigned quantity so the whole signed range
    // round-trips: `format_duration` writes `i128::MIN` as `PT<|MIN|>S`, whose
    // magnitude (2^127) is one past `i128::MAX`, so only the negative branch admits
    // it.
    let magnitude = seconds
        .checked_mul(NANOS_PER_SEC as u128)?
        .checked_add(fraction_nanos)?;
    if negative {
        if magnitude == 0 {
            return None; // `-PT0S` has no canonical spelling; zero is `PT0S`
        }
        match i128::try_from(magnitude) {
            Ok(value) => Some(-value),
            Err(_) => (magnitude == 1u128 << 127).then_some(i128::MIN),
        }
    } else {
        i128::try_from(magnitude).ok()
    }
}

/// Formats UTC nanoseconds-since-epoch as canonical `YYYY-MM-DDTHH:MM:SS[.fraction]Z`,
/// with a trimmed fraction only when non-zero. `None` outside year 0001-9999.
pub fn format_instant(nanos: i128) -> Option<String> {
    if !supported_instant_nanos(nanos) {
        return None;
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
    Some(out)
}

/// Parses canonical UTC `YYYY-MM-DDTHH:MM:SS[.fraction]Z` to nanoseconds since the
/// epoch. Fixed width through the seconds field, with an optional `.fraction` before
/// the `Z`.
pub fn parse_instant(bytes: &[u8]) -> Option<i128> {
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

// --- the closed temporal arithmetic floor ---
//
// Every operation is total: it returns `None` exactly when the result would leave
// the supported day/nanosecond domain, which the runtime maps to a
// `run.temporal_overflow` fault. No operation reads a clock, timezone, or locale;
// calendar months and years are not duration units.

/// `date + days` in whole days, or `None` if the result leaves the supported
/// calendar range. `add` is an `i64` day count (a language `int`).
pub fn add_days(days: i32, add: i64) -> Option<i32> {
    let result = i64::from(days).checked_add(add)?;
    let result = i32::try_from(result).ok()?;
    supported_date_days(result).then_some(result)
}

/// The signed number of days from `from` to `to` (`to - from`). Both operands are
/// supported dates, so the difference always fits an `i64` and never faults.
pub fn days_between(from: i32, to: i32) -> i64 {
    i64::from(to) - i64::from(from)
}

/// `a + b` over signed-nanosecond durations, or `None` on `i128` overflow.
pub fn duration_add(a: i128, b: i128) -> Option<i128> {
    a.checked_add(b)
}

/// `a - b` over signed-nanosecond durations, or `None` on `i128` overflow.
pub fn duration_sub(a: i128, b: i128) -> Option<i128> {
    a.checked_sub(b)
}

/// `instant + duration`, or `None` if the result leaves the supported instant range.
pub fn instant_add_duration(instant: i128, duration: i128) -> Option<i128> {
    let result = instant.checked_add(duration)?;
    supported_instant_nanos(result).then_some(result)
}

/// `instant - duration`, or `None` if the result leaves the supported instant range.
pub fn instant_sub_duration(instant: i128, duration: i128) -> Option<i128> {
    let result = instant.checked_sub(duration)?;
    supported_instant_nanos(result).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> i32 {
        parse_date(text.as_bytes()).expect("canonical date")
    }
    fn instant(text: &str) -> i128 {
        parse_instant(text.as_bytes()).expect("canonical instant")
    }
    fn duration(text: &str) -> i128 {
        parse_duration(text.as_bytes()).expect("canonical duration")
    }

    #[test]
    fn date_round_trips_and_boundaries() {
        for text in ["0001-01-01", "1970-01-01", "2026-07-15", "9999-12-31"] {
            assert_eq!(format_date(date(text)).as_deref(), Some(text));
        }
        // The supported-range endpoints are exactly years 0001 and 9999.
        assert_eq!(date("1970-01-01"), 0);
        assert_eq!(date("0001-01-01"), SUPPORTED_DATE_MIN_DAYS);
        assert_eq!(date("9999-12-31"), SUPPORTED_DATE_MAX_DAYS);
        assert_eq!(
            format_date(SUPPORTED_DATE_MIN_DAYS).as_deref(),
            Some("0001-01-01")
        );
        assert_eq!(
            format_date(SUPPORTED_DATE_MAX_DAYS).as_deref(),
            Some("9999-12-31")
        );
        assert_eq!(format_date(SUPPORTED_DATE_MIN_DAYS - 1), None);
        assert_eq!(format_date(SUPPORTED_DATE_MAX_DAYS + 1), None);
    }

    #[test]
    fn date_parse_rejects_noncanonical() {
        for bad in [
            "2026-7-15",   // unpadded month
            "2026-07-5",   // unpadded day
            "2026/07/15",  // wrong separator
            "2026-13-01",  // month out of range
            "2021-02-29",  // not a leap year
            "0000-01-01",  // year below 0001
            "10000-01-01", // year above 9999 (wrong width)
            "2026-07-15 ", // trailing space
            "+2026-07-15", // signed
        ] {
            assert_eq!(parse_date(bad.as_bytes()), None, "{bad} must reject");
        }
    }

    #[test]
    fn instant_round_trips_and_fractions() {
        for text in [
            "0001-01-01T00:00:00Z",
            "2026-07-15T17:00:00Z",
            "2026-07-15T12:00:00.5Z",
            "2026-07-15T12:00:00.123456789Z",
            "9999-12-31T23:59:59.999999999Z",
        ] {
            assert_eq!(format_instant(instant(text)).as_deref(), Some(text));
        }
        assert_eq!(instant("1970-01-01T00:00:00Z"), 0);
        assert_eq!(instant("0001-01-01T00:00:00Z"), SUPPORTED_INSTANT_MIN_NANOS);
        assert_eq!(
            instant("9999-12-31T23:59:59.999999999Z"),
            SUPPORTED_INSTANT_MAX_NANOS
        );
        assert_eq!(format_instant(SUPPORTED_INSTANT_MIN_NANOS - 1), None);
        assert_eq!(format_instant(SUPPORTED_INSTANT_MAX_NANOS + 1), None);
    }

    #[test]
    fn instant_parse_rejects_noncanonical() {
        for bad in [
            "2026-07-15T12:00:00",             // missing Z
            "2026-07-15T12:00:00z",            // lowercase z
            "2026-07-15T24:00:00Z",            // hour out of range
            "2026-07-15T12:00:60Z",            // second out of range
            "2026-07-15T12:00:00.Z",           // empty fraction
            "2026-07-15T12:00:00.50Z",         // trailing-zero fraction
            "2026-07-15T12:00:00.1234567890Z", // over-long fraction
            "2026-07-15 12:00:00Z",            // space instead of T
        ] {
            assert_eq!(parse_instant(bad.as_bytes()), None, "{bad} must reject");
        }
    }

    #[test]
    fn duration_round_trips_and_zero() {
        for text in [
            "PT0S",
            "PT1S",
            "-PT1S",
            "PT90S",
            "PT0.5S",
            "-PT0.123456789S",
        ] {
            assert_eq!(format_duration(duration(text)), text);
        }
        assert_eq!(duration("PT0S"), 0);
        assert_eq!(duration("PT1S"), NANOS_PER_SEC);
        assert_eq!(duration("-PT1S"), -NANOS_PER_SEC);
        // The full i128 nanosecond range round-trips.
        assert_eq!(format_duration(i128::MAX), format_duration(i128::MAX));
        assert_eq!(
            parse_duration(format_duration(i128::MAX).as_bytes()),
            Some(i128::MAX)
        );
        assert_eq!(
            parse_duration(format_duration(i128::MIN).as_bytes()),
            Some(i128::MIN)
        );
    }

    #[test]
    fn duration_parse_rejects_noncanonical() {
        for bad in [
            "PT0.0S", // trailing-zero fraction
            "-PT0S",  // negative zero has no canonical form
            "PT01S",  // leading-zero seconds
            "PT1",    // missing S
            "1S",     // missing PT
            "PTS",    // missing seconds
            "PT1.S",  // empty fraction
            "PT-1S",  // sign in the wrong place
        ] {
            assert_eq!(parse_duration(bad.as_bytes()), None, "{bad} must reject");
        }
    }

    #[test]
    fn add_days_is_checked_at_the_boundary() {
        assert_eq!(add_days(date("2026-07-15"), 10), Some(date("2026-07-25")));
        assert_eq!(add_days(date("2026-07-25"), 10), Some(date("2026-08-04")));
        // Stepping past year 9999 or before 0001 faults (returns None).
        assert_eq!(add_days(SUPPORTED_DATE_MAX_DAYS, 1), None);
        assert_eq!(add_days(SUPPORTED_DATE_MIN_DAYS, -1), None);
        assert_eq!(add_days(SUPPORTED_DATE_MAX_DAYS, i64::MAX), None);
    }

    #[test]
    fn days_between_is_signed() {
        assert_eq!(days_between(date("2026-07-15"), date("2026-07-25")), 10);
        assert_eq!(days_between(date("2026-07-25"), date("2026-07-15")), -10);
        assert_eq!(days_between(date("2026-07-15"), date("2026-07-15")), 0);
    }

    #[test]
    fn duration_arithmetic_is_checked() {
        assert_eq!(
            duration_add(duration("PT60S"), duration("PT30S")),
            Some(duration("PT90S"))
        );
        assert_eq!(
            duration_sub(duration("PT60S"), duration("PT30S")),
            Some(duration("PT30S"))
        );
        assert_eq!(duration_add(i128::MAX, 1), None);
        assert_eq!(duration_sub(i128::MIN, 1), None);
    }

    #[test]
    fn instant_shift_is_range_checked() {
        let noon = instant("2026-07-15T12:00:00Z");
        assert_eq!(
            instant_add_duration(noon, duration("PT3600S")),
            Some(instant("2026-07-15T13:00:00Z"))
        );
        assert_eq!(
            instant_sub_duration(noon, duration("PT3600S")),
            Some(instant("2026-07-15T11:00:00Z"))
        );
        // Shifting past the supported range faults.
        assert_eq!(instant_add_duration(SUPPORTED_INSTANT_MAX_NANOS, 1), None);
        assert_eq!(instant_sub_duration(SUPPORTED_INSTANT_MIN_NANOS, 1), None);
    }
}
