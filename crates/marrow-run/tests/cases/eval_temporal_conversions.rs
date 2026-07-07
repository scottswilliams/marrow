//! Temporal builtins (instants, dates, durations, the clock capability) and the
//! scalar/temporal conversion builtins.

use crate::support;
use support::*;

use marrow_run::{
    Host, RUN_CAPABILITY, RUN_DECIMAL_OVERFLOW, RUN_TEMPORAL_OVERFLOW, RUN_TYPE, Value,
};
use marrow_store::Decimal;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{
    SUPPORTED_DATE_MAX_DAYS, SUPPORTED_DATE_MIN_DAYS, SUPPORTED_INSTANT_MAX_NANOS,
    SUPPORTED_INSTANT_MIN_NANOS,
};

const VALUE_RANGE: &str = "value.range";

#[test]
fn formats_and_parses_instants() {
    // An instant round-trips through its canonical UTC text.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2026-05-28T12:00:00Z".into()))
    );
}

#[test]
fn parse_instant_rejects_invalid_text() {
    let program = checked_program(
        "pub fn f(): instant\n    return std::clock::parseInstant(\"not a time\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap_err().code(),
        RUN_TYPE
    );
}

#[test]
fn instant_input_accepts_standard_rfc3339_and_normalizes_to_canonical() {
    // The instant parsers accept standard RFC-3339 input — trailing-zero fractional
    // seconds and an explicit numeric offset — and normalize to the canonical UTC
    // `...Z` output form. Both the `std::clock::parseInstant` and `instant(...)`
    // entry points share this acceptance.
    let program = checked_program(
        "pub fn parsed(t: string): string\n    return std::clock::formatInstant(std::clock::parseInstant(t))\n\
         pub fn built(t: string): string\n    return std::clock::formatInstant(instant(t))\n",
    );
    let cases: &[(&str, &str)] = &[
        // Trailing-zero fractions trim to the canonical form.
        ("2024-01-01T00:00:00.500Z", "2024-01-01T00:00:00.5Z"),
        ("2024-01-01T00:00:00.000Z", "2024-01-01T00:00:00Z"),
        ("2024-01-01T00:00:00.100000000Z", "2024-01-01T00:00:00.1Z"),
        // Cosmetic zero-padding past nanosecond width is significant-nanosecond-exact
        // and normalizes to the same canonical value as the 9-digit form.
        ("2026-06-20T12:00:00.5000000000Z", "2026-06-20T12:00:00.5Z"),
        ("2024-01-01T00:00:00.0000000000Z", "2024-01-01T00:00:00Z"),
        // An explicit UTC offset is equivalent to `Z`.
        ("2024-01-01T00:00:00+00:00", "2024-01-01T00:00:00Z"),
        ("2024-01-01T00:00:00-00:00", "2024-01-01T00:00:00Z"),
        // A non-UTC offset normalizes to the equivalent UTC instant.
        ("2024-01-01T05:00:00+05:00", "2024-01-01T00:00:00Z"),
        ("2024-01-01T00:00:00-05:00", "2024-01-01T05:00:00Z"),
        // The canonical form itself still round-trips.
        ("2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z"),
        ("2024-01-01T00:00:00.5Z", "2024-01-01T00:00:00.5Z"),
    ];
    for (input, expected) in cases {
        for entry in ["test::parsed", "test::built"] {
            assert_eq!(
                run(checked_entry!(&program, entry, Value::Str((*input).into()))),
                Ok(Some(Value::Str((*expected).into()))),
                "{entry}({input})"
            );
        }
    }
}

#[test]
fn duration_input_accepts_iso8601_time_subset_and_normalizes() {
    // Durations accept the time-based ISO-8601 subset `PnDTnHnMnS` where every
    // component is an exact fixed span (1D=86400s, 1H=3600s, 1M=60s) summed and
    // normalized to canonical `PT<seconds>S`. Trailing-zero fractional seconds and a
    // canonical input both round-trip. Both `parseDuration` and `duration(...)` share
    // this acceptance.
    let program = checked_program(
        "pub fn parsed(t: string): string\n    return std::clock::formatDuration(std::clock::parseDuration(t))\n\
         pub fn built(t: string): string\n    return std::clock::formatDuration(duration(t))\n",
    );
    let cases: &[(&str, &str)] = &[
        // Multi-component time spans sum to canonical seconds.
        ("PT1H30M", "PT5400S"),
        ("PT1H", "PT3600S"),
        ("PT1M", "PT60S"),
        ("P1D", "PT86400S"),
        ("P1DT2H3M4S", "PT93784S"),
        ("-PT1H30M", "-PT5400S"),
        // Fractions on the seconds component are preserved and trimmed to canonical.
        ("PT1.5S", "PT1.5S"),
        ("PT1.500S", "PT1.5S"),
        ("PT1.000S", "PT1S"),
        ("PT0.100000000S", "PT0.1S"),
        // Cosmetic zero-padding past nanosecond width stays significant-exact.
        ("PT1.5000000000S", "PT1.5S"),
        ("PT1H0.5S", "PT3600.5S"),
        // The canonical store spelling still parses unchanged.
        ("PT90S", "PT90S"),
        ("-PT1.500S", "-PT1.5S"),
    ];
    for (input, expected) in cases {
        for entry in ["test::parsed", "test::built"] {
            assert_eq!(
                run(checked_entry!(&program, entry, Value::Str((*input).into()))),
                Ok(Some(Value::Str((*expected).into()))),
                "{entry}({input})"
            );
        }
    }
}

