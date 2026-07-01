//! Input parsing for instant and duration text.
//!
//! Saved temporal values have one canonical spelling, and the store decoder accepts
//! only that. User-facing input — `std::clock::parseInstant`/`parseDuration` and the
//! `instant(text)`/`duration(text)` constructors — is a wider surface: it accepts
//! standard RFC-3339 / ISO-8601 spellings and normalizes them to the canonical value.
//! Instants accept trailing-zero fractional seconds and explicit numeric offsets, with
//! a non-UTC offset shifted to the equivalent UTC instant since instants are stored in
//! UTC. Durations accept the time-based ISO-8601 subset `PnDTnHnMnS` of exact fixed
//! spans, summing each component to signed nanoseconds; nominal year/month components
//! are calendar-ambiguous and refused. This widening lives only at the input boundary;
//! output and storage stay canonical.

use marrow_store::value::{NANOS_PER_DAY, date_days, supported_instant_nanos};

const NANOS_PER_SEC: i128 = 1_000_000_000;
const NANOS_PER_MINUTE: i128 = 60 * NANOS_PER_SEC;
const NANOS_PER_HOUR: i128 = 60 * NANOS_PER_MINUTE;

/// Why an ISO-8601 duration string was rejected, so the boundary can render the
/// right diagnostic: a calendar component carries a clarifying note, while any other
/// malformed text falls back to the generic invalid-text message.
pub(crate) enum DurationParseError {
    /// A nominal year or month component. Marrow durations are exact signed spans
    /// with no calendar or DST arithmetic, so these have no unambiguous nanosecond
    /// width and are refused outright.
    CalendarAmbiguous,
    /// Any other unparseable or out-of-range text.
    Malformed,
}

impl DurationParseError {
    /// Renders a boundary diagnostic for rejected duration text by extending a
    /// caller-supplied base message: a calendar component appends the clarifying note
    /// that Marrow durations are exact spans, while any other malformed text keeps the
    /// generic message unchanged.
    pub(crate) fn message(&self, base: &str) -> String {
        match self {
            DurationParseError::CalendarAmbiguous => format!(
                "{base} (calendar-ambiguous; Marrow durations are exact spans — \
                 use days/hours/minutes/seconds)"
            ),
            DurationParseError::Malformed => base.to_string(),
        }
    }
}

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

/// Parses the time-based ISO-8601 duration subset `[-]PnDTnHnMnS` to a signed
/// nanosecond span. Every component is an exact fixed width — `1D` is 86400s, `1H`
/// 3600s, `1M` 60s — summed under an `i128` overflow envelope; only the seconds
/// component may carry a fraction. Nominal year/month components have no exact
/// nanosecond width and are refused. The result is a plain span; canonical
/// `PT<seconds>S` output is produced by the formatter, so no spelling is round-tripped
/// here.
pub(crate) fn parse_iso8601_duration_nanos(text: &str) -> Result<i128, DurationParseError> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let body = body
        .strip_prefix('P')
        .ok_or(DurationParseError::Malformed)?;

    let (date_part, time_part) = match body.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (body, None),
    };

    let mut magnitude: i128 = 0;
    let mut any_component = false;

    if !date_part.is_empty() {
        // A date-position component is only ever days for an exact span; a `Y` or `M`
        // here is the calendar-ambiguous case the language deliberately bans.
        let days = take_duration_component(date_part, 'D')?;
        magnitude = add_component(magnitude, days, NANOS_PER_DAY)?;
        any_component = true;
    }

    if let Some(time) = time_part {
        if time.is_empty() {
            return Err(DurationParseError::Malformed);
        }
        let (hours, rest) = split_optional_component(time, 'H')?;
        let (minutes, rest) = split_optional_component(rest, 'M')?;
        if let Some(hours) = hours {
            magnitude = add_component(magnitude, hours, NANOS_PER_HOUR)?;
            any_component = true;
        }
        if let Some(minutes) = minutes {
            magnitude = add_component(magnitude, minutes, NANOS_PER_MINUTE)?;
            any_component = true;
        }
        if !rest.is_empty() {
            let seconds_nanos = parse_seconds_component(rest)?;
            magnitude = magnitude
                .checked_add(seconds_nanos)
                .ok_or(DurationParseError::Malformed)?;
            any_component = true;
        }
    }

    if !any_component {
        return Err(DurationParseError::Malformed);
    }
    if negative && magnitude == 0 {
        // Negative zero has no canonical spelling; zero is `PT0S`.
        return Err(DurationParseError::Malformed);
    }
    Ok(if negative { -magnitude } else { magnitude })
}

