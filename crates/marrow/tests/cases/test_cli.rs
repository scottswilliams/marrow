use crate::support;
use std::fs;
use std::path::Path;
use support::{find_code_segment, json, jsonl, marrow_sub, parse_result_line, temp_project, write};

fn run_test(dir: impl AsRef<Path>) -> std::process::Output {
    run_test_args(&[dir.as_ref().to_str().expect("project path utf8")])
}

fn run_test_args(args: &[&str]) -> std::process::Output {
    marrow_sub("test", args)
}

const CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#;

fn mixed_outcome_project(name: &str) -> support::TempProject {
    temp_project(name, |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn passes()\n    print(\"passing output\")\n    std::assert::isTrue(true)\n\npub fn fails()\n    print(\"failure output\")\n    std::assert::isTrue(false)\n\npub fn errors()\n    print(\"error output\")\n    var x: decimal = 1.0\n    x = x / 0.0\n",
        );
    })
}

/// The per-test outcome `marrow test` reports in text mode, parsed from its result
/// line. The outcome is read from the leading label rather than matched as a free
/// substring; a non-`ok` outcome also carries a located `file:line:col: code:
/// message` follow-up line whose code and line are parsed alongside.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Fail,
    Error,
}

/// One parsed test result: its qualified name, its outcome, and — for a non-`ok`
/// outcome — the dotted fault code and 1-based line of the located follow-up.
#[derive(Debug)]
struct TestResult {
    name: String,
    outcome: Outcome,
    code: Option<String>,
    line: Option<u32>,
}

/// The passed/failed/errored tallies `marrow test` reports in its summary line. These
/// are typed facts: the counts the run actually produced, parsed from the rendered
/// summary rather than matched as a substring.
#[derive(Debug, PartialEq, Eq)]
struct Summary {
    total: u32,
    selected: u32,
    passed: u32,
    failed: u32,
    errored: u32,
}

/// The whole parsed text `marrow test` report: every test's outcome plus the summary
/// tallies, read from stdout.
struct TestReport {
    results: Vec<TestResult>,
    summary: Summary,
}

/// The exact rendered summary line for a single-test passing run. This small golden
/// pins the text render contract (singular `test`, the count ordering) where the
/// parsed [`Summary`] tallies cannot.
const SUMMARY_GOLDEN_ONE_PASS: &str = "1 test: 1 passed, 0 failed, 0 errored";
const SUMMARY_GOLDEN_FILTERED_ONE_PASS: &str =
    "1 of 3 selected tests: 1 passed, 0 failed, 0 errored";

/// Parse `marrow test` stdout into typed results and a summary. Result lines lead with
/// a fixed `ok`/`FAIL`/`ERROR` label; a non-`ok` result is followed by an indented
/// located line `file:line:col: code: message`. The summary is `N test[s]: P passed,
/// F failed, E errored`.
fn parse_report(stdout: &[u8]) -> TestReport {
    let text = String::from_utf8(stdout.to_vec()).expect("stdout utf8");
    let mut results = Vec::new();
    let mut summary = None;
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some(name) = line.strip_prefix("ok    ") {
            results.push(TestResult {
                name: name.to_string(),
                outcome: Outcome::Ok,
                code: None,
                line: None,
            });
        } else if let Some(rest) = line
            .strip_prefix("FAIL  ")
            .map(|name| (Outcome::Fail, name))
            .or_else(|| {
                line.strip_prefix("ERROR ")
                    .map(|name| (Outcome::Error, name))
            })
        {
            let (outcome, name) = rest;
            let located = lines
                .next()
                .expect("a located line follows a non-ok result");
            let parsed = parse_result_line(located);
            results.push(TestResult {
                name: name.to_string(),
                outcome,
                code: Some(parsed.code),
                line: Some(parsed.line.expect("a located fault line")),
            });
        } else if let Some(parsed) = parse_summary(line) {
            summary = Some(parsed);
        }
    }
    TestReport {
        results,
        summary: summary.expect("a summary line"),
    }
}

