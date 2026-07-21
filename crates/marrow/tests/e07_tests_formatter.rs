//! E07 frozen gate — the source-test and formatter journeys.
//!
//! Two production-path journeys, driven over on-disk fixtures under
//! `fixtures/v01/e07_tests_formatter/` so each program lives as idiomatic `.mw`
//! source rather than a Rust string constant:
//!
//! - **`marrow test`** over an idiomatic durable application (`catalog`: storeless,
//!   direct-durable, and driver tests, all passing) and over a segregated
//!   mixed-outcome program (`outcomes`: one passed, one `run.assert` failure, one
//!   `run.todo` error) — pinning the JSONL surface (canonical key order, typed
//!   codes, the summary ledger), the `--filter` selection, and the exit codes.
//! - **`marrow fmt`** over a deliberately mis-formatted project (`unformatted`):
//!   `--check` refuses, `--write` canonicalizes to pinned bytes with every comment
//!   preserved, and both are idempotent. The two `check_format` refusals are pinned
//!   as typed variants and as CLI codes over segregated fixtures whose source is
//!   intentionally unparseable (`unparseable`) or would strand a comment inside an
//!   open delimiter (`comment_loss`); a refusal never rewrites the file.
//!
//! Every fixture directory except the three deliberate pre-format inputs
//! (`unformatted`, `unparseable`, `comment_loss`) is itself formatter-canonical.
//! Assertions are on typed codes, exact byte-sorted JSONL tokens, the summary
//! ledger, and byte-exact formatter output — never on rendered prose.

mod common;

use common::Project;

use marrow_syntax::{FormatRefusal, check_format};