#[test]
fn duration_input_rejects_nominal_calendar_components_with_a_clear_message() {
    // Year and month components are calendar-ambiguous: Marrow durations are exact
    // signed nanosecond spans with no DST/calendar arithmetic, so they reject with a
    // message that points to the exact units. The date-position `M` (months) is
    // distinguished from the time-position `M` (minutes) by the `T` separator. Both
    // entry points carry the clarifying note.
    let parse =
        checked_program("pub fn f(t: string): duration\n    return std::clock::parseDuration(t)\n");
    let convert = checked_program("pub fn f(t: string): duration\n    return duration(t)\n");
    for input in ["P1Y", "P1M", "P1MT", "P1Y2M", "P1YT1H"] {
        for program in [&parse, &convert] {
            let error =
                run(checked_entry!(program, "test::f", Value::Str(input.into()))).unwrap_err();
            assert_eq!(error.code(), RUN_TYPE, "{input}");
            assert!(
                error.message.contains("calendar-ambiguous")
                    && error.message.contains("days/hours/minutes/seconds"),
                "{input}: {}",
                error.message
            );
        }
    }
}

#[test]
fn duration_input_still_rejects_malformed_forms() {
    // Widening covers only the exact time-based subset: leading-zero seconds, `-PT0S`,
    // a missing `S`, a bare `P`, and garbage remain catchable type errors.
    let program =
        checked_program("pub fn f(t: string): duration\n    return std::clock::parseDuration(t)\n");
    for input in [
        "PT01S",
        "-PT0S",
        "PT1",
        "1.5S",
        "PTS",
        "P",
        "PT",
        "nonsense",
        // Real sub-nanosecond fractional precision exceeds the nanosecond cap.
        "PT1.1234567891S",
    ] {
        assert_eq!(
            run(checked_entry!(
                &program,
                "test::f",
                Value::Str(input.into())
            ))
            .unwrap_err()
            .code(),
            RUN_TYPE,
            "{input}"
        );
    }
}

#[test]
fn instant_input_still_rejects_malformed_text() {
    // Widening input acceptance must not swallow genuinely malformed text: a bad
    // offset, a lowercase zone marker, or a missing zone remain catchable type errors.
    let program =
        checked_program("pub fn f(t: string): instant\n    return std::clock::parseInstant(t)\n");
    for input in [
        "2024-01-01T00:00:00",       // no zone designator
        "2024-01-01T00:00:00z",      // lowercase zone marker
        "2024-01-01T00:00:00+5",     // malformed offset
        "2024-01-01T00:00:00.Z",     // empty fraction
        "2024-01-01T00:00:00+24:00", // offset hour out of range
        // Real sub-nanosecond precision (10 significant fractional digits) exceeds the
        // nanosecond cap even after trailing zeros are trimmed.
        "2024-01-01T00:00:00.1234567891Z",
    ] {
        assert_eq!(
            run(checked_entry!(
                &program,
                "test::f",
                Value::Str(input.into())
            ))
            .unwrap_err()
            .code(),
            RUN_TYPE,
            "{input}"
        );
    }
}

#[test]
fn formats_and_parses_dates() {
    // A date round-trips through its canonical YYYY-MM-DD text (leap day).
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDate(std::clock::parseDate(\"2024-02-29\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2024-02-29".into()))
    );
}

