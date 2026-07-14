//! Saved values round-trip through their canonical byte form, and non-canonical
//! bytes are rejected.

use marrow_kernel::codec::civil::{
    SUPPORTED_DATE_MAX_DAYS, SUPPORTED_DATE_MIN_DAYS, SUPPORTED_INSTANT_MAX_NANOS,
    SUPPORTED_INSTANT_MIN_NANOS, date_days, supported_date_days, supported_instant_nanos,
};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{
    RuntimeScalar, ScalarKind, ValueError, decode_value, encode_value, scalar_key_matches_type,
    validate_scalar_key,
};

fn round_trips(value: RuntimeScalar, ty: ScalarKind) {
    let bytes = encode_value(&value).expect("in-range value encodes");
    assert_eq!(decode_value(&bytes, ty), Some(value), "round-trip failed");
}

#[test]
fn values_round_trip_through_canonical_bytes() {
    round_trips(RuntimeScalar::Bool(true), ScalarKind::Bool);
    round_trips(RuntimeScalar::Bool(false), ScalarKind::Bool);
    round_trips(RuntimeScalar::Int(0), ScalarKind::Int);
    round_trips(RuntimeScalar::Int(-42), ScalarKind::Int);
    round_trips(RuntimeScalar::Int(i64::MIN), ScalarKind::Int);
    round_trips(RuntimeScalar::Int(i64::MAX), ScalarKind::Int);
    round_trips(RuntimeScalar::Str("Dune".into()), ScalarKind::Str);
    round_trips(RuntimeScalar::Str(String::new()), ScalarKind::Str);
    round_trips(
        RuntimeScalar::Bytes(vec![0x00, 0xFF, 0x01]),
        ScalarKind::Bytes,
    );
}

/// Encode a value known to be in range, unwrapping the canonical bytes.
fn encoded(value: &RuntimeScalar) -> Vec<u8> {
    encode_value(value).expect("in-range value encodes")
}

#[test]
fn canonical_forms_match_the_docs() {
    assert_eq!(encoded(&RuntimeScalar::Bool(true)), b"1");
    assert_eq!(encoded(&RuntimeScalar::Bool(false)), b"0");
    assert_eq!(encoded(&RuntimeScalar::Int(42)), b"42");
    assert_eq!(encoded(&RuntimeScalar::Int(-5)), b"-5");
    assert_eq!(encoded(&RuntimeScalar::Str("hi".into())), b"hi");
}

#[test]
fn scalar_names_are_the_canonical_store_spelling() {
    for (scalar, name) in [
        (ScalarKind::Bool, "bool"),
        (ScalarKind::Int, "int"),
        (ScalarKind::Str, "string"),
        (ScalarKind::Bytes, "bytes"),
        (ScalarKind::Date, "date"),
        (ScalarKind::Instant, "instant"),
        (ScalarKind::Duration, "duration"),
    ] {
        assert_eq!(scalar.name(), name);
    }
}

#[test]
fn non_canonical_integers_are_rejected() {
    assert_eq!(decode_value(b"+1", ScalarKind::Int), None);
    assert_eq!(decode_value(b"01", ScalarKind::Int), None);
    assert_eq!(decode_value(b"-0", ScalarKind::Int), None);
    assert_eq!(decode_value(b" 1", ScalarKind::Int), None);
    assert_eq!(decode_value(b"x", ScalarKind::Int), None);
}

#[test]
fn non_canonical_booleans_are_rejected() {
    assert_eq!(decode_value(b"2", ScalarKind::Bool), None);
    assert_eq!(decode_value(b"true", ScalarKind::Bool), None);
    assert_eq!(decode_value(b"", ScalarKind::Bool), None);
}