/// One nonempty JSONL record whose `name` field contains `needle`, or a panic that
/// dumps the surrounding output.
fn record_named<'a>(lines: &'a [String], needle: &str) -> &'a str {
    lines
        .iter()
        .map(String::as_str)
        .find(|line| line.contains(&format!(r#""name":"{needle}""#)))
        .unwrap_or_else(|| panic!("no JSONL record for `{needle}` in:\n{}", lines.join("\n")))
}

/// The `kind: "summary"` record, or a panic.
fn summary(lines: &[String]) -> &str {
    lines
        .iter()
        .map(String::as_str)
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record in:\n{}", lines.join("\n")))
}

/// Assert `keys` appear in `record` in strictly ascending byte position — the
/// canonical-JSONL contract is one object per line with keys in ascending byte order.
fn assert_key_order(record: &str, keys: &[&str]) {
    let mut last = 0usize;
    for key in keys {
        let needle = format!(r#""{key}":"#);
        let at = record
            .find(&needle)
            .unwrap_or_else(|| panic!("record is missing key `{key}`: {record}"));
        assert!(
            at >= last,
            "key `{key}` is out of ascending order in: {record}"
        );
        last = at;
    }
}

// ---------------------------------------------------------------------------
// `marrow test` — the source-test journey
// ---------------------------------------------------------------------------

/// The idiomatic durable application's storeless, direct-durable, and driver tests
/// all pass. Each runs against its own fresh ephemeral attachment, so the direct
/// write and the driver commit never leak between bodies; the run exits zero and the
/// summary accounts for all four as passed.
#[test]
fn catalog_tests_all_pass_with_a_canonical_jsonl_surface() {
    let out = Project::from_fixture("e07_tests_formatter/catalog")
        .run_cli("catalog-test", &["test", "--format", "jsonl"]);
    assert!(out.success(), "every catalog test must pass: {out:?}");
    let lines = out.jsonl_lines();

    for name in [
        "greeting composes a salutation",
        "a fresh member is not registered",
        "a directly written member reads back",
        "register then read back through exports",
    ] {
        let record = record_named(&lines, name);
        assert!(record.contains(r#""outcome":"passed""#), "{record}");
        assert!(record.contains(r#""kind":"test""#), "{record}");
        // A passed record carries no fault code.
        assert!(!record.contains(r#""code":"#), "{record}");
    }

    // The passed-record surface is one object with keys in ascending byte order.
    assert_key_order(
        record_named(&lines, "greeting composes a salutation"),
        &["file", "kind", "name", "outcome", "span"],
    );

    // The summary is one byte-sorted object; pin the whole line so a miscount that
    // grows a field cannot slip past an undelimited substring match.
    assert_eq!(
        summary(&lines),
        r#"{"errored":0,"failed":0,"kind":"summary","passed":4,"selected":4,"total":4}"#,
    );
}

/// The default text report lists each test and a summary ledger and exits zero.
#[test]
fn catalog_text_report_summarizes_the_run() {
    let out =
        Project::from_fixture("e07_tests_formatter/catalog").run_cli("catalog-text", &["test"]);
    assert!(out.success(), "{out:?}");
    assert!(
        out.stdout_text()
            .contains("4 passed, 0 failed, 0 errored (4/4 selected)"),
        "{}",
        out.stdout_text()
    );
}

/// `--filter` runs only the tests whose name contains the substring; `selected`
/// falls to the matched count while `total` still reports every discovered test.
#[test]
fn filter_selects_a_subset_and_keeps_the_discovered_total() {
    let workspace =
        Project::from_fixture("e07_tests_formatter/catalog").materialize("catalog-filter");

    let matched = workspace.marrow(&["test", "--format", "jsonl", "--filter", "greeting"]);
    assert!(matched.success(), "{matched:?}");
    let lines = matched.jsonl_lines();
    let record = record_named(&lines, "greeting composes a salutation");
    assert!(record.contains(r#""outcome":"passed""#), "{record}");
    assert!(
        !lines
            .iter()
            .any(|l| l.contains(r#""name":"a fresh member"#)),
        "filter must exclude unmatched tests: {lines:?}"
    );
    assert_eq!(
        summary(&lines),
        r#"{"errored":0,"failed":0,"kind":"summary","passed":1,"selected":1,"total":4}"#,
    );

    // A filter that matches nothing is a usage error (exit 2), not a failing run.
    let none = workspace.marrow(&["test", "--filter", "no-such-test"]);
    assert_eq!(
        none.code(),
        Some(2),
        "empty filter is a usage error: {none:?}"
    );
}

/// The mixed-outcome program pins each of the three outcomes and their typed codes:
/// a passed test with no code, a `run.assert` failure, and a `run.todo` error. The
/// run exits nonzero and the summary counts one of each. The `todo` author text stays
/// out of the typed JSONL grammar.
#[test]
fn outcomes_pin_passed_failed_and_errored_with_typed_codes() {
    let out = Project::from_fixture("e07_tests_formatter/outcomes")
        .run_cli("outcomes-test", &["test", "--format", "jsonl"]);
    assert!(
        !out.success(),
        "a failing/erroring run must exit nonzero: {out:?}"
    );
    assert_eq!(out.code(), Some(1), "{out:?}");
    let lines = out.jsonl_lines();

    let passed = record_named(&lines, "double doubles its argument");
    assert!(passed.contains(r#""outcome":"passed""#), "{passed}");
    assert!(
        !passed.contains(r#""code":"#),
        "a passed record carries no code: {passed}"
    );

    let failed = record_named(&lines, "a false assertion fails the test");
    assert!(failed.contains(r#""outcome":"failed""#), "{failed}");
    assert!(failed.contains(r#""code":"run.assert""#), "{failed}");
    // The fault span leads the record, keeping the whole object byte-sorted.
    assert_key_order(failed, &["code", "file", "kind", "name", "outcome", "span"]);

    let errored = record_named(&lines, "an unfinished path errors the test");
    assert!(errored.contains(r#""outcome":"errored""#), "{errored}");
    assert!(errored.contains(r#""code":"run.todo""#), "{errored}");

    assert_eq!(
        summary(&lines),
        r#"{"errored":1,"failed":1,"kind":"summary","passed":1,"selected":3,"total":3}"#,
    );

    // The `todo` static text is a runtime detail, never part of the typed record grammar.
    assert!(
        !out.stdout_text().contains("compute the expected total"),
        "todo author text must stay out of the JSONL surface: {}",
        out.stdout_text()
    );
}

// ---------------------------------------------------------------------------
// `marrow fmt` — the formatter journey
// ---------------------------------------------------------------------------

/// The canonical form of the `unformatted` fixture's single source: parameter and
/// operator spacing normalized, bodies re-indented to four spaces, the blank-line run
/// collapsed to one, and both comments (own-line and trailing) preserved in place.
const REPORT_CANONICAL: &str = "pub fn total(a: int, b: int): int {\n    \
     // sum the two inputs\n    return a + b\n}\n\npub fn describe(n: int): string {\n    \
     if n > 0 {\n        return \"positive\" // the common case\n    }\n    \
     return \"nonpositive\"\n}\n";

/// A deliberately mis-formatted project: `--check` refuses and names the file without
/// rewriting it, `--write` canonicalizes to the pinned bytes with every comment
/// preserved, and both operations are then idempotent.
#[test]
fn fmt_check_refuses_then_write_canonicalizes_idempotently() {
    let workspace =
        Project::from_fixture("e07_tests_formatter/unformatted").materialize("fmt-unformatted");
    let before = workspace.read("src/report.mw");
    assert_ne!(
        before, REPORT_CANONICAL,
        "the input fixture must be non-canonical"
    );

    // --check fails and leaves the file untouched.
    let checked = workspace.marrow(&["fmt", "--check", "."]);
    assert_eq!(
        checked.code(),
        Some(1),
        "a non-canonical project fails --check: {checked:?}"
    );
    assert!(
        checked.stderr_text().contains("report.mw"),
        "{}",
        checked.stderr_text()
    );
    assert_eq!(
        workspace.read("src/report.mw"),
        before,
        "--check must not write"
    );

    // --write canonicalizes to the pinned bytes.
    let written = workspace.marrow(&["fmt", "--write", "."]);
    assert!(written.success(), "{written:?}");
    assert_eq!(
        workspace.read("src/report.mw"),
        REPORT_CANONICAL,
        "byte-exact canonical form"
    );
    // Both comments survive the rewrite.
    assert!(REPORT_CANONICAL.contains("// sum the two inputs"));
    assert!(REPORT_CANONICAL.contains("// the common case"));

    // Idempotent: a canonical project passes --check, and a second --write is a no-op.
    let rechecked = workspace.marrow(&["fmt", "--check", "."]);
    assert!(
        rechecked.success(),
        "canonical project must pass --check: {rechecked:?}"
    );
    let rewritten = workspace.marrow(&["fmt", "--write", "."]);
    assert!(rewritten.success(), "{rewritten:?}");
    assert_eq!(
        workspace.read("src/report.mw"),
        REPORT_CANONICAL,
        "second --write is a fixed point"
    );
}

/// Source that does not parse is refused as a typed `ParseInvalid` and, through the
/// CLI, reported with `parse.syntax` while the file is left byte-for-byte untouched —
/// the formatter never publishes a guess over unparseable input.
#[test]
fn unparseable_source_is_refused_and_left_untouched() {
    let workspace =
        Project::from_fixture("e07_tests_formatter/unparseable").materialize("fmt-unparseable");
    let before = workspace.read("src/broken.mw");

    match check_format(&before) {
        Err(FormatRefusal::ParseInvalid(diagnostics)) => {
            assert!(
                !diagnostics.is_empty(),
                "a parse refusal carries its diagnostics"
            );
        }
        other => panic!("expected ParseInvalid, got {other:?}"),
    }

    let out = workspace.marrow(&["fmt", "--check", "src/broken.mw"]);
    assert_eq!(out.code(), Some(1), "{out:?}");
    assert!(
        out.stderr_text().contains("parse.syntax"),
        "{}",
        out.stderr_text()
    );
    assert_eq!(
        workspace.read("src/broken.mw"),
        before,
        "a refused file is never rewritten"
    );
}

/// A comment stranded on a continuation line inside an open delimiter cannot be
/// re-emitted losslessly, so `check_format` refuses as `CommentLoss` and the CLI
/// reports `fmt.comment_loss` rather than dropping the comment; the file is untouched.
#[test]
fn comment_loss_is_refused_and_left_untouched() {
    let workspace =
        Project::from_fixture("e07_tests_formatter/comment_loss").materialize("fmt-comment-loss");
    let before = workspace.read("src/stranded.mw");

    assert_eq!(
        check_format(&before),
        Err(FormatRefusal::CommentLoss),
        "a stranded interior comment must refuse as CommentLoss"
    );

    let out = workspace.marrow(&["fmt", "--write", "src/stranded.mw"]);
    assert_eq!(out.code(), Some(1), "{out:?}");
    assert!(
        out.stderr_text().contains("fmt.comment_loss"),
        "{}",
        out.stderr_text()
    );
    assert_eq!(
        workspace.read("src/stranded.mw"),
        before,
        "a refused file is never rewritten"
    );
}

/// The frozen test-journey fixtures are themselves formatter-canonical. This turns the
/// header's canonical-form claim into an enforcement artifact and covers the formatter
/// over durable and `test` syntax that the plain-function `unformatted` fixture never
/// exercises, so a canonicalization regression there is caught rather than drifting.
#[test]
fn frozen_test_fixtures_are_formatter_canonical() {
    for name in ["catalog", "outcomes"] {
        let workspace = Project::from_fixture(&format!("e07_tests_formatter/{name}"))
            .materialize(&format!("canonical-{name}"));
        let out = workspace.marrow(&["fmt", "--check", "."]);
        assert!(
            out.success(),
            "fixture `{name}` must be formatter-canonical: {}",
            out.stderr_text()
        );
    }
}
