//! E07 COLLECTIONS + TEMPORAL gate: the frozen ensemble of small programs a user
//! would actually write over `List`/`Map` and the narrow temporal value types, driven
//! end to end through the production path (capture -> compile -> verify -> VM) by the
//! shared `.mw` fixture harness and asserted as typed [`Value`]s.
//!
//! The programs live on disk as the `fixtures/v01/e07_collections_temporal` fixture,
//! one module per journey; this file is the assertions. Each export is storeless, so a
//! session runs it on the VM directly and no identity ledger is needed.
//!
//! Collections carry no COLLTYPES index a test can predict, so a returned `List`/`Map`
//! is inspected by destructuring rather than by rebuilding an equal value. Temporal
//! values are the runtime scalars `Value::Date` (days since the Unix epoch),
//! `Value::Instant` (signed nanoseconds since the epoch), and `Value::Duration`
//! (signed nanoseconds); the epoch-day and epoch-nanosecond expecteds are computed here
//! independently of the compiler (Howard Hinnant's `days_from_civil`), and
//! `epoch_origin_is_day_zero` anchors that computation against the compiler's own fold.

mod common;

use common::Project;
use marrow_kernel::codec::key::KeyScalar;
use marrow_vm::Value;

// ---------------------------------------------------------------------------
// Typed-value helpers
// ---------------------------------------------------------------------------

/// A present optional carrying a text value, the `string?` shape an export returns for
/// a hit.
fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

/// The vacant optional, the `string?` shape an export returns for an absent bracket
/// read.
fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

/// The `(key, value)` pairs of a returned `Map<string, int>`, in the map's stored key
/// order.
fn string_int_entries(value: Option<Value>) -> Vec<(String, i64)> {
    let Some(Value::Map(_, _, entries)) = value else {
        panic!("expected a Map value, got {value:?}");
    };
    entries
        .iter()
        .map(|(key, value)| {
            let KeyScalar::Str(text) = key else {
                panic!("expected a string map key, got {key:?}");
            };
            let Value::Int(quantity) = value else {
                panic!("expected an int map value, got {value:?}");
            };
            (text.to_string(), *quantity)
        })
        .collect()
}

/// Epoch days for a proleptic-Gregorian date, via Howard Hinnant's `days_from_civil`;
/// the compiler folds `date("YYYY-MM-DD")` to the same count (see
/// `epoch_origin_is_day_zero`).
fn date_days(year: i64, month: i64, day: i64) -> i32 {
    let year = year - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146097 + day_of_era - 719468) as i32
}

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const SECONDS_PER_DAY: i128 = 86_400;

/// Epoch nanoseconds for a UTC wall-clock instant, the `instant` runtime scalar.
fn instant_nanos(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i128 {
    let days = i128::from(date_days(year, month, day));
    let seconds = days * SECONDS_PER_DAY + i128::from(hour * 3600 + minute * 60 + second);
    seconds * NANOS_PER_SECOND
}

/// A duration of whole seconds as its nanosecond scalar.
fn duration_seconds(seconds: i128) -> Value {
    Value::Duration(seconds * NANOS_PER_SECOND)
}

// ---------------------------------------------------------------------------
// List journeys (roster)
// ---------------------------------------------------------------------------

/// A list constructed from text reads 1-based: `xs[1]` is the first name and
/// `xs[length(xs)]` is the last.
#[test]
fn a_list_reads_its_first_and_last_positions() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("first", vec![Value::Text("ada,grace,hopper".into())]),
        some_text("ada"),
    );
    assert_eq!(
        session.call("last", vec![Value::Text("ada,grace,hopper".into())]),
        some_text("hopper"),
    );
}

/// A computed position inside `1..=length` reads present; one outside reads absent,
/// with no out-of-bounds fault.
#[test]
fn an_out_of_range_list_position_reads_absent() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    let names = || Value::Text("a,b,c".into());
    assert_eq!(
        session.call("at", vec![names(), Value::Int(2)]),
        some_text("b")
    );
    assert_eq!(session.call("at", vec![names(), Value::Int(0)]), absent());
    assert_eq!(session.call("at", vec![names(), Value::Int(9)]), absent());
}