#[test]
fn invalid_utf8_is_rejected_for_text_but_kept_for_bytes() {
    assert_eq!(decode_value(&[0xFF], ScalarKind::Str), None);
    assert_eq!(
        decode_value(&[0xFF], ScalarKind::Bytes),
        Some(RuntimeScalar::Bytes(vec![0xFF]))
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
        let value = RuntimeScalar::Date(date_days(year, month, day).expect("valid date"));
        let bytes = encoded(&value);
        assert_eq!(bytes, text.as_bytes(), "canonical form for {text}");
        assert_eq!(decode_value(&bytes, ScalarKind::Date), Some(value));
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
    assert_eq!(decode_value(b"2021-02-29", ScalarKind::Date), None);
    assert_eq!(decode_value(b"2021-2-3", ScalarKind::Date), None); // unpadded
    assert_eq!(decode_value(b"2021-13-01", ScalarKind::Date), None);
    assert_eq!(decode_value(b"2021/05/28", ScalarKind::Date), None); // wrong separator
    assert_eq!(decode_value(b"20210528", ScalarKind::Date), None);
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
        let value = RuntimeScalar::Duration(nanos);
        let bytes = encoded(&value);
        assert_eq!(bytes, text.as_bytes(), "canonical form for {nanos} ns");
        assert_eq!(decode_value(&bytes, ScalarKind::Duration), Some(value));
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
            decode_value(bad.as_bytes(), ScalarKind::Duration),
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
        let value = decode_value(text.as_bytes(), ScalarKind::Instant).expect("valid instant");
        assert_eq!(encoded(&value), text.as_bytes(), "re-encode {text}");
    }
}

#[test]
fn the_epoch_instant_is_zero_nanos() {
    assert_eq!(
        decode_value(b"1970-01-01T00:00:00Z", ScalarKind::Instant),
        Some(RuntimeScalar::Instant(0))
    );
    assert_eq!(encoded(&RuntimeScalar::Instant(0)), b"1970-01-01T00:00:00Z");
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
            decode_value(bad.as_bytes(), ScalarKind::Instant),
            None,
            "{bad}"
        );
    }
}

#[test]
fn date_encode_enforces_the_canonical_year_range() {
    // The documented range encodes and round-trips at both ends.
    for days in [
        date_days(1, 1, 1).expect("year 0001"),
        date_days(9999, 12, 31).expect("year 9999"),
    ] {
        let value = RuntimeScalar::Date(days);
        let bytes = encode_value(&value).expect("in-range date encodes");
        assert_eq!(decode_value(&bytes, ScalarKind::Date), Some(value));
    }
    // A day just outside the range, and the i32 extremes, are a typed range error
    // rather than a 5-7 digit year that decode could never read back.
    for days in [
        SUPPORTED_DATE_MIN_DAYS - 1,
        SUPPORTED_DATE_MAX_DAYS + 1,
        i32::MIN,
        i32::MAX,
    ] {
        assert_eq!(
            encode_value(&RuntimeScalar::Date(days)),
            Err(ValueError::DateOutOfRange { days }),
            "out-of-range day {days}"
        );
    }
}

#[test]
fn instant_encode_enforces_the_canonical_year_range() {
    // The documented range encodes and round-trips at both ends.
    for text in ["0001-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z"] {
        let value = decode_value(text.as_bytes(), ScalarKind::Instant).expect("valid instant");
        assert_eq!(encoded(&value), text.as_bytes());
    }
    // An instant whose calendar day falls outside year 0001-9999 is a typed range
    // error, matching the date boundary; i128 extremes are rejected too.
    for nanos in [
        SUPPORTED_INSTANT_MIN_NANOS - 1,
        SUPPORTED_INSTANT_MAX_NANOS + 1,
        i128::MIN,
        i128::MAX,
    ] {
        assert!(
            matches!(
                encode_value(&RuntimeScalar::Instant(nanos)),
                Err(ValueError::InstantOutOfRange { .. })
            ),
            "out-of-range instant {nanos} must be a typed error"
        );
    }
}

