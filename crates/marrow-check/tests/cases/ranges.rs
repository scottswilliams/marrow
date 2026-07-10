//! Range-for header checks: endpoint typing, the `by` step, the default-step
//! rule, and the dead-loop / direction-safety rules.
use crate::support;
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

/// The diagnostic messages a module's source produces, in report order. Range
/// diagnostics name the endpoint type with an indefinite article, a grammar
/// detail that lives only in the rendered message; the article golden below is
/// the sole consumer.
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
fn a_nested_range_cannot_be_an_outer_range_endpoint() {
    let found = codes(&module("    for i in (1..2)..10\n        print(\"x\")\n"));
    assert_eq!(found, ["check.range_value"], "{found:?}");
}

#[test]
fn an_open_ended_range_for_header_is_an_ill_formed_range() {
    // A range used as a `for` iterable that is missing an endpoint is an
    // ill-formed range header (`check.range`), not a range-outside-`for`
    // misuse (`check.range_value`).
    for body in [
        "    for i in 0..\n        var x = i\n",
        "    for i in ..10\n        var x = i\n",
        "    for i in 0..=\n        var x = i\n",
        "    for i in ..=10\n        var x = i\n",
        "    for i in ..\n        var x = i\n",
    ] {
        let diagnostics = diagnostics(&module(body));
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code.to_string()).collect();
        assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
        assert!(!codes.iter().any(|c| c == "check.range_value"), "{codes:?}");
        let range = diagnostics
            .iter()
            .find(|d| d.code == "check.range")
            .expect("a check.range diagnostic");
        assert!(
            range.message.contains("both endpoints"),
            "{}",
            range.message
        );
    }
}

#[test]
fn misusing_the_loop_variable_as_a_wrong_type_is_a_check_error() {
    // `i` is an int; adding it as a string is an operator error, which only
    // fires because the loop variable carries its endpoint type.
    let codes = codes(&module(
        "    for i in 1..10\n        var x: string = i + \"x\"\n",
    ));
    assert!(
        codes.iter().any(|c| c == "check.operator_type"),
        "{codes:?}"
    );
}

#[test]
fn a_non_steppable_or_mismatched_endpoint_pair_is_a_range_error() {
    // `..` is a loop shape, not a value operator, so a non-steppable or mismatched
    // endpoint pair is a `check.range` header error (the endpoint-type condition),
    // never a `check.operator_type` fall-through.
    for body in [
        "    for i in 0..10.5\n        var x = i\n",
        "    for d in 1.5..2.5\n        var x = d\n",
        "    for s in \"a\"..\"z\"\n        var x = s\n",
        "    for b in true..false\n        var x = b\n",
        "    for t in 0..std::clock::today()\n        var x = t\n",
    ] {
        let codes = codes(&module(body));
        assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
        assert!(
            !codes.iter().any(|c| c == "check.operator_type"),
            "{codes:?}"
        );
    }
}

#[test]
fn a_concrete_non_scalar_endpoint_pair_is_a_range_error_not_an_operator_error() {
    // An enum, resource, or identity endpoint is concrete but not steppable. The
    // range header owns the endpoint diagnostic (`check.range`), so none of these
    // fall through to the value-operator path (`check.operator_type`).
    for source in [
        // Two enum members.
        "module m\nenum Color\n    red\n    blue\nfn f()\n    for c in Color::red..Color::blue\n        var x = c\n",
        // Two whole resources.
        "module m\nresource Point\n    required x: int\nfn f(a: Point, b: Point)\n    for p in a..b\n        var y = p\n",
        // Two saved identities.
        "module m\nresource Book\n    required title: string\nstore ^books(id: int): Book\nfn f()\n    for k in nextId(^books)..nextId(^books)\n        var y = k\n",
        // A scalar left endpoint against a resource right endpoint.
        "module m\nresource Point\n    required x: int\nfn f(p: Point)\n    for z in 0..p\n        var y = z\n",
    ] {
        let codes = codes(source);
        assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
        assert!(
            !codes.iter().any(|c| c == "check.operator_type"),
            "{codes:?}"
        );
    }
}

#[test]
fn an_int_and_decimal_endpoint_pair_is_a_range_error() {
    let codes = codes(&module("    for i in 0..2.5\n        var x = i\n"));
    assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");
    assert!(
        !codes.iter().any(|c| c == "check.operator_type"),
        "{codes:?}"
    );
}

#[test]
fn a_date_range_default_step_checks_clean_and_runs() {
    // A same-typed steppable pair still checks clean after the header owns the
    // non-steppable diagnostic.
    let codes = codes(&module(
        "    for d in std::clock::today()..std::clock::today() by 1.day\n        var x: date = d\n",
    ));
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn an_undefined_endpoint_is_not_double_reported_as_a_range_error() {
    // An unresolved endpoint faults during name resolution; its type is `unknown`,
    // so the range header defers rather than adding a spurious `check.range`.
    let codes = codes(&module(
        "    for i in undefined_lo..undefined_hi\n        var x = i\n",
    ));
    assert!(!codes.iter().any(|c| c == "check.range"), "{codes:?}");
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
        "module m\nresource Book\n    required title: string\nstore ^books(id: int): Book\nfn f()\n    for book in ^books by 1\n        var x = book.title\n",
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

/// The instant endpoint named in a range diagnostic, with its indefinite article. This is
/// a human-render contract with no typed signal: `instant` is vowel-initial, so a faithful
/// message reads "an `instant`", never "a `instant`". Pinned as a golden so an intentional
/// wording change is reviewed, while the range invariant itself is the typed `check.range`
/// code asserted alongside it.
const RENDERED_INSTANT_ARTICLE: &str = "an `instant`";

#[test]
fn an_instant_endpoint_range_diagnostic_uses_the_right_article() {
    // Both the step-mismatch (`by 1`) and the default-step (no `by`) instant ranges are a
    // `check.range` error and render the endpoint type name, so both must select the article.
    for body in [
        "    for t in std::clock::now()..std::clock::now() by 1\n        var x = t\n",
        "    for t in std::clock::now()..std::clock::now()\n        var x = t\n",
    ] {
        let source = module(body);

        let codes = codes(&source);
        assert!(codes.iter().any(|c| c == "check.range"), "{codes:?}");

        let messages = messages(&source);
        assert!(
            messages
                .iter()
                .any(|m| m.contains(RENDERED_INSTANT_ARTICLE)),
            "{messages:?}"
        );
        assert!(
            !messages.iter().any(|m| m.contains("a `instant`")),
            "{messages:?}"
        );
    }
}
