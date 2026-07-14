//! Proleptic-Gregorian calendar arithmetic shared by the date and instant codecs.
//!
//! Dates are held as days since the Unix epoch (1970-01-01) and instants as
//! signed nanoseconds since the epoch; both are restricted to the canonical
//! year range 0001-9999 that the byte codecs can read back exactly.

pub(crate) const NANOS_PER_SEC: i128 = 1_000_000_000;

/// Nanoseconds in a 24-hour day, the date<->nanos conversion factor. Owned here
/// alongside the date/instant codecs; the runtime imports it for the same model.
pub const NANOS_PER_DAY: i128 = 86_400 * NANOS_PER_SEC;

pub const SUPPORTED_DATE_MIN_DAYS: i32 = -719_162;
pub const SUPPORTED_DATE_MAX_DAYS: i32 = 2_932_896;

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
pub(crate) fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month_part = (if month > 2 { month - 3 } else { month + 9 }) as i64;
    let day_of_year = (153 * month_part + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// Hinnant's `civil_from_days`, the inverse of [`days_from_civil`].
pub(crate) fn civil_from_days(days: i64) -> (i32, u32, u32) {
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
