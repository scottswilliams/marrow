//! Saved values round-trip through their canonical byte form, and non-canonical
//! bytes are rejected.

use marrow_store::value::{SavedValue, ValueError, ValueType, date_days, decode_value, encode_value};

fn round_trips(value: SavedValue, ty: ValueType) {
    let bytes = encode_value(&value).expect("in-range value encodes");
    assert_eq!(decode_value(&bytes, ty), Some(value), "round-trip failed");
}

#[test]
fn values_round_trip_through_canonical_bytes() {
    round_trips(SavedValue::Bool(true), ValueType::Bool);
    round_trips(SavedValue::Bool(false), ValueType::Bool);
    round_trips(SavedValue::Int(0), ValueType::Int);
    round_trips(SavedValue::Int(-42), ValueType::Int);
    round_trips(SavedValue::Int(i64::MIN), ValueType::Int);
    round_trips(SavedValue::Int(i64::MAX), ValueType::Int);
    round_trips(SavedValue::Str("Dune".into()), ValueType::Str);
    round_trips(SavedValue::Str(String::new()), ValueType::Str);
    round_trips(
        SavedValue::ErrorCode("store.limit_exceeded".into()),
        ValueType::ErrorCode,
    );
    round_trips(SavedValue::Bytes(vec![0x00, 0xFF, 0x01]), ValueType::Bytes);
}

/// Encode a value known to be in range, unwrapping the canonical bytes.
fn encoded(value: &SavedValue) -> Vec<u8> {
    encode_value(value).expect("in-range value encodes")
}

#[test]
fn canonical_forms_match_the_docs() {
    assert_eq!(encoded(&SavedValue::Bool(true)), b"1");
    assert_eq!(encoded(&SavedValue::Bool(false)), b"0");
    assert_eq!(encoded(&SavedValue::Int(42)), b"42");
    assert_eq!(encoded(&SavedValue::Int(-5)), b"-5");
    assert_eq!(encoded(&SavedValue::Str("hi".into())), b"hi");
}

#[test]
fn non_canonical_integers_are_rejected() {
    assert_eq!(decode_value(b"+1", ValueType::Int), None);
    assert_eq!(decode_value(b"01", ValueType::Int), None);
    assert_eq!(decode_value(b"-0", ValueType::Int), None);
    assert_eq!(decode_value(b" 1", ValueType::Int), None);
    assert_eq!(decode_value(b"x", ValueType::Int), None);
}

#[test]
fn non_canonical_booleans_are_rejected() {
    assert_eq!(decode_value(b"2", ValueType::Bool), None);
    assert_eq!(decode_value(b"true", ValueType::Bool), None);
    assert_eq!(decode_value(b"", ValueType::Bool), None);
}

#[test]
fn invalid_utf8_is_rejected_for_text_but_kept_for_bytes() {
    assert_eq!(decode_value(&[0xFF], ValueType::Str), None);
    assert_eq!(
        decode_value(&[0xFF], ValueType::Bytes),
        Some(SavedValue::Bytes(vec![0xFF]))
    );
}

#[test]
fn dates_round_trip_through_canonical_text() {
    for (year, month, day, text) in [
        (1970, 1, 1, "1970-01-01"),
        (2026, 5, 28, "2026-05-28"),
        (2000, 2, 29, "2000-02-29"), // a leap day
        (1, 1, 1, "0001-01-01"),
        (9999, 12, 31, "9999-12-31"),
        (1969, 12, 31, "1969-12-31"), // pre-epoch
    ] {
        let value = SavedValue::Date(date_days(year, month, day).expect("valid date"));
        let bytes = encoded(&value);
        assert_eq!(bytes, text.as_bytes(), "canonical form for {text}");
        assert_eq!(decode_value(&bytes, ValueType::Date), Some(value));
    }
}

#[test]
fn the_epoch_is_day_zero() {
    assert_eq!(date_days(1970, 1, 1), Some(0));
}