#[test]
fn temporal_codec_constants_match_supported_boundaries() {
    assert_eq!(SUPPORTED_DATE_MIN_DAYS, date_days(1, 1, 1).unwrap());
    assert_eq!(SUPPORTED_DATE_MAX_DAYS, date_days(9999, 12, 31).unwrap());
    assert!(supported_date_days(SUPPORTED_DATE_MIN_DAYS));
    assert!(supported_date_days(SUPPORTED_DATE_MAX_DAYS));
    assert!(!supported_date_days(SUPPORTED_DATE_MIN_DAYS - 1));
    assert!(!supported_date_days(SUPPORTED_DATE_MAX_DAYS + 1));

    assert_eq!(
        decode_value(b"0001-01-01T00:00:00Z", ScalarKind::Instant),
        Some(RuntimeScalar::Instant(SUPPORTED_INSTANT_MIN_NANOS))
    );
    assert_eq!(
        decode_value(b"9999-12-31T23:59:59.999999999Z", ScalarKind::Instant),
        Some(RuntimeScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS))
    );
    assert!(supported_instant_nanos(SUPPORTED_INSTANT_MIN_NANOS));
    assert!(supported_instant_nanos(SUPPORTED_INSTANT_MAX_NANOS));
    assert!(!supported_instant_nanos(SUPPORTED_INSTANT_MIN_NANOS - 1));
    assert!(!supported_instant_nanos(SUPPORTED_INSTANT_MAX_NANOS + 1));
}

#[test]
fn scalar_key_validation_rejects_temporal_key_neighbors() {
    for key in [
        KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS),
        KeyScalar::Date(SUPPORTED_DATE_MAX_DAYS),
        KeyScalar::Instant(SUPPORTED_INSTANT_MIN_NANOS),
        KeyScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS),
        KeyScalar::Duration(i128::MIN),
        KeyScalar::Duration(i128::MAX),
    ] {
        validate_scalar_key(&key).expect("supported scalar key");
    }

    assert_eq!(
        validate_scalar_key(&KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS - 1)),
        Err(ValueError::DateOutOfRange {
            days: SUPPORTED_DATE_MIN_DAYS - 1
        })
    );
    assert_eq!(
        validate_scalar_key(&KeyScalar::Date(SUPPORTED_DATE_MAX_DAYS + 1)),
        Err(ValueError::DateOutOfRange {
            days: SUPPORTED_DATE_MAX_DAYS + 1
        })
    );
    assert_eq!(
        validate_scalar_key(&KeyScalar::Instant(SUPPORTED_INSTANT_MIN_NANOS - 1)),
        Err(ValueError::InstantOutOfRange {
            nanos: SUPPORTED_INSTANT_MIN_NANOS - 1
        })
    );
    assert_eq!(
        validate_scalar_key(&KeyScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS + 1)),
        Err(ValueError::InstantOutOfRange {
            nanos: SUPPORTED_INSTANT_MAX_NANOS + 1
        })
    );
}

#[test]
fn scalar_key_type_match_validates_temporal_ranges() {
    assert!(scalar_key_matches_type(
        &KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS),
        ScalarKind::Date
    ));
    assert!(!scalar_key_matches_type(
        &KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS - 1),
        ScalarKind::Date
    ));
    assert!(!scalar_key_matches_type(
        &KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS),
        ScalarKind::Int
    ));
    assert!(scalar_key_matches_type(
        &KeyScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS),
        ScalarKind::Instant
    ));
    assert!(!scalar_key_matches_type(
        &KeyScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS + 1),
        ScalarKind::Instant
    ));
}

#[test]
fn scalar_key_projection_validates_temporal_ranges() {
    assert_eq!(
        RuntimeScalar::Date(SUPPORTED_DATE_MIN_DAYS).as_key(),
        Ok(Some(KeyScalar::Date(SUPPORTED_DATE_MIN_DAYS)))
    );
    assert_eq!(
        RuntimeScalar::Date(SUPPORTED_DATE_MIN_DAYS - 1).as_key(),
        Err(ValueError::DateOutOfRange {
            days: SUPPORTED_DATE_MIN_DAYS - 1
        })
    );
    assert_eq!(
        RuntimeScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS).as_key(),
        Ok(Some(KeyScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS)))
    );
    assert_eq!(
        RuntimeScalar::Instant(SUPPORTED_INSTANT_MAX_NANOS + 1).as_key(),
        Err(ValueError::InstantOutOfRange {
            nanos: SUPPORTED_INSTANT_MAX_NANOS + 1
        })
    );
}
