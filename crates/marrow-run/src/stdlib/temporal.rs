//! Input parsing for instant and duration text.
//!
//! Saved temporal values have one canonical spelling, and the store decoder accepts
//! only that. User-facing input — `std::clock::parseInstant`/`parseDuration` and the
//! `instant(text)`/`duration(text)` constructors — is a wider surface: it accepts
//! standard RFC-3339 / ISO-8601 spellings and normalizes them to the canonical value.
//! Instants accept trailing-zero fractional seconds and explicit numeric offsets, with
//! a non-UTC offset shifted to the equivalent UTC instant since instants are stored in
//! UTC; durations accept trailing-zero fractional seconds. This widening lives only at
//! the input boundary; output and storage stay canonical.

use marrow_store::value::{
    NANOS_PER_DAY, SavedValue, ScalarType, date_days, decode_value, supported_instant_nanos,
};

const NANOS_PER_SEC: i128 = 1_000_000_000;

/// Parses standard RFC-3339 instant text to canonical UTC nanoseconds-since-epoch,
/// returning `None` for malformed text or a value outside the supported range. The
/// zone designator is a literal `Z` or a `±HH:MM` offset; a numeric offset is
/// subtracted to reach UTC.
pub(crate) fn parse_rfc3339_instant_nanos(text: &str) -> Option<i128> {
    let bytes = text.as_bytes();
    if bytes.len() < 11 || bytes[10] != b'T' {
        return None;
    }
    let days = i128::from(parse_date(&bytes[0..10])?);
    let (seconds_of_day, fraction_nanos, offset_nanos) = parse_time_and_zone(&bytes[11..])?;
    let nanos = days * NANOS_PER_DAY + i128::from(seconds_of_day) * NANOS_PER_SEC + fraction_nanos
        - offset_nanos;
    supported_instant_nanos(nanos).then_some(nanos)
}

/// Parses a fixed-width `YYYY-MM-DD` field to days-since-epoch, rejecting unpadded
/// fields, stray separators, and impossible dates.
fn parse_date(field: &[u8]) -> Option<i32> {
    if field.len() != 10 || field[4] != b'-' || field[7] != b'-' {
        return None;
    }
    let year = parse_uint(&field[0..4])?;
    let month = parse_uint(&field[5..7])?;
    let day = parse_uint(&field[8..10])?;
    date_days(i32::try_from(year).ok()?, month, day)
}

/// Parses the `HH:MM:SS[.fraction]<zone>` portion after the `T`, returning the
/// seconds within the day, the sub-second nanoseconds, and the zone's offset from
/// UTC in nanoseconds (zero for `Z`).
fn parse_time_and_zone(time: &[u8]) -> Option<(u32, i128, i128)> {
    if time.len() < 8 || time[2] != b':' || time[5] != b':' {
        return None;
    }
    let hours = parse_uint(&time[0..2])?;
    let minutes = parse_uint(&time[3..5])?;
    let seconds = parse_uint(&time[6..8])?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let seconds_of_day = hours * 3600 + minutes * 60 + seconds;

    let rest = &time[8..];
    let (fraction_bytes, zone) = match rest.split_first() {
        Some((b'.', after_dot)) => {
            let zone_start = after_dot.iter().position(|b| !b.is_ascii_digit())?;
            (Some(&after_dot[..zone_start]), &after_dot[zone_start..])
        }
        _ => (None, rest),
    };
    let fraction_nanos = match fraction_bytes {
        None => 0,
        Some(digits) => parse_fraction_nanos(digits)?,
    };
    let offset_nanos = parse_zone_offset_nanos(zone)?;
    Some((seconds_of_day, fraction_nanos, offset_nanos))
}

/// Parses a signed `PT<seconds>S` span to nanoseconds. The only widening over the
/// canonical store spelling is trailing-zero fractional seconds, so the fraction is
/// trimmed back to canonical form and the store decoder owns every other rule
/// (`PT`/`S` framing, no leading zeros, no `-PT0S`, overflow). `None` for malformed
/// or out-of-range text.
pub(crate) fn parse_iso8601_duration_nanos(text: &str) -> Option<i128> {
    match decode_value(
        canonicalize_duration_fraction(text).as_bytes(),
        ScalarType::Duration,
    )? {
        SavedValue::Duration(nanos) => Some(nanos),
        _ => None,
    }
}

/// Trims trailing zeros from a duration's fractional-second digits, dropping the `.`
/// when an all-zero fraction (`PT1.000S`) collapses to whole seconds, so a standard
/// ISO-8601 spelling reaches the canonical decoder as `PT1.5S` / `PT1S`. The fraction
/// runs from the `.` to the trailing `S`; a bare `.` with no digits, or text without a
/// `.`, is returned verbatim for the decoder to reject on its own terms.
fn canonicalize_duration_fraction(text: &str) -> String {
    let Some((head, after_dot)) = text.split_once('.') else {
        return text.to_string();
    };
    let Some((fraction, suffix)) = after_dot.split_once('S') else {
        return text.to_string();
    };
    let trimmed = fraction.trim_end_matches('0');
    if fraction.is_empty() || !suffix.is_empty() {
        text.to_string()
    } else if trimmed.is_empty() {
        format!("{head}S")
    } else {
        format!("{head}.{trimmed}S")
    }
}

/// Parses one to nine fractional-second digits to nanoseconds. Unlike the canonical
/// store decoder, trailing zeros are accepted here and normalized away on output.
fn parse_fraction_nanos(digits: &[u8]) -> Option<i128> {
    if digits.is_empty() || digits.len() > 9 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let padded = format!("{:0<9}", std::str::from_utf8(digits).ok()?);
    padded.parse().ok()
}

/// Parses an RFC-3339 zone designator to its offset from UTC in nanoseconds: `Z`
/// is zero, `±HH:MM` is the signed offset.
fn parse_zone_offset_nanos(zone: &[u8]) -> Option<i128> {
    if zone == b"Z" {
        return Some(0);
    }
    if zone.len() != 6 || zone[3] != b':' {
        return None;
    }
    let sign: i128 = match zone[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours = parse_uint(&zone[1..3])?;
    let minutes = parse_uint(&zone[4..6])?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (i128::from(hours) * 3600 + i128::from(minutes) * 60) * NANOS_PER_SEC)
}

/// Parses an all-ASCII-digit field as a `u32`; any non-digit byte rejects it, so
/// unpadded or signed fields never parse.
fn parse_uint(field: &[u8]) -> Option<u32> {
    if field.iter().all(u8::is_ascii_digit) {
        std::str::from_utf8(field).ok()?.parse().ok()
    } else {
        None
    }
}