#[test]
fn impossible_and_non_canonical_dates_are_rejected() {
    // Impossible calendar dates.
    assert_eq!(date_days(2021, 2, 29), None); // 2021 is not a leap year
    assert_eq!(date_days(2021, 13, 1), None);
    assert_eq!(date_days(2021, 0, 1), None);
    assert_eq!(date_days(0, 1, 1), None); // year below 0001
    // Non-canonical text forms.
    assert_eq!(decode_value(b"2021-02-29", ValueType::Date), None);
    assert_eq!(decode_value(b"2021-2-3", ValueType::Date), None); // unpadded
    assert_eq!(decode_value(b"2021-13-01", ValueType::Date), None);
    assert_eq!(decode_value(b"2021/05/28", ValueType::Date), None); // wrong separator
    assert_eq!(decode_value(b"20210528", ValueType::Date), None);
}

#[test]
fn durations_round_trip_through_canonical_text() {
    for (nanos, text) in [
        (0i128, "PT0S"),
        (1_500_000_000, "PT1.5S"),
        (-250_000_000, "-PT0.25S"),
        (90_061_000_000_000, "PT90061S"),
        (1, "PT0.000000001S"), // one nanosecond
        (-1, "-PT0.000000001S"),
        (-1_000_000_000, "-PT1S"),
    ] {
        let value = SavedValue::Duration(nanos);
        let bytes = encoded(&value);
        assert_eq!(bytes, text.as_bytes(), "canonical form for {nanos} ns");
        assert_eq!(decode_value(&bytes, ValueType::Duration), Some(value));
    }
}

#[test]
fn non_canonical_durations_are_rejected() {
    for bad in [
        "-PT0S",    // negative zero
        "PT00S",    // leading zero
        "PT0.0S",   // trailing-zero fraction
        "PT1.250S", // trailing-zero fraction
        "PT.5S",    // missing seconds
        "PT1.5",    // missing S
        "P1S",      // missing T
        "PT1.5s",   // lowercase unit
    ] {
        assert_eq!(
            decode_value(bad.as_bytes(), ValueType::Duration),
            None,
            "{bad}"
        );
    }
}

#[test]
fn instants_round_trip_through_canonical_text() {
    for text in [
        "1970-01-01T00:00:00Z",
        "2026-05-28T12:30:45Z",
        "2026-05-28T12:30:45.5Z",
        "2026-05-28T12:30:45.000000001Z", // one nanosecond
        "1969-12-31T23:59:59Z",           // pre-epoch
        "0001-01-01T00:00:00Z",
        "9999-12-31T23:59:59.999999999Z",
    ] {
        let value = decode_value(text.as_bytes(), ValueType::Instant).expect("valid instant");
        assert_eq!(encoded(&value), text.as_bytes(), "re-encode {text}");
    }
}

#[test]
fn the_epoch_instant_is_zero_nanos() {
    assert_eq!(
        decode_value(b"1970-01-01T00:00:00Z", ValueType::Instant),
        Some(SavedValue::Instant(0))
    );
    assert_eq!(encoded(&SavedValue::Instant(0)), b"1970-01-01T00:00:00Z");
}

#[test]
fn non_canonical_instants_are_rejected() {
    for bad in [
        "2026-05-28t12:30:45Z",      // lowercase T
        "2026-05-28T12:30:45z",      // lowercase Z
        "2026-05-28T12:30:45",       // missing Z
        "2026-05-28T12:30:45+00:00", // numeric offset, not Z
        "2026-05-28T12:30:60Z",      // seconds out of range
        "2026-05-28T24:00:00Z",      // hour out of range
        "2026-05-28T12:30:45.0Z",    // trailing-zero fraction
        "2026-05-28T12:30:45.Z",     // empty fraction
        "2026-02-29T00:00:00Z",      // impossible date
    ] {
        assert_eq!(
            decode_value(bad.as_bytes(), ValueType::Instant),
            None,
            "{bad}"
        );
    }
}

