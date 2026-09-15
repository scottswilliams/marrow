//! End-to-end narrow-temporal tests: `date`/`instant`/`duration` value types
//! built from canonical text literals travel the real production path (capture ->
//! compile -> encode -> verify -> VM) through the built binary, via the `temporal`
//! conformance fixture (a due-date scheduler). The language comparison order agrees
//! with the kernel key-codec byte order (pinned in `marrow-vm`'s
//! `temporal_order_agreement` test); these cases exercise the language verdicts and
//! the closed arithmetic floor.

use crate::common::{Project, conformance_dir, marrow_in};

#[test]
fn temporal_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(&conformance_dir("temporal"), &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "temporal fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":10"#), "{summary}");
}

/// A malformed or out-of-range temporal literal is a compile-time `check.type`
/// diagnostic, not a runtime fault: the literal is validated and folded at compile
/// time.
#[test]
fn a_malformed_temporal_literal_is_a_check_type() {
    let bodies = [
        r#"const d: date = date("2026-13-01")"#, // impossible month
        r#"const d: date = date("2021-02-29")"#, // not a leap year
        r#"const d: date = date("0000-01-01")"#, // year below 0001
        r#"const i: instant = instant("2026-07-15T12:00:00")"#, // missing Z
        r#"const u: duration = duration("PT01S")"#, // leading-zero seconds
        r#"const u: duration = duration("-PT0S")"#, // negative zero
    ];
    for body in bodies {
        let workspace = Project::single(&format!(
            "module main\n\npub fn f(): int {{\n\x20   {body}\n\x20   return 0\n}}\n"
        ))
        .materialize("bad-lit");
        let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{body}: {stdout}"
        );
    }
}

/// A temporal constructor argument must be a static string literal; a non-literal
/// argument is a typed `check.unsupported` (there is no runtime temporal parse).
#[test]
fn a_non_literal_temporal_argument_is_a_check_unsupported() {
    let workspace = Project::single(
        r#"module main

pub fn f(s: string): date {
    return date(s)
}
"#,
    )
    .materialize("non-lit");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl", "--", "2026-07-15"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// The prototype's `1.second` duration-suffix literal is not in the beta floor; it
/// is a typed `check.unsupported` pointing at the canonical-text constructor.
#[test]
fn a_duration_suffix_literal_is_rejected() {
    let workspace = Project::single(
        r#"module main

pub fn f(): duration {
    return 1.second
}
"#,
    )
    .materialize("suffix");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// A `Map[date, V]` is admitted (temporal types are key scalars) and iterates in
/// ascending date order regardless of insertion order.
#[test]
fn a_date_keyed_map_iterates_in_date_order() {
    let workspace = Project::single(
        r#"module main

pub fn schedule(): Map<date, int> {
    var m: Map<date, int> = Map()
    m[date("2026-07-25")] = 2
    m[date("2026-07-15")] = 1
    return m
}
"#,
    )
    .materialize("date-map");
    let jsonl = workspace.marrow(&["run", "schedule", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    // Keys render as canonical text in ascending date order (earlier date first).
    assert!(
        stdout.contains(r#""data":{"2026-07-15":1,"2026-07-25":2}"#),
        "{stdout}"
    );
}

/// A temporal export renders its result as canonical text (and JSONL string).
#[test]
fn a_temporal_result_renders_as_canonical_text() {
    let workspace = Project::single(
        r#"module main

pub fn tomorrow(d: date): date {
    return addDays(d, 1)
}
"#,
    )
    .materialize("render");
    let text = workspace.marrow(&["run", "tomorrow", "--", "2026-07-15"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(text.status.success(), "{stdout}");
    assert!(stdout.contains("2026-07-16"), "{stdout}");

    let jsonl = workspace.marrow(&["run", "tomorrow", "--format", "jsonl", "--", "2026-07-15"]);
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(stdout.contains(r#""data":"2026-07-16""#), "{stdout}");
}

/// `addDays` past the supported range faults `run.temporal_overflow` at
/// runtime (the value is computed from arguments, not a compile-time literal).
#[test]
fn date_add_days_overflow_is_a_runtime_fault() {
    let workspace = Project::single(
        r#"module main

pub fn f(d: date, n: int): date {
    return addDays(d, n)
}
"#,
    )
    .materialize("overflow");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl", "--", "9999-12-31", "1"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"run.temporal_overflow""#),
        "{stdout}"
    );
}

/// The temporal arithmetic floor is spelled camelCase verb-first (`addDays`,
/// `daysBetween`), matching the rest of the builtin floor (`isEmpty`, `nextId`).
/// The retired snake_case spellings are not aliases: they resolve to nothing and
/// are a `check.type` "not in scope" rejection, so there is one way to write each.
#[test]
fn the_retired_snake_case_temporal_names_are_out_of_scope() {
    for retired in ["date_add_days", "date_days_between"] {
        let workspace = Project::single(&format!(
            "module main\n\npub fn f(a: date, b: date): int {{\n    return {retired}(a, b)\n}}\n"
        ))
        .materialize("retired");
        let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{retired} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{retired}: {stdout}"
        );
    }
}

/// A duration word literal `COUNT UNIT` folds at compile time to the same `duration`
/// value as its canonical text, over each fixed unit, and renders as canonical text.
#[test]
fn duration_word_literals_fold_to_canonical_durations() {
    let cases: [(&str, &str); 5] = [
        ("3 days", "PT259200S"),
        ("1 second", "PT1S"),
        ("2 weeks", "PT1209600S"),
        ("1 hour", "PT3600S"),
        ("5 minutes", "PT300S"),
    ];
    for (literal, canonical) in cases {
        let workspace = Project::single(&format!(
            "module main\n\npub fn f(): duration {{\n    return {literal}\n}}\n"
        ))
        .materialize("dur-words");
        let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{literal}: {stdout}");
        assert!(
            stdout.contains(&format!(r#""data":"{canonical}""#)),
            "{literal} should fold to {canonical}: {stdout}"
        );
    }
}

/// A unit word is contextual: it names a unit only immediately after an integer
/// literal, so an ordinary identifier spelling a unit is untouched.
#[test]
fn a_unit_word_is_an_ordinary_identifier_away_from_an_integer_literal() {
    let workspace = Project::single(
        "module main\n\npub fn f(): int {\n    const seconds = 5\n    return seconds\n}\n",
    )
    .materialize("dur-ident");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":5"#), "{stdout}");
}

/// Months and years have no fixed span, so a duration word literal spelled with one
/// is a parse error rather than a duration.
#[test]
fn a_month_or_year_word_literal_is_a_parse_error() {
    for literal in ["1 month", "3 months", "1 year", "2 years"] {
        let workspace = Project::single(&format!(
            "module main\n\npub fn f(): duration {{\n    return {literal}\n}}\n"
        ))
        .materialize("dur-unfixed");
        let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{literal} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"parse.syntax""#),
            "{literal}: {stdout}"
        );
    }
}

/// Scaling a duration literal by a variable is not expressible: the literal folds to
/// a `duration` first, so `n * 1 minute` is a plain `int * duration` type error.
#[test]
fn scaling_a_duration_literal_is_a_type_error() {
    let workspace = Project::single(
        "module main\n\npub fn f(n: int): duration {\n    return n * 1 minute\n}\n",
    )
    .materialize("dur-scale");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}