#[test]
fn date_calendar_helpers_cover_leap_day_and_year_boundary() {
    let program = checked_program(
        "pub fn leap(): string\n\
         \x20   return std::clock::formatDate(std::clock::addDays(std::clock::parseDate(\"2024-02-28\"), 1))\n\
         pub fn yearBoundary(): string\n\
         \x20   return std::clock::formatDate(std::clock::addDays(std::clock::parseDate(\"2024-12-31\"), 1))\n\
         pub fn daysForward(): int\n\
         \x20   return std::clock::daysBetween(std::clock::parseDate(\"2024-02-28\"), std::clock::parseDate(\"2024-03-01\"))\n\
         pub fn daysReverse(): int\n\
         \x20   return std::clock::daysBetween(std::clock::parseDate(\"2024-03-01\"), std::clock::parseDate(\"2024-02-28\"))\n\
         pub fn yearPart(): int\n\
         \x20   return std::clock::year(std::clock::parseDate(\"2024-02-29\"))\n\
         pub fn monthPart(): int\n\
         \x20   return std::clock::month(std::clock::parseDate(\"2024-02-29\"))\n\
         pub fn dayPart(): int\n\
         \x20   return std::clock::day(std::clock::parseDate(\"2024-02-29\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::leap")),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::yearBoundary")),
        Ok(Some(Value::Str("2025-01-01".into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::daysForward")),
        Ok(Some(Value::Int(2)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::daysReverse")),
        Ok(Some(Value::Int(-2)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::yearPart")),
        Ok(Some(Value::Int(2024)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::monthPart")),
        Ok(Some(Value::Int(2)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::dayPart")),
        Ok(Some(Value::Int(29)))
    );
}

#[test]
fn date_calendar_addition_overflow_is_catchable() {
    let program = checked_program(
        "pub fn upper(): string\n\
         \x20   try\n\
         \x20       var d: date = std::clock::addDays(std::clock::parseDate(\"9999-12-31\"), 1)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         pub fn lower(): string\n\
         \x20   try\n\
         \x20       var d: date = std::clock::addDays(std::clock::parseDate(\"0001-01-01\"), -1)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         pub fn normalizeInjectedDate(d: date, days: int): string\n\
         \x20   try\n\
         \x20       var shifted: date = std::clock::addDays(d, days)\n\
         \x20       return std::clock::formatDate(shifted)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::upper")),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lower")),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::normalizeInjectedDate",
            Value::Date(i32::MIN),
            Value::Int(2_147_483_648)
        )),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
}

#[test]
fn formats_and_parses_durations() {
    // A duration round-trips through its canonical PT<seconds>S text.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDuration(std::clock::parseDuration(\"PT90S\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("PT90S".into()))
    );
}

#[test]
fn duration_literals_evaluate_to_their_fixed_spans() {
    // Each unit literal evaluates to the same duration value as parsing its
    // canonical PT<seconds>S text.
    let cases = [
        ("1.second", "PT1S"),
        ("1.minute", "PT60S"),
        ("1.hour", "PT3600S"),
        ("1.day", "PT86400S"),
        ("1.week", "PT604800S"),
        ("2.days", "PT172800S"),
        ("3.hours", "PT10800S"),
    ];
    for (literal, canonical) in cases {
        let program = checked_program(&format!("pub fn f(): duration\n    return {literal}\n"));
        let reference = checked_program(&format!(
            "pub fn f(): duration\n    return std::clock::parseDuration(\"{canonical}\")\n"
        ));
        assert_eq!(
            run(checked_entry!(&program, "test::f")).unwrap(),
            run(checked_entry!(&reference, "test::f")).unwrap(),
            "{literal} should equal duration(\"{canonical}\")"
        );
    }
}

#[test]
fn duration_literal_is_usable_where_a_duration_value_is() {
    // A duration literal flows into instant/duration arithmetic like any duration.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatInstant(std::clock::parseInstant(\"2026-05-28T12:00:00Z\") + 1.hour)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("2026-05-28T13:00:00Z".into()))
    );
}

#[test]
fn temporal_operators_cover_linear_instant_and_duration_math() {
    let program = checked_program(
        "pub fn forward(): string\n\
         \x20   return std::clock::formatInstant(std::clock::parseInstant(\"2026-05-28T12:00:00Z\") + std::clock::parseDuration(\"PT3600S\"))\n\
         pub fn backward(): string\n\
         \x20   return std::clock::formatInstant(std::clock::parseInstant(\"2026-05-28T12:00:00Z\") - 30.minutes)\n\
         pub fn elapsed(): string\n\
         \x20   return std::clock::formatDuration(std::clock::parseInstant(\"2026-05-28T13:00:00Z\") - std::clock::parseInstant(\"2026-05-28T12:00:00Z\"))\n\
         pub fn duration_math(): string\n\
         \x20   return std::clock::formatDuration(1.hour + 30.minutes - 15.minutes)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::forward")).unwrap(),
        Some(Value::Str("2026-05-28T13:00:00Z".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::backward")).unwrap(),
        Some(Value::Str("2026-05-28T11:30:00Z".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::elapsed")).unwrap(),
        Some(Value::Str("PT3600S".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::duration_math")).unwrap(),
        Some(Value::Str("PT4500S".into()))
    );
}

#[test]
fn temporal_operator_overflow_is_catchable() {
    let program = checked_program(
        "pub fn instant_code(): string\n\
         \x20   try\n\
         \x20       var text: string = std::clock::formatInstant(std::clock::parseInstant(\"9999-12-31T23:59:59Z\") + 1.day)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn duration_code(a: duration, b: duration): string\n\
         \x20   try\n\
         \x20       var d: duration = a + b\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::instant_code")),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::duration_code",
            Value::Duration(i128::MAX),
            Value::Duration(1)
        )),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
}

#[test]
fn instant_min_max_arithmetic_stays_inside_the_saved_range() {
    let program = checked_program(
        "pub fn shift(t: instant, d: duration): string\n\
         \x20   try\n\
         \x20       return std::clock::formatInstant(t + d)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::shift",
            Value::Instant(SUPPORTED_INSTANT_MIN_NANOS),
            Value::Duration(0)
        )),
        Ok(Some(Value::Str("0001-01-01T00:00:00Z".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::shift",
            Value::Instant(SUPPORTED_INSTANT_MIN_NANOS),
            Value::Duration(-1)
        )),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::shift",
            Value::Instant(SUPPORTED_INSTANT_MAX_NANOS),
            Value::Duration(0)
        )),
        Ok(Some(Value::Str("9999-12-31T23:59:59.999999999Z".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::shift",
            Value::Instant(SUPPORTED_INSTANT_MAX_NANOS),
            Value::Duration(1)
        )),
        Ok(Some(Value::Str(RUN_TEMPORAL_OVERFLOW.into())))
    );
}

#[test]
fn host_clock_rejects_instants_outside_the_saved_range() {
    let program = checked_program("pub fn now(): instant\n    return std::clock::now()\n");
    let store = TreeStore::memory();

    assert_eq!(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MIN_NANOS),
            checked_entry!(&program, "test::now")
        )
        .unwrap()
        .value,
        Some(Value::Instant(SUPPORTED_INSTANT_MIN_NANOS))
    );
    assert_run_error(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MIN_NANOS - 1),
            checked_entry!(&program, "test::now"),
        ),
        RUN_TEMPORAL_OVERFLOW,
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MAX_NANOS),
            checked_entry!(&program, "test::now")
        )
        .unwrap()
        .value,
        Some(Value::Instant(SUPPORTED_INSTANT_MAX_NANOS))
    );
    assert_run_error(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MAX_NANOS + 1),
            checked_entry!(&program, "test::now"),
        ),
        RUN_TEMPORAL_OVERFLOW,
    );
}