/// `append` grows a list and `join` renders it in insertion order.
#[test]
fn append_then_join_preserves_order() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call(
            "addAndList",
            vec![
                Value::Text("ada,grace".into()),
                Value::Text("hopper".into()),
                Value::Text("|".into()),
            ],
        ),
        Some(Value::Text("ada|grace|hopper".into())),
    );
}

/// A variadic `List(a, b, c)` iterates its literal contents.
#[test]
fn a_variadic_list_iterates_its_contents() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call(
            "sumEntries",
            vec![Value::Int(10), Value::Int(20), Value::Int(12)]
        ),
        Some(Value::Int(42)),
    );
}

/// An empty list has length zero and is empty.
#[test]
fn an_empty_list_is_empty() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(session.call("emptyList", vec![]), Some(Value::Bool(true)));
}

// ---------------------------------------------------------------------------
// Map journeys (inventory)
// ---------------------------------------------------------------------------

/// Bracket presence over a `Map<string, int>`: a stored key reads its value, a missing
/// key falls through `??` to the default.
#[test]
fn map_bracket_presence_reads_value_or_default() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(session.call("skuCount", vec![]), Some(Value::Int(3)));
    assert_eq!(
        session.call("onHand", vec![Value::Text("apple".into())]),
        Some(Value::Int(10))
    );
    assert_eq!(
        session.call("onHand", vec![Value::Text("missing".into())]),
        Some(Value::Int(0))
    );
    assert_eq!(session.call("totalUnits", vec![]), Some(Value::Int(22)));
}

/// The restock/discontinue journey: a replace at an existing key, an `unset` removal,
/// and a new key leave the ledger in ascending key order with the removed key gone.
#[test]
fn a_restock_replaces_unsets_and_adds() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    let ledger = string_int_entries(session.call("afterRestock", vec![]));
    assert_eq!(
        ledger,
        vec![
            ("apple".to_string(), 13),
            ("cherry".to_string(), 4),
            ("plum".to_string(), 7),
        ],
        "apple restocked to 13, pear discontinued, cherry added, in ascending key order",
    );
}

/// `unset` removes a present key (the discontinued sku then reads absent) and is an
/// idempotent no-op on an absent key.
#[test]
fn unset_removes_a_key_and_is_idempotent() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("discontinuedIsAbsent", vec![]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("unsetAbsentIsNoop", vec![]),
        Some(Value::Int(3)),
        "unsetting a key the map never held leaves its three entries untouched",
    );
}

/// A `Map` is a copied value: unsetting a key on a copy leaves the source untouched.
#[test]
fn a_map_copy_does_not_alias_its_source() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("copyDoesNotAlias", vec![]),
        Some(Value::Bool(true))
    );
}

// ---------------------------------------------------------------------------
// Keyed-position round trip (seating): the hole-preserving Map<int, string>
// ---------------------------------------------------------------------------

/// A `Map<int, string>` keeps no positional filler: an unassigned seat reads absent,
/// an assigned one reads its holder, and iteration visits seats in ascending key order.
#[test]
fn numbered_positions_round_trip_with_holes() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(session.call("occupied", vec![]), Some(Value::Int(3)));
    assert_eq!(
        session.call("holder", vec![Value::Int(1)]),
        some_text("ada")
    );
    assert_eq!(
        session.call("holder", vec![Value::Int(2)]),
        absent(),
        "seat 2 is a hole, not a positional filler",
    );
    assert_eq!(
        session.call("holder", vec![Value::Int(3)]),
        some_text("grace")
    );
    assert_eq!(
        session.call("order", vec![]),
        Some(Value::Text("ada,grace,hopper".into())),
        "iteration visits the seats in ascending key order",
    );
}

/// A `Map<int, V>` admits the key `0` as an ordinary key, not a dead list index.
#[test]
fn a_map_admits_the_key_zero() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(session.call("zeroKeyReads", vec![]), some_text("front"));
}