#[test]
fn decimals_round_trip_through_canonical_text() {
    for text in [
        "0", "5", "-5", "0.25", "1.5", "-1.5", "123.45", "0.025", "-0.5",
    ] {
        let value = decode_value(text.as_bytes(), ValueType::Decimal).expect("valid decimal");
        assert_eq!(encoded(&value), text.as_bytes(), "re-encode {text}");
    }
}

#[test]
fn decimal_encoding_is_value_canonical() {
    let enc = |coefficient, scale| encoded(&SavedValue::Decimal { coefficient, scale });
    // Trailing-zero scale is normalized away to one spelling per value.
    assert_eq!(enc(15, 1), b"1.5");
    assert_eq!(enc(150, 2), b"1.5");
    assert_eq!(enc(0, 0), b"0");
    assert_eq!(enc(0, 5), b"0");
    assert_eq!(enc(-25, 2), b"-0.25");
}

#[test]
fn non_canonical_decimals_are_rejected() {
    for bad in [
        "1.0", "1.50", "01", "-0", ".5", "1.", "+1", "1e3", "00", "0.0",
    ] {
        assert_eq!(
            decode_value(bad.as_bytes(), ValueType::Decimal),
            None,
            "{bad}"
        );
    }
}

#[test]
fn decimals_outside_the_envelope_are_rejected() {
    // 35 significant digits exceeds the 34-digit envelope.
    assert_eq!(
        decode_value("1".repeat(35).as_bytes(), ValueType::Decimal),
        None
    );
    // 35 fractional places exceeds the 34-place scale.
    let too_deep = format!("0.{}", "1".repeat(35));
    assert_eq!(decode_value(too_deep.as_bytes(), ValueType::Decimal), None);
}

// One day before year 0001-01-01 and one day after year 9999-12-31: the canonical
// range's outer neighbors, used to prove encode rejects exactly what decode does.
const DAY_BEFORE_YEAR_ONE: i32 = -719163; // 0000-12-31
const DAY_AFTER_YEAR_9999: i32 = 2932897; // 10000-01-01

#[test]
fn date_encode_enforces_the_canonical_year_range() {
    // The documented range encodes and round-trips at both ends.
    for days in [
        date_days(1, 1, 1).expect("year 0001"),
        date_days(9999, 12, 31).expect("year 9999"),
    ] {
        let value = SavedValue::Date(days);
        let bytes = encode_value(&value).expect("in-range date encodes");
        assert_eq!(decode_value(&bytes, ValueType::Date), Some(value));
    }
    // A day just outside the range, and the i32 extremes, are a typed range error
    // rather than a 5-7 digit year that decode could never read back.
    for days in [DAY_BEFORE_YEAR_ONE, DAY_AFTER_YEAR_9999, i32::MIN, i32::MAX] {
        assert_eq!(
            encode_value(&SavedValue::Date(days)),
            Err(ValueError::DateOutOfRange { days }),
            "out-of-range day {days}"
        );
    }
}

#[test]
fn instant_encode_enforces_the_canonical_year_range() {
    const NANOS_PER_DAY: i128 = 86_400 * 1_000_000_000;
    // The documented range encodes and round-trips at both ends.
    for text in ["0001-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z"] {
        let value = decode_value(text.as_bytes(), ValueType::Instant).expect("valid instant");
        assert_eq!(encoded(&value), text.as_bytes());
    }
    // An instant whose calendar day falls outside year 0001-9999 is a typed range
    // error, matching the date boundary; i128 extremes are rejected too.
    for nanos in [
        i128::from(DAY_BEFORE_YEAR_ONE) * NANOS_PER_DAY,
        i128::from(DAY_AFTER_YEAR_9999) * NANOS_PER_DAY,
        i128::MIN,
        i128::MAX,
    ] {
        assert!(
            matches!(
                encode_value(&SavedValue::Instant(nanos)),
                Err(ValueError::InstantOutOfRange { .. })
            ),
            "out-of-range instant {nanos} must be a typed error"
        );
    }
}