/// Parse `N test[s]: ...` or `S of T selected tests: ...` into typed tallies, or
/// `None` for any other line.
fn parse_summary(line: &str) -> Option<Summary> {
    let (selected, total, rest) = if let Some((selected, rest)) = line.split_once(" of ") {
        let selected: u32 = selected.parse().ok()?;
        let (total, rest) = rest.split_once(" selected test")?;
        let total: u32 = total.parse().ok()?;
        (selected, total, rest)
    } else {
        let (count, rest) = line.split_once(" test")?;
        let total: u32 = count.parse().ok()?;
        (total, total, rest)
    };
    let rest = rest.strip_prefix('s').unwrap_or(rest);
    let rest = rest.strip_prefix(": ")?;
    let mut tallies = rest.split(", ");
    let passed = leading_count(tallies.next()?, "passed")?;
    let failed = leading_count(tallies.next()?, "failed")?;
    let errored = leading_count(tallies.next()?, "errored")?;
    Some(Summary {
        total,
        selected,
        passed,
        failed,
        errored,
    })
}

/// Parse `<n> <label>` (for example `1 passed`) into `n`, requiring the trailing label.
fn leading_count(field: &str, label: &str) -> Option<u32> {
    let (count, found) = field.split_once(' ')?;
    (found == label).then(|| count.parse().ok())?
}

/// The dotted code of a project-level diagnostic `marrow test` prints on stderr, read
/// from its structured position rather than matched as a substring. The test driver
/// reports such a fault either bare (`code: message`, as `test.none` does) or located
/// with a severity word (`file:line:col: error: code: message`, as a failed check
/// does); the shared bare/located [`parse_result_line`] does not model that severity
/// segment, so the code is found directly among the `": "`-delimited segments here.
fn stderr_code(stderr: &[u8]) -> String {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("an error line");
    let segments: Vec<&str> = line.split(": ").collect();
    let (_index, code) = find_code_segment(&segments);
    code.to_string()
}

#[test]
fn runs_passing_tests_and_reports_a_summary() {
    let root = temp_project("test-pass", |root| {
        write(root, "marrow.json", CONFIG);
        write(
            root,
            "src/app.mw",
            "module app\n\npub fn add(a: int, b: int): int\n    return a + b\n",
        );
        write(
            root,
            "tests/app_test.mw",
            "pub fn adds_numbers()\n    std::assert::isTrue(app::add(2, 3) == 5)\n",
        );
    });
    let output = run_test(&root);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = parse_report(&output.stdout);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].name, "tests::app_test::adds_numbers");
    assert_eq!(report.results[0].outcome, Outcome::Ok);
    assert_eq!(
        report.summary,
        Summary {
            total: 1,
            selected: 1,
            passed: 1,
            failed: 0,
            errored: 0,
        }
    );
    // The summary line is human-rendered text; pin its exact form.
    let summary_line = String::from_utf8(output.stdout.clone())
        .expect("stdout utf8")
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a summary line")
        .to_string();
    assert_eq!(summary_line, SUMMARY_GOLDEN_ONE_PASS);
}

#[test]
fn test_rejects_duplicate_format_flag() {
    let output = run_test_args(&["--format", "json", "--format", "text", "missing-project"]);

    // A repeated single-valued flag is a usage error (exit 2), distinct from a
    // failing test run (exit 1). The flag name surfaces in the human usage message;
    // there is no typed code for an argument-parsing error.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("--format"), "{stderr}");
}

#[test]
fn a_failed_assertion_is_a_located_failure() {
    let root = temp_project("test-fail", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn wrong()\n    std::assert::isTrue(1 == 2)\n",
        );
    });
    let output = run_test(&root);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = parse_report(&output.stdout);
    let result = &report.results[0];
    assert_eq!(result.name, "tests::app_test::wrong");
    assert_eq!(result.outcome, Outcome::Fail);
    assert_eq!(result.code.as_deref(), Some("run.assertion"));
    assert_eq!(result.line, Some(2));
    assert_eq!(
        report.summary,
        Summary {
            total: 1,
            selected: 1,
            passed: 0,
            failed: 1,
            errored: 0,
        }
    );
}

#[test]
fn a_runtime_fault_is_reported_as_an_error() {
    let root = temp_project("test-error", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        // `/` yields `decimal`, so a `decimal` dividend keeps the assignment
        // well-typed at check time; the fault is purely a runtime divide-by-zero.
        write(
            root,
            "tests/app_test.mw",
            "pub fn divides_by_zero()\n    var x: decimal = 1.0\n    x = x / 0.0\n",
        );
    });
    let output = run_test(&root);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = parse_report(&output.stdout);
    let result = &report.results[0];
    assert_eq!(result.name, "tests::app_test::divides_by_zero");
    assert_eq!(result.outcome, Outcome::Error);
    assert_eq!(result.code.as_deref(), Some("run.divide_by_zero"));
    // The fault's origin and the test file agree for a same-file fault, so the located
    // ERROR line names the test file at the dividing line.
    assert_eq!(result.line, Some(3));
    assert_eq!(
        report.summary,
        Summary {
            total: 1,
            selected: 1,
            passed: 0,
            failed: 0,
            errored: 1,
        }
    );
}

