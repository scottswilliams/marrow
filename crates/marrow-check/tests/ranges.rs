//! Range-for header checks: endpoint typing, the `by` step, the default-step
//! rule, and the dead-loop / direction-safety rules.

mod support;

use marrow_check::{CheckDiagnostic, check_project};

use support::{config, temp_project, write};

/// The diagnostics a module's source produces, in report order.
fn diagnostics(source: &str) -> Vec<CheckDiagnostic> {
    let root = temp_project("range", |root| write(root, "src/m.mw", source));
    let (report, _program) = check_project(&root, &config()).expect("check");
    report.diagnostics
}

/// The diagnostic codes a module's source produces, in report order.
fn codes(source: &str) -> Vec<String> {
    diagnostics(source)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// The diagnostic messages a module's source produces, in report order.
///
/// Range diagnostics select the indefinite article for the endpoint type name
/// (`an instant`, not `a instant`); that grammar lives only in the rendered
/// message, with no typed signal to assert. The two article tests below are the
/// only coverage of that rule and rely on this helper.
fn messages(source: &str) -> Vec<String> {
    diagnostics(source)
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

fn module(body: &str) -> String {
    format!("module m\nfn f()\n{body}")
}

#[test]
fn an_int_range_with_a_by_step_checks_clean() {
    let codes = codes(&module("    for i in 1..10 by 2\n        var x: int = i\n"));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn an_int_range_default_step_checks_clean() {
    let codes = codes(&module("    for i in 1..10\n        var x: int = i\n"));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_range_loop_variable_is_typed_as_its_endpoint() {
    // The body uses `i` where an `int` is required; it must type-check, proving the
    // loop variable is the endpoint type rather than `unknown`.
    let codes = codes(&module("    for i in 1..10\n        var x: int = i + 1\n"));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_range_cannot_initialize_a_local_constant() {
    let codes = codes(&module("    const r = 1..10\n"));
    assert!(codes.iter().any(|c| c == "check.range_value"), "{codes:?}");
}

#[test]
fn a_bare_range_expression_statement_is_rejected() {
    let codes = codes(&module("    1..10\n"));
    assert!(codes.iter().any(|c| c == "check.range_value"), "{codes:?}");
}

#[test]
fn misusing_the_loop_variable_as_a_wrong_type_is_a_check_error() {
    // `i` is an int; concatenating it as a string is an operator error, which only
    // fires because the loop variable carries its endpoint type.
    let codes = codes(&module(
        "    for i in 1..10\n        var x: string = i _ \"x\"\n",
    ));
    assert!(
        codes.iter().any(|c| c == "check.operator_type"),
        "{codes:?}"
    );
}

#[test]
fn a_non_steppable_endpoint_is_a_check_error() {
    // A string range has no step, so its endpoints are rejected.
    let codes = codes(&module("    for s in \"a\"..\"z\"\n        var x = s\n"));
    assert!(
        codes.iter().any(|c| c == "check.operator_type"),
        "{codes:?}"
    );
}

#[test]
fn a_decimal_range_without_by_is_a_check_error() {
    let codes = codes(&module("    for x in 0.0..1.0\n        var y = x\n"));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_decimal_range_with_a_decimal_by_checks_clean() {
    let codes = codes(&module(
        "    for x in 0.0..1.0 by 0.25\n        var y: decimal = x\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_decimal_range_with_an_int_step_is_a_check_error() {
    let codes = codes(&module("    for x in 0.0..2.0 by 1\n        var y = x\n"));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn an_instant_range_without_by_is_a_check_error() {
    let codes = codes(&module(
        "    for t in std::clock::now()..std::clock::now()\n        var x = t\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_date_range_steps_by_a_duration() {
    // A date range needs a duration step, not a number.
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 1\n        var x = d\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_date_range_with_a_default_step_checks_clean() {
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today()\n        var x: date = d\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_literal_wrong_direction_range_is_a_dead_loop_error() {
    // `1..10 by -1` can never run; a static dead loop is a bug.
    let codes = codes(&module("    for i in 1..10 by -1\n        var x = i\n"));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_literal_descending_range_checks_clean() {
    let codes = codes(&module(
        "    for i in 10..1 by -1\n        var x: int = i\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_zero_step_is_a_check_error() {
    let codes = codes(&module("    for i in 1..10 by 0\n        var x = i\n"));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_variable_direction_range_is_not_flagged() {
    // A wrong direction is only knowable for literals; a variable step is an empty
    // loop at runtime, never a check error.
    let codes = codes("module m\nfn g(step: int)\n    for i in 1..10 by step\n        var x = i\n");
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_step_on_a_non_range_iterable_is_a_check_error() {
    let codes = codes(
        "module m\nresource Book at ^books(id: int)\n    required title: string\nfn f()\n    for book in ^books by 1\n        var x = book.title\n",
    );
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_negated_duration_step_on_a_date_range_is_a_check_error() {
    // A descending temporal range is not expressible: durations are non-negative, so
    // `by -1.day` faults at runtime. The checker rejects it rather than green-lighting
    // a guaranteed fault.
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by -1.day\n        var x = d\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_negated_duration_step_on_an_instant_range_is_a_check_error() {
    let codes = codes(&module(
        "    for t in std::clock::now()..std::clock::now() by -1.hour\n        var x = t\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn an_ascending_duration_step_on_a_date_range_checks_clean() {
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 1.day\n        var x: date = d\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_sub_day_literal_step_on_a_date_range_is_a_check_error() {
    // A date has no time of day, so `by 1.hour` would fault at runtime; the checker
    // catches the guaranteed fault now rather than green-lighting it.
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 1.hour\n        var x = d\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_non_whole_day_literal_step_on_a_date_range_is_a_check_error() {
    // 25 hours is not a whole number of days, so the date step faults at runtime.
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 25.hours\n        var x = d\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_whole_day_multiple_literal_step_on_a_date_range_checks_clean() {
    // 48 hours is exactly two days, so the date step is valid.
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 48.hours\n        var x: date = d\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_week_literal_step_on_a_date_range_checks_clean() {
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 1.week\n        var x: date = d\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_sub_day_literal_step_on_an_instant_range_checks_clean() {
    // Instants have a time component, so a sub-day step is valid; only date steps are
    // restricted to whole days.
    let codes = codes(&module(
        "    for t in std::clock::now()..std::clock::now() by 1.hour\n        var x: instant = t\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_literal_decimal_wrong_direction_range_is_a_dead_loop_error() {
    // A statically-dead decimal loop is as provably empty as an integer one.
    let codes = codes(&module(
        "    for x in 0.0..1.0 by -0.5\n        var y = x\n",
    ));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_literal_decimal_descending_wrong_direction_range_is_a_dead_loop_error() {
    let codes = codes(&module("    for x in 1.0..0.0 by 0.5\n        var y = x\n"));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
}

#[test]
fn a_valid_descending_decimal_range_checks_clean() {
    let codes = codes(&module(
        "    for x in 1.0..0.0 by -0.5\n        var y: decimal = x\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_valid_ascending_decimal_range_checks_clean() {
    let codes = codes(&module(
        "    for x in 0.0..1.0 by 0.5\n        var y: decimal = x\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_vowel_initial_endpoint_step_mismatch_uses_the_right_article() {
    // `instant` is vowel-initial; the message must read "an instant", not "a instant".
    let messages = messages(&module(
        "    for t in std::clock::now()..std::clock::now() by 1\n        var x = t\n",
    ));
    assert!(
        messages.iter().any(|m| m.contains("an `instant`")),
        "{messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("a `instant`")),
        "{messages:?}"
    );
}

#[test]
fn a_vowel_initial_endpoint_default_step_uses_the_right_article() {
    let messages = messages(&module(
        "    for t in std::clock::now()..std::clock::now()\n        var x = t\n",
    ));
    assert!(
        messages.iter().any(|m| m.contains("an `instant`")),
        "{messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("a `instant`")),
        "{messages:?}"
    );
}