#[test]
fn host_clock_rejects_today_outside_the_saved_date_range() {
    let program = checked_program("pub fn today(): date\n    return std::clock::today()\n");
    let store = TreeStore::memory();

    assert_eq!(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MIN_NANOS),
            checked_entry!(&program, "test::today")
        )
        .unwrap()
        .value,
        Some(Value::Date(SUPPORTED_DATE_MIN_DAYS))
    );
    assert_run_error(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MIN_NANOS - 1),
            checked_entry!(&program, "test::today"),
        ),
        RUN_TEMPORAL_OVERFLOW,
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MAX_NANOS),
            checked_entry!(&program, "test::today")
        )
        .unwrap()
        .value,
        Some(Value::Date(SUPPORTED_DATE_MAX_DAYS))
    );
    assert_run_error(
        run_entry_with_host(
            &store,
            &Host::new().with_clock(SUPPORTED_INSTANT_MAX_NANOS + 1),
            checked_entry!(&program, "test::today"),
        ),
        RUN_TEMPORAL_OVERFLOW,
    );
}

#[test]
fn clock_today_reads_the_host_clock_capability() {
    // `today()` is the host clock's UTC calendar date.
    let program = checked_program(
        "pub fn f(): string\n    return std::clock::formatDate(std::clock::today())\n",
    );
    let store = TreeStore::memory();
    // 2023-11-14T22:13:20Z.
    let host = Host::new().with_clock(1_700_000_000_000_000_000);
    let outcome =
        run_entry_with_host(&store, &host, checked_entry!(&program, "test::f")).expect("today");
    assert_eq!(outcome.value, Some(Value::Str("2023-11-14".into())));
}

#[test]
fn clock_today_without_a_clock_capability_is_a_capability_error() {
    let program = checked_program("pub fn t(): date\n    return std::clock::today()\n");
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::t"));
    assert_run_error(result, RUN_CAPABILITY);
}

#[test]
fn a_date_round_trips_through_saved_data() {
    // A `date` value saves and loads through a managed field write and read.
    let program = checked_program(
        "resource Event\n    on: date\nstore ^events(id: int): Event\n\npub fn record(id: int, text: string)\n    ^events(id).on = std::clock::parseDate(text)\n\npub fn dateOf(id: int): string\n    return std::clock::formatDate(^events(id).on ?? std::clock::parseDate(\"1970-01-01\"))\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::record",
            Value::Int(1),
            Value::Str("2024-02-29".into())
        ),
    )
    .expect("record");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::dateOf", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(outcome.value, Some(Value::Str("2024-02-29".into())));
}

const DATE_KEYED_EVENTS: &str = "\
resource Event
    title: string
    notes(day: date): string
store ^events(day: date): Event

pub fn rootCount(): int
    return count(^events)

pub fn rootExists(): bool
    return exists(^events)

pub fn rootKeys(): int
    var n = 0
    for event in ^events
        n = n + 1
    return n