/// Reads the whole date-position field, requiring it to be exactly `<digits>D`. A
/// year or month unit (`Y`, or an `M` before the `T`) is calendar-ambiguous and
/// rejected with that reason; anything else is malformed.
fn take_duration_component(field: &str, unit: char) -> Result<i128, DurationParseError> {
    if field.contains('Y') || field.contains('M') {
        return Err(DurationParseError::CalendarAmbiguous);
    }
    let digits = field
        .strip_suffix(unit)
        .ok_or(DurationParseError::Malformed)?;
    parse_unsigned_count(digits)
}

/// Pulls an optional leading `<digits><unit>` off the front of a time-position field,
/// returning the parsed count (if present) and the remaining text. A `Y` unit anywhere
/// in the time part is calendar-ambiguous.
fn split_optional_component(
    field: &str,
    unit: char,
) -> Result<(Option<i128>, &str), DurationParseError> {
    if field.contains('Y') {
        return Err(DurationParseError::CalendarAmbiguous);
    }
    match field.split_once(unit) {
        Some((digits, rest)) => Ok((Some(parse_unsigned_count(digits)?), rest)),
        None => Ok((None, field)),
    }
}

/// Parses the trailing seconds component `<digits>[.<fraction>]S` to nanoseconds.
fn parse_seconds_component(field: &str) -> Result<i128, DurationParseError> {
    let body = field
        .strip_suffix('S')
        .ok_or(DurationParseError::Malformed)?;
    let (seconds_text, fraction_text) = match body.split_once('.') {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (body, None),
    };
    let seconds = parse_unsigned_count(seconds_text)?;
    let fraction_nanos = match fraction_text {
        None => 0,
        Some(digits) => {
            parse_fraction_nanos(digits.as_bytes()).ok_or(DurationParseError::Malformed)?
        }
    };
    seconds
        .checked_mul(NANOS_PER_SEC)
        .and_then(|whole| whole.checked_add(fraction_nanos))
        .ok_or(DurationParseError::Malformed)
}

/// Parses an all-digit unsigned count with no leading zero, so a non-canonical
/// spelling like `01` is malformed. Empty input or any non-digit byte is malformed.
fn parse_unsigned_count(digits: &str) -> Result<i128, DurationParseError> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DurationParseError::Malformed);
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(DurationParseError::Malformed);
    }
    digits.parse().map_err(|_| DurationParseError::Malformed)
}

/// Multiplies a component count by its fixed nanosecond width and adds it to the
/// running magnitude under the `i128` overflow envelope.
fn add_component(magnitude: i128, count: i128, width: i128) -> Result<i128, DurationParseError> {
    count
        .checked_mul(width)
        .and_then(|nanos| magnitude.checked_add(nanos))
        .ok_or(DurationParseError::Malformed)
}

/// Parses a fractional-second field to nanoseconds. RFC-3339 secfrac is unbounded, so
/// cosmetic trailing zeros are trimmed before the nanosecond cap applies: only the
/// significant fraction must fit nine digits. Real sub-nanosecond precision — more than
/// nine significant digits — is refused, and the value normalizes away on output.
fn parse_fraction_nanos(digits: &[u8]) -> Option<i128> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let significant = std::str::from_utf8(digits).ok()?.trim_end_matches('0');
    if significant.len() > 9 {
        return None;
    }
    format!("{significant:0<9}").parse().ok()
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