#[test]
fn format_json_reports_test_results_and_summary() {
    let root = mixed_outcome_project("test-json-report");
    let output = run_test_args(&[
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = json(output.stdout);
    let project = fs::canonicalize(&root)
        .expect("canonical project path")
        .display()
        .to_string();
    assert_eq!(report["project"], serde_json::json!(project));
    assert_eq!(
        report["summary"],
        serde_json::json!({
            "total": 3,
            "selected": 3,
            "passed": 1,
            "failed": 1,
            "errored": 1,
        })
    );
    let tests = report["tests"].as_array().expect("tests array");
    assert_eq!(tests.len(), 3, "{report}");
    assert_eq!(
        tests[0]["name"],
        serde_json::json!("tests::app_test::passes")
    );
    assert_eq!(tests[0]["outcome"], serde_json::json!("passed"));
    assert_eq!(
        tests[0]["file"],
        serde_json::json!(root.join("tests/app_test.mw").display().to_string())
    );
    assert_eq!(tests[0]["span"]["line"], serde_json::json!(1));
    assert_eq!(tests[0]["span"]["column"], serde_json::json!(1));
    assert!(tests[0].get("status").is_none(), "{report}");
    assert!(tests[0].get("location").is_none(), "{report}");
    assert!(tests[0].get("code").is_none(), "{report}");
    assert!(tests[0].get("output").is_none(), "{report}");
    assert_eq!(
        tests[1]["name"],
        serde_json::json!("tests::app_test::fails")
    );
    assert_eq!(tests[1]["outcome"], serde_json::json!("failed"));
    assert_eq!(tests[1]["code"], serde_json::json!("run.assertion"));
    assert_eq!(tests[1]["span"]["line"], serde_json::json!(7));
    assert_eq!(tests[1]["output"], serde_json::json!("failure output\n"));
    assert!(tests[1].get("status").is_none(), "{report}");
    assert!(tests[1].get("location").is_none(), "{report}");
    assert_eq!(
        tests[2]["name"],
        serde_json::json!("tests::app_test::errors")
    );
    assert_eq!(tests[2]["outcome"], serde_json::json!("errored"));
    assert_eq!(tests[2]["code"], serde_json::json!("run.divide_by_zero"));
    assert_eq!(tests[2]["span"]["line"], serde_json::json!(12));
    assert_eq!(tests[2]["output"], serde_json::json!("error output\n"));
    assert!(tests[2].get("status").is_none(), "{report}");
    assert!(tests[2].get("location").is_none(), "{report}");
}

#[test]
fn format_json_reports_null_output_for_failed_test_without_output() {
    let root = temp_project("test-json-fail-no-output", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn fails()\n    std::assert::isTrue(false)\n",
        );
    });
    let output = run_test_args(&[
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = json(output.stdout);
    let tests = report["tests"].as_array().expect("tests array");
    assert_eq!(tests.len(), 1, "{report}");
    assert_eq!(tests[0]["outcome"], serde_json::json!("failed"));
    assert!(tests[0].get("output").is_some(), "{report}");
    assert!(tests[0]["output"].is_null(), "{report}");
}

#[test]
fn format_jsonl_bounds_failed_test_output() {
    let printed = "x".repeat(70_000);
    let source =
        format!("pub fn fails()\n    print(\"{printed}\")\n    std::assert::isTrue(false)\n");
    let root = temp_project("test-jsonl-bounded-output", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(root, "tests/app_test.mw", &source);
    });
    let output = run_test_args(&[
        "--format",
        "jsonl",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = jsonl(output.stdout);
    let captured = records[0]["output"].as_str().expect("captured output");
    assert!(captured.len() <= 64 * 1024, "{}", captured.len());
    assert!(captured.ends_with("[output truncated]\n"), "{captured:?}");
}

#[test]
fn format_jsonl_discards_passing_test_logs() {
    let logged = "x".repeat(70_000);
    let source = format!(
        "pub fn passes()\n    std::log::info(\"{logged}\")\n    std::assert::isTrue(true)\n"
    );
    let root = temp_project("test-jsonl-discard-logs", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(root, "tests/app_test.mw", &source);
    });
    let output = run_test_args(&[
        "--format",
        "jsonl",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let records = jsonl(output.stdout);
    assert_eq!(records[0]["outcome"], serde_json::json!("passed"));
    assert!(records[0].get("output").is_none(), "{records:#?}");
}

#[test]
fn format_json_reports_canonical_absolute_project_for_relative_path() {
    let root = temp_project("test-json-canonical-project", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn passes()\n    std::assert::isTrue(true)\n",
        );
    });
    let cwd = root.parent().expect("temp project parent");
    let relative = root
        .file_name()
        .expect("temp project name")
        .to_str()
        .expect("utf8 project name");
    let output = support::marrow_sub_in(cwd, "test", &["--format", "json", relative]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = json(output.stdout);
    let expected = fs::canonicalize(&root)
        .expect("canonical project path")
        .display()
        .to_string();
    assert_eq!(report["project"], serde_json::json!(expected));
}

#[test]
fn format_jsonl_streams_test_results_then_summary() {
    let root = mixed_outcome_project("test-jsonl-report");
    let output = run_test_args(&[
        "--format",
        "jsonl",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = jsonl(output.stdout);
    assert_eq!(records.len(), 4, "{records:#?}");
    assert_eq!(records[0]["kind"], serde_json::json!("test"));
    assert_eq!(
        records[0]["name"],
        serde_json::json!("tests::app_test::passes")
    );
    assert_eq!(records[0]["outcome"], serde_json::json!("passed"));
    assert_eq!(
        records[0]["file"],
        serde_json::json!(root.join("tests/app_test.mw").display().to_string())
    );
    assert_eq!(records[0]["span"]["line"], serde_json::json!(1));
    assert_eq!(records[0]["span"]["column"], serde_json::json!(1));
    assert!(records[0].get("status").is_none(), "{records:#?}");
    assert!(records[0].get("location").is_none(), "{records:#?}");
    assert!(records[0].get("code").is_none(), "{records:#?}");
    assert!(records[0].get("output").is_none(), "{records:#?}");
    assert_eq!(records[1]["kind"], serde_json::json!("test"));
    assert_eq!(
        records[1]["name"],
        serde_json::json!("tests::app_test::fails")
    );
    assert_eq!(records[1]["outcome"], serde_json::json!("failed"));
    assert_eq!(records[1]["code"], serde_json::json!("run.assertion"));
    assert_eq!(records[1]["span"]["line"], serde_json::json!(7));
    assert_eq!(records[1]["output"], serde_json::json!("failure output\n"));
    assert!(records[1].get("status").is_none(), "{records:#?}");
    assert!(records[1].get("location").is_none(), "{records:#?}");
    assert_eq!(records[2]["kind"], serde_json::json!("test"));
    assert_eq!(
        records[2]["name"],
        serde_json::json!("tests::app_test::errors")
    );
    assert_eq!(records[2]["outcome"], serde_json::json!("errored"));
    assert_eq!(records[2]["code"], serde_json::json!("run.divide_by_zero"));
    assert_eq!(records[2]["span"]["line"], serde_json::json!(12));
    assert_eq!(records[2]["output"], serde_json::json!("error output\n"));
    assert!(records[2].get("status").is_none(), "{records:#?}");
    assert!(records[2].get("location").is_none(), "{records:#?}");
    assert_eq!(
        records[3],
        serde_json::json!({
            "kind": "summary",
            "total": 3,
            "selected": 3,
            "passed": 1,
            "failed": 1,
            "errored": 1,
        })
    );
}

#[test]
fn filter_runs_only_matching_qualified_test_names() {
    let root = mixed_outcome_project("test-filter-pass");
    let output = run_test_args(&[
        "--filter",
        "app_test::passes",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = parse_report(&output.stdout);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].name, "tests::app_test::passes");
    assert_eq!(report.results[0].outcome, Outcome::Ok);
    let summary_line = String::from_utf8(output.stdout.clone())
        .expect("stdout utf8")
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a summary line")
        .to_string();
    assert_eq!(summary_line, SUMMARY_GOLDEN_FILTERED_ONE_PASS);
}

#[test]
fn format_jsonl_filter_reports_selected_and_total() {
    let root = mixed_outcome_project("test-jsonl-filter");
    let output = run_test_args(&[
        "--format",
        "jsonl",
        "--filter",
        "app_test::fails",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = jsonl(output.stdout);
    assert_eq!(records.len(), 2, "{records:#?}");
    assert_eq!(records[0]["kind"], serde_json::json!("test"));
    assert_eq!(
        records[0]["name"],
        serde_json::json!("tests::app_test::fails")
    );
    assert_eq!(records[0]["outcome"], serde_json::json!("failed"));
    assert_eq!(records[0]["code"], serde_json::json!("run.assertion"));
    assert_eq!(records[0]["output"], serde_json::json!("failure output\n"));
    assert_eq!(
        records[1],
        serde_json::json!({
            "kind": "summary",
            "total": 3,
            "selected": 1,
            "passed": 0,
            "failed": 1,
            "errored": 0,
        })
    );
}

#[test]
fn filter_with_no_matches_fails_closed() {
    let root = mixed_outcome_project("test-filter-none");
    let output = run_test_args(&[
        "--format",
        "jsonl",
        "--filter",
        "typo",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = jsonl(output.stdout);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["code"], serde_json::json!("test.none"));
    assert_eq!(
        records[0]["message"],
        serde_json::json!("no tests matched filter `typo`")
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn format_json_with_trace_is_a_usage_error() {
    let root = temp_project("test-json-trace-streams", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn traced()\n    std::assert::isTrue(true)\n",
        );
    });
    let output = run_test_args(&[
        "--trace",
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--trace") && stderr.contains("text"),
        "{stderr}"
    );
}

#[test]
fn format_json_with_trace_rejects_before_running_tests() {
    let root = temp_project("test-json-trace-envelope", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
        write(
            root,
            "tests/app_test.mw",
            "pub fn first()\n    std::assert::isTrue(true)\n\npub fn second()\n    std::assert::isTrue(true)\n",
        );
    });
    let output = run_test_args(&[
        "--trace",
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(
        stderr.contains("--trace") && stderr.contains("text"),
        "{stderr}"
    );
}

#[test]
fn reports_when_no_tests_are_found() {
    let root = temp_project("test-none", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
    });
    let output = run_test(&root);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(stderr_code(&output.stderr), "test.none");
}

#[test]
fn format_json_reports_when_no_tests_are_found() {
    let root = temp_project("test-none-json", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/app.mw", "module app\n");
    });
    let output = run_test_args(&[
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let record = json(output.stdout);
    assert_eq!(record["code"], serde_json::json!("test.none"));
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn refuses_to_run_tests_when_the_project_does_not_check() {
    let root = temp_project("test-badcheck", |root| {
        write(root, "marrow.json", CONFIG);
        // The path implies module `shelf::books`, but the file declares another.
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_test(&root);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(stderr_code(&output.stderr), "check.module_path");
}

#[test]
fn format_json_reports_project_check_errors_on_stdout() {
    let root = temp_project("test-badcheck-json", |root| {
        write(root, "marrow.json", CONFIG);
        write(root, "src/shelf/books.mw", "module shelf::other\n");
    });
    let output = run_test_args(&[
        "--format",
        "json",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = json(output.stdout);
    assert_eq!(report["status"], serde_json::json!("failed"));
    assert_eq!(report["diagnostics"][0]["code"], "check.module_path");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn format_jsonl_reports_project_load_errors_on_stdout() {
    let root = temp_project("test-badconfig-jsonl", |root| {
        write(root, "marrow.json", r#"{ "sourceRoots": [] }"#);
    });
    let output = run_test_args(&[
        "--format",
        "jsonl",
        root.to_str().expect("project path utf8"),
    ]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let records = jsonl(output.stdout);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["code"], "config.invalid");
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn each_test_runs_against_a_fresh_store() {
    let root = temp_project("test-isolation", |root| {
        write(root, "marrow.json", CONFIG);
        write(
            root,
            "src/app.mw",
            "module app\n\nresource Box\n    required value: int\nstore ^box(id: int): Box\n",
        );
        // The first test writes the box; the second asserts it is absent. Both pass
        // only if each test gets its own fresh store.
        write(
            root,
            "tests/iso_test.mw",
            "pub fn a_writes()\n    ^box(1).value = 1\n\npub fn b_sees_a_fresh_store()\n    std::assert::absent(^box(1))\n",
        );
    });
    let output = run_test(&root);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = parse_report(&output.stdout);
    assert_eq!(report.results.len(), 2);
    assert!(report.results.iter().all(|r| r.outcome == Outcome::Ok));
    assert_eq!(
        report.summary,
        Summary {
            total: 2,
            selected: 2,
            passed: 2,
            failed: 0,
            errored: 0,
        }
    );
}