pub fn rootNext(): Id(^events)
    return next(^events) ?? Id(^events, std::clock::parseDate(\"1970-01-01\"))

pub fn rootPrev(): Id(^events)
    return prev(^events) ?? Id(^events, std::clock::parseDate(\"1970-01-01\"))

pub fn noteCount(day: date): int
    return count(^events(day).notes)

pub fn noteExists(day: date): bool
    return exists(^events(day).notes)

pub fn noteKeys(day: date): int
    var n = 0
    for note_day in ^events(day).notes
        n = n + 1
    return n

pub fn noteValues(day: date): int
    var n = 0
    for k, note in ^events(day).notes
        n = n + 1
    return n

pub fn noteEntries(day: date): int
    var n = 0
    for note_day, note in ^events(day).notes
        n = n + 1
    return n

pub fn noteNext(day: date): date
    return next(^events(day).notes) ?? std::clock::parseDate(\"1970-01-01\")

pub fn notePrev(day: date): date
    return prev(^events(day).notes) ?? std::clock::parseDate(\"1970-01-01\")

pub fn writeRoot(day: date)
    ^events(day).title = \"x\"

pub fn writeNote(day: date, note_day: date)
    ^events(day).notes(note_day) = \"x\"
";

#[test]
fn malformed_date_keyed_root_faults_before_count_presence_or_iteration() {
    let program = checked_program(DATE_KEYED_EVENTS);
    let store = TreeStore::memory();
    write_record_presence(
        &program,
        &store,
        "events",
        &[SavedKey::Date(SUPPORTED_DATE_MIN_DAYS - 1)],
    );

    for entry in [
        "test::rootCount",
        "test::rootExists",
        "test::rootKeys",
        "test::rootNext",
        "test::rootPrev",
    ] {
        assert_run_error(
            run_entry(&store, checked_entry!(&program, entry)),
            VALUE_RANGE,
        );
    }
}

#[test]
fn malformed_date_keyed_layer_faults_before_count_presence_or_neighbors() {
    let program = checked_program(DATE_KEYED_EVENTS);
    let store = TreeStore::memory();
    let root_day = SavedKey::Date(0);
    let malformed_day = SavedKey::Date(SUPPORTED_DATE_MIN_DAYS - 1);
    write_data_bytes(
        &program,
        &store,
        "events",
        &[root_day],
        &keyed_data_path(&program, "events", &[("notes", vec![malformed_day])], &[]),
        b"x".to_vec(),
    );

    for entry in [
        "test::noteCount",
        "test::noteExists",
        "test::noteKeys",
        "test::noteValues",
        "test::noteEntries",
        "test::noteNext",
        "test::notePrev",
    ] {
        assert_run_error(
            run_entry(&store, checked_entry!(&program, entry, Value::Date(0))),
            VALUE_RANGE,
        );
    }
}

#[test]
fn malformed_date_keyed_layer_presence_scans_past_valid_children() {
    let program = checked_program(DATE_KEYED_EVENTS);
    let store = TreeStore::memory();
    write_data_bytes(
        &program,
        &store,
        "events",
        &[SavedKey::Date(0)],
        &keyed_data_path(
            &program,
            "events",
            &[("notes", vec![SavedKey::Date(0)])],
            &[],
        ),
        b"valid".to_vec(),
    );
    write_data_bytes(
        &program,
        &store,
        "events",
        &[SavedKey::Date(0)],
        &keyed_data_path(
            &program,
            "events",
            &[("notes", vec![SavedKey::Date(SUPPORTED_DATE_MAX_DAYS + 1)])],
            &[],
        ),
        b"bad".to_vec(),
    );

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::noteExists", Value::Date(0)),
        ),
        VALUE_RANGE,
    );
}

#[test]
fn wrong_scalar_temporal_keys_fault_before_traversal_results() {
    let program = checked_program(DATE_KEYED_EVENTS);
    let store = TreeStore::memory();
    write_record_presence(
        &program,
        &store,
        "events",
        &[SavedKey::Str("bad-root".into())],
    );
    for entry in [
        "test::rootCount",
        "test::rootExists",
        "test::rootKeys",
        "test::rootNext",
        "test::rootPrev",
    ] {
        assert_run_error(run_entry(&store, checked_entry!(&program, entry)), RUN_TYPE);
    }

    let store = TreeStore::memory();
    write_data_bytes(
        &program,
        &store,
        "events",
        &[SavedKey::Date(0)],
        &keyed_data_path(
            &program,
            "events",
            &[("notes", vec![SavedKey::Str("bad-note".into())])],
            &[],
        ),
        b"x".to_vec(),
    );
    for entry in [
        "test::noteCount",
        "test::noteExists",
        "test::noteKeys",
        "test::noteValues",
        "test::noteEntries",
        "test::noteNext",
        "test::notePrev",
    ] {
        assert_run_error(
            run_entry(&store, checked_entry!(&program, entry, Value::Date(0))),
            RUN_TYPE,
        );
    }
}

#[test]
fn runtime_writes_reject_out_of_range_temporal_keys() {
    let program = checked_program(DATE_KEYED_EVENTS);
    let store = TreeStore::memory();

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::writeRoot",
                Value::Date(SUPPORTED_DATE_MIN_DAYS - 1)
            ),
        ),
        VALUE_RANGE,
    );
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::writeNote",
                Value::Date(0),
                Value::Date(SUPPORTED_DATE_MAX_DAYS + 1)
            ),
        ),
        VALUE_RANGE,
    );
}