// ---------------------------------------------------------------------------
// Temporal keys crossing into collections (calendar)
// ---------------------------------------------------------------------------

/// A `Map<date, string>` is admitted because `date` is a key type, and it iterates in
/// ascending date order regardless of insertion order; a day with no event reads absent.
#[test]
fn a_date_keyed_map_iterates_in_date_order() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("agenda", vec![]),
        Some(Value::Text("kickoff -> review -> release".into())),
    );
    assert_eq!(session.call("labelOnKickoff", vec![]), some_text("kickoff"));
    assert_eq!(session.call("labelOnEmptyDay", vec![]), absent());
}

// ---------------------------------------------------------------------------
// Temporal journeys (scheduler)
// ---------------------------------------------------------------------------

/// The compiler folds the epoch origin to day zero, anchoring the independent
/// `date_days` computation the other date assertions rely on.
#[test]
fn epoch_origin_is_day_zero() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(session.call("epochOrigin", vec![]), Some(Value::Date(0)));
}

/// `addDays` shifts a date by whole days, staying a `date`.
#[test]
fn add_days_shifts_a_date() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("dueDate", vec![Value::Int(10)]),
        Some(Value::Date(date_days(2026, 7, 25))),
        "2026-07-15 plus ten days crosses no month boundary",
    );
    assert_eq!(
        session.call("dueDate", vec![Value::Int(17)]),
        Some(Value::Date(date_days(2026, 8, 1))),
        "seventeen days crosses into August",
    );
}

/// `daysBetween` returns the signed day span, negative when the due day is already past.
#[test]
fn days_between_counts_the_signed_span() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("daysUntil", vec![Value::Int(10), Value::Int(0)]),
        Some(Value::Int(10))
    );
    assert_eq!(
        session.call("daysUntil", vec![Value::Int(0), Value::Int(5)]),
        Some(Value::Int(-5))
    );
}

/// A date comparison is a strict same-type order: overdue means strictly before.
#[test]
fn a_date_comparison_is_strict() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("overdue", vec![Value::Int(0), Value::Int(5)]),
        Some(Value::Bool(true)),
        "due five days before today is overdue",
    );
    assert_eq!(
        session.call("overdue", vec![Value::Int(3), Value::Int(3)]),
        Some(Value::Bool(false)),
        "due on the day is not yet overdue",
    );
    assert_eq!(
        session.call("overdue", vec![Value::Int(5), Value::Int(0)]),
        Some(Value::Bool(false)),
    );
}

/// The whole-unit word literals fold to the same nanosecond durations as their
/// canonical forms, and `duration + duration` sums two windows.
#[test]
fn duration_word_literals_fold_and_sum() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("oneDay", vec![]),
        Some(duration_seconds(SECONDS_PER_DAY)),
        "`1 day` is 86400 seconds",
    );
    assert_eq!(
        session.call("totalLead", vec![]),
        Some(duration_seconds(10 * SECONDS_PER_DAY)),
        "`3 days + 1 week` is ten days",
    );
}

/// Shifting an `instant` by a `duration` yields an `instant`: subtract to reach the
/// reminder, add to reach the deadline.
#[test]
fn an_instant_shifts_by_a_duration() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("reminder", vec![]),
        Some(Value::Instant(instant_nanos(2026, 7, 15, 15, 0, 0))),
        "two hours before 17:00 is 15:00",
    );
    assert_eq!(
        session.call("deadline", vec![]),
        Some(Value::Instant(instant_nanos(2026, 7, 15, 17, 0, 0))),
        "an eight-hour window after 09:00 is 17:00",
    );
}

/// The remaining temporal orderings: a sub-second fraction sorts after the whole
/// second, a negative duration sorts below zero, and same-type date equality holds.
#[test]
fn temporal_comparisons_order_correctly() {
    let mut session = Project::from_fixture("e07_collections_temporal").session();
    assert_eq!(
        session.call("fractionalAfterWhole", vec![]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("negativeBelowZero", vec![]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("dateEquality", vec![]),
        Some(Value::Bool(true))
    );
}