#[test]
fn temporal_values_order_and_equate() {
    // Dates, instants, and durations compare by their underlying counts, matching
    // the ordered/equatable types the checker already advertises.
    let program = checked_program(
        "pub fn dateBefore(a: string, b: string): bool\n    return std::clock::parseDate(a) < std::clock::parseDate(b)\npub fn dateSame(a: string, b: string): bool\n    return std::clock::parseDate(a) == std::clock::parseDate(b)\npub fn instantBefore(a: string, b: string): bool\n    return std::clock::parseInstant(a) < std::clock::parseInstant(b)\npub fn durationBefore(a: string, b: string): bool\n    return std::clock::parseDuration(a) < std::clock::parseDuration(b)\n",
    );
    let call = |entry: &str, a: &str, b: &str| {
        run(checked_entry!(
            &program,
            entry,
            Value::Str(a.into()),
            Value::Str(b.into())
        ))
    };
    assert_eq!(
        call("test::dateBefore", "2024-01-01", "2024-12-31"),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call("test::dateBefore", "2024-12-31", "2024-01-01"),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        call("test::dateSame", "2024-02-29", "2024-02-29"),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call(
            "test::instantBefore",
            "2026-05-28T12:00:00Z",
            "2026-05-28T13:00:00Z"
        ),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        call("test::durationBefore", "PT60S", "PT3600S"),
        Ok(Some(Value::Bool(true)))
    );
}

/// A short-form `clock::formatDate(clock::parseDate(s))` lowers through the
/// checker to std call targets and then runs without runtime alias expansion.
#[test]
fn short_form_std_call_runs() {
    let program = checked_program_with_imports(
        "pub fn roundtrip(s: string): string\n    return clock::formatDate(clock::parseDate(s))\n",
        &["std::clock"],
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::roundtrip",
            Value::Str("2024-02-29".into())
        )),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
}

#[test]
fn print_renders_temporals_and_bytes_directly() {
    // `print` renders temporals as their canonical text and bytes as `0x`-prefixed
    // lowercase hex (dump parity), no explicit conversion required.
    let program = checked_program(
        "pub fn showDate(t: string)\n    print(std::clock::parseDate(t))\n\
         pub fn showInstant(t: string)\n    print(std::clock::parseInstant(t))\n\
         pub fn showDuration()\n    print(1.hour)\n\
         pub fn showBytes(b: bytes)\n    print(b)\n",
    );
    assert_eq!(
        run_full(checked_entry!(
            &program,
            "test::showDate",
            Value::Str("2024-02-29".into())
        ))
        .expect("date")
        .output,
        "2024-02-29\n"
    );
    assert_eq!(
        run_full(checked_entry!(
            &program,
            "test::showInstant",
            Value::Str("2026-05-28T12:00:00Z".into())
        ))
        .expect("instant")
        .output,
        "2026-05-28T12:00:00Z\n"
    );
    assert_eq!(
        run_full(checked_entry!(&program, "test::showDuration"))
            .expect("duration")
            .output,
        "PT3600S\n"
    );
    assert_eq!(
        run_full(checked_entry!(
            &program,
            "test::showBytes",
            Value::Bytes(vec![0x73, 0x6e, 0x6f, 0xff])
        ))
        .expect("bytes")
        .output,
        "0x736e6fff\n"
    );
}

#[test]
fn string_of_bytes_yields_prefixed_lowercase_hex() {
    // `string(bytes)` renders the bytes as `0x`-prefixed lowercase hex, matching the
    // print/interpolation display and `data dump`. UTF-8 decoding lives in
    // `std::bytes::toText`; this conversion never depends on valid UTF-8.
    let program = checked_program("pub fn hex(b: bytes): string\n    return string(b)\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::hex",
            Value::Bytes(vec![0x73, 0x6e, 0x6f, 0x77])
        )),
        Ok(Some(Value::Str("0x736e6f77".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::hex",
            Value::Bytes(vec![0xff, 0x00])
        )),
        Ok(Some(Value::Str("0xff00".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::hex",
            Value::Bytes(Vec::new())
        )),
        Ok(Some(Value::Str("0x".into())))
    );
}

#[test]
fn scalar_conversions_validate_a_dynamic_value() {
    // A conversion builtin asserts a dynamically-typed value is the target type
    // and returns it (the `unknown` → concrete bridge).
    let program = checked_program(
        "pub fn asInt(v: int): int\n    return int(v)\npub fn asString(v: string): string\n    return string(v)\npub fn asBool(v: bool): bool\n    return bool(v)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::asInt", Value::Int(42))),
        Ok(Some(Value::Int(42)))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::asString",
            Value::Str("hi".into())
        )),
        Ok(Some(Value::Str("hi".into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::asBool", Value::Bool(true))),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn a_conversion_rejects_a_value_of_the_wrong_type() {
    // `int(...)` validates; a string value is not an int.
    let program = checked_program("pub fn f(v: int): int\n    return int(v)\n");
    let error = rejected_entry_call(&program, "test::f", vec![Value::Str("x".into())]);
    assert_eq!(error.code(), RUN_TYPE);
}

#[test]
fn temporal_conversions_validate_their_values() {
    // `date`/`instant`/`duration` validate canonical temporal values (here built
    // via the std::clock parsers), returning them unchanged.
    let program = checked_program(
        "pub fn d(t: string): string\n    return std::clock::formatDate(date(std::clock::parseDate(t)))\npub fn span(t: string): string\n    return std::clock::formatDuration(duration(std::clock::parseDuration(t)))\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("2024-02-29".into())
        )),
        Ok(Some(Value::Str("2024-02-29".into())))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::span",
            Value::Str("PT90S".into())
        )),
        Ok(Some(Value::Str("PT90S".into())))
    );
}

#[test]
fn bool_conversion_accepts_canonical_int_forms() {
    // `bool(...)` accepts only the canonical integer forms at runtime: 0 and 1.
    let program = checked_program("pub fn b(v: int): bool\n    return bool(v)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::b", Value::Int(0))),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::b", Value::Int(1))),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn bool_conversion_rejects_non_canonical_values() {
    let program = checked_program("pub fn b(v: int): bool\n    return bool(v)\n");
    assert_run_error(
        run(checked_entry!(&program, "test::b", Value::Int(2))),
        RUN_TYPE,
    );
    checker_rejects(
        "pub fn bs(v: string): bool\n    return bool(v)\n",
        "check.call_argument",
    );
}

#[test]
fn int_conversion_parses_canonical_text() {
    let program = checked_program("pub fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::n", Value::Str("12".into()))),
        Ok(Some(Value::Int(12)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::n", Value::Str("-7".into()))),
        Ok(Some(Value::Int(-7)))
    );
}

#[test]
fn decimal_conversion_parses_canonical_text() {
    // `decimal("1.5")` parses to a decimal; rendered back through interpolation it
    // round-trips to its canonical text.
    let program = checked_program("pub fn d(v: string): string\n    return $\"{decimal(v)}\"\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("1.5".into())
        )),
        Ok(Some(Value::Str("1.5".into())))
    );
}

#[test]
fn error_code_conversion_validates_and_returns_text() {
    let program = checked_program(
        "pub fn code(): string\n\
         \x20   const code: ErrorCode = ErrorCode(\"x.y\")\n\
         \x20   if code == ErrorCode(\"x.y\")\n\
         \x20       return string(code)\n\
         \x20   return \"mismatch\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::code")),
        Ok(Some(Value::Str("x.y".into())))
    );
}

#[test]
fn error_code_conversion_accepts_dynamic_text() {
    let program =
        checked_program("pub fn code(raw: string): string\n    return string(ErrorCode(raw))\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::code",
            Value::Str("app.missing".into())
        )),
        Ok(Some(Value::Str("app.missing".into())))
    );
}

#[test]
fn error_code_conversion_failures_are_catchable_type_errors() {
    let program = checked_program(
        "pub fn code(raw: string): string\n\
         \x20   try\n\
         \x20       return string(ErrorCode(raw))\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n",
    );
    for raw in ["Bad.Code", "missing_dot"] {
        assert_eq!(
            run(checked_entry!(
                &program,
                "test::code",
                Value::Str(raw.into())
            )),
            Ok(Some(Value::Str(RUN_TYPE.into())))
        );
    }
}

#[test]
fn conversion_builtins_accept_documented_sources() {
    let program = checked_program(
        "pub fn intFromDecimal(v: decimal): int\n    return int(v)\n\
         pub fn decimalFromInt(v: int): string\n    return string(decimal(v))\n\
         pub fn stringFromInt(v: int): string\n    return string(v)\n\
         pub fn stringFromDecimal(v: decimal): string\n    return string(v)\n\
         pub fn stringFromBool(v: bool): string\n    return string(v)\n\
         pub fn stringFromBytes(v: bytes): string\n    return string(v)\n\
         pub fn bytesFromBytes(v: bytes): bytes\n    return bytes(v)\n\
         pub fn dateFromText(v: string): string\n    return string(date(v))\n\
         pub fn instantFromText(v: string): string\n    return string(instant(v))\n\
         pub fn durationFromText(v: string): string\n    return string(duration(v))\n",
    );
    let decimal = |raw: &str| Value::Decimal(Decimal::parse(raw).expect("decimal"));
    let cases: &[(&str, Value, Value)] = &[
        ("intFromDecimal", decimal("42"), Value::Int(42)),
        ("decimalFromInt", Value::Int(42), Value::Str("42".into())),
        ("stringFromInt", Value::Int(-7), Value::Str("-7".into())),
        (
            "stringFromDecimal",
            decimal("1.5"),
            Value::Str("1.5".into()),
        ),
        (
            "stringFromBool",
            Value::Bool(true),
            Value::Str("true".into()),
        ),
        (
            "stringFromBytes",
            Value::Bytes("snow".as_bytes().to_vec()),
            Value::Str("0x736e6f77".into()),
        ),
        (
            "bytesFromBytes",
            Value::Bytes(vec![0, 1, 2]),
            Value::Bytes(vec![0, 1, 2]),
        ),
        (
            "dateFromText",
            Value::Str("2024-02-29".into()),
            Value::Str("2024-02-29".into()),
        ),
        (
            "instantFromText",
            Value::Str("1970-01-01T00:00:00Z".into()),
            Value::Str("1970-01-01T00:00:00Z".into()),
        ),
        (
            "durationFromText",
            Value::Str("PT90S".into()),
            Value::Str("PT90S".into()),
        ),
    ];
    for (entry, input, expected) in cases {
        assert_eq!(
            run(checked_entry!(
                &program,
                &format!("test::{entry}"),
                input.clone()
            )),
            Ok(Some(expected.clone())),
            "{entry}"
        );
    }
}

#[test]
fn documented_conversions_reject_invalid_dynamic_values() {
    let program = checked_program(
        "pub fn intFromDecimal(v: decimal): int\n    return int(v)\n\
         pub fn dateFromText(v: string): date\n    return date(v)\n\
         fn unknownInt(): unknown\n    return 9\n\
         pub fn bytesFromUnknown(): bytes\n    return bytes(unknownInt())\n",
    );
    let cases: &[(&str, Value)] = &[
        (
            "intFromDecimal",
            Value::Decimal(Decimal::parse("1.5").expect("decimal")),
        ),
        ("dateFromText", Value::Str("2021-02-29".into())),
    ];
    for (entry, input) in cases {
        assert_run_error(
            run(checked_entry!(
                &program,
                &format!("test::{entry}"),
                input.clone()
            )),
            RUN_TYPE,
        );
    }
    assert_run_error(
        run(checked_entry!(&program, "test::bytesFromUnknown")),
        RUN_TYPE,
    );
}

#[test]
fn documented_conversion_failures_are_catchable_type_errors() {
    let program = checked_program(
        "pub fn code(v: string): string\n    try\n        var n: int = int(v)\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::code",
            Value::Str("not a number".into())
        )),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
}

#[test]
fn a_numeric_conversion_rejects_malformed_text() {
    // Malformed text is a typed numeric error, not a silent zero.
    let program = checked_program("pub fn n(v: string): int\n    return int(v)\n");
    assert_run_error(
        run(checked_entry!(
            &program,
            "test::n",
            Value::Str("nope".into())
        )),
        RUN_TYPE,
    );
    let program = checked_program("pub fn d(v: string): decimal\n    return decimal(v)\n");
    assert_run_error(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("1.2.3".into())
        )),
        RUN_TYPE,
    );
}

#[test]
fn decimal_conversion_distinguishes_malformed_text_from_envelope_overflow() {
    let program = checked_program(
        "pub fn d(v: string): decimal\n    return decimal(v)\n\
         pub fn caught(v: string): string\n    try\n        var d: decimal = decimal(v)\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_run_error(
        run(checked_entry!(
            &program,
            "test::d",
            Value::Str("1.2.3".into())
        )),
        RUN_TYPE,
    );
    for raw in [
        "99999999999999999999999999999999999",
        "0.11111111111111111111111111111111111",
    ] {
        let error =
            run_expecting_error(checked_entry!(&program, "test::d", Value::Str(raw.into())));
        assert_eq!(error.code(), RUN_DECIMAL_OVERFLOW);
        assert_eq!(
            run(checked_entry!(
                &program,
                "test::caught",
                Value::Str(raw.into())
            )),
            Ok(Some(Value::Str(RUN_DECIMAL_OVERFLOW.into())))
        );
    }
}

#[test]
fn a_conversion_error_message_includes_the_rejected_string() {
    // The message must not embed an article, so it reads correctly for
    // vowel-initial type names (not 'requires a int value').
    let program = checked_program("pub fn n(v: string): int\n    return int(v)\n");
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::n",
            Value::Str("nope".into())
        ))
        .unwrap_err()
        .message,
        "cannot convert value \"nope\" to int"
    );
}

#[test]
fn conversion_error_message_includes_a_bounded_bytes_preview() {
    // A bytes value reaching a conversion that rejects it (here `int`, via an
    // `unknown` source) renders as canonical `0x`-hex, the same form `print`
    // produces, so distinct values render distinctly. Large blobs truncate.
    let program = checked_program(
        "fn raw(v: bytes): unknown\n    return v\n\
         pub fn n(v: bytes): int\n    return int(raw(v))\n",
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::n",
            Value::Bytes(vec![0xff])
        ))
        .unwrap_err()
        .message,
        "cannot convert value 0xff to int"
    );
}

#[test]
fn parse_date_and_duration_errors_include_the_rejected_text() {
    let program = checked_program(
        "pub fn d(): date\n    return std::clock::parseDate(\"2023-02-29\")\n\
         pub fn span(): duration\n    return std::clock::parseDuration(\"nonsense\")\n",
    );

    let date_error = run(checked_entry!(&program, "test::d")).unwrap_err();
    assert_eq!(date_error.code(), RUN_TYPE);
    assert_eq!(
        date_error.message,
        "parseDate: invalid date text \"2023-02-29\""
    );
    let duration_error = run(checked_entry!(&program, "test::span")).unwrap_err();
    assert_eq!(duration_error.code(), RUN_TYPE);
    assert_eq!(
        duration_error.message,
        "parseDuration: invalid duration text \"nonsense\""
    );
}
