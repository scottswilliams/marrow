//! End-to-end `marrow test` tests: `test "name"` declarations and their owned
//! `assert` statement travel the real production path (capture → compile-with-tests
//! → encode → verify → VM) through the built binary and report typed JSONL.
//!
//! Sources are inline through the shared harness's [`Project`] builder; each test
//! scaffolds a project and drives the built binary through the CLI path.

use crate::common::Project;
use marrow_test_support::broken_output;

#[test]
fn entry_binding_captures_keys_and_handles_absence() {
    let output = Project::single(
        r#"resource Counter { required value: int }
store ^counters[id: int]: Counter

pub fn update(): int {
    transaction {
        ^counters[1] = Counter(value: 4)
        var id = 1
        ref counter = ^counters[id] else { return -1 }
        id = 2
        counter.value = counter.value + 3
        ref missing = ^counters[id] else { return counter.value }
        return missing.value
    }
}

test "captured address survives key mutation and absence runs its arm" {
    assert update() == 7
}
"#,
    )
    .run_cli("entry-binding", &["test", "--format", "jsonl"]);
    assert!(output.status.success(), "{output:?}");
}

/// Ordinary invocation controls use the same source and format as failed output.
#[test]
fn test_output_failure_returns_io_write_without_panicking() {
    let workspace = Project::single("test \"passes\" {\n    assert true\n}\n")
        .materialize("test-output-failure");
    for format in ["text", "jsonl"] {
        let args = ["test", "--format", format];
        assert!(workspace.marrow(&args).success());
        let writer = broken_output();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_marrow"))
            .args(args)
            .current_dir(workspace.dir())
            .stdin(std::process::Stdio::null())
            .stdout(writer)
            .output()
            .expect("run test with unwritable output");
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.starts_with(b"io.write:"), "{output:?}");
    }
}

#[test]
fn run_and_test_usage_preserve_their_exit_when_stderr_is_closed() {
    for args in [vec!["run"], vec!["test", "--unknown"]] {
        let writer = broken_output();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_marrow"))
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(writer)
            .output()
            .expect("run usage with unwritable diagnostic output");
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
    }
}

/// One passing and one failing test drive `marrow test --format jsonl`: the
/// passing test reports `passed`, the failing one reports `failed` with the
/// `run.assert` code, and the run ends with a typed summary. The command exits
/// nonzero because a test failed.
#[test]
fn passing_and_failing_tests_report_typed_jsonl() {
    let output = Project::single(
        r#"test "one plus one" {
    assert 1 + 1 == 2
}

test "one is two" {
    assert 1 == 2
}
"#,
    )
    .run_cli("pass-fail", &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a failing test must exit nonzero: {output:?}"
    );

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    let passed = lines
        .iter()
        .find(|l| l.contains(r#""name":"one plus one""#))
        .unwrap_or_else(|| panic!("no record for the passing test: {stdout}"));
    assert!(passed.contains(r#""outcome":"passed""#), "{passed}");
    assert!(passed.contains(r#""kind":"test""#), "{passed}");

    let failed = lines
        .iter()
        .find(|l| l.contains(r#""name":"one is two""#))
        .unwrap_or_else(|| panic!("no record for the failing test: {stdout}"));
    assert!(failed.contains(r#""outcome":"failed""#), "{failed}");
    assert!(failed.contains(r#""code":"run.assert""#), "{failed}");

    let summary = lines
        .iter()
        .find(|l| l.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""passed":1"#), "{summary}");
    assert!(summary.contains(r#""failed":1"#), "{summary}");
    assert!(summary.contains(r#""total":2"#), "{summary}");
}

/// The text report prints a failed assertion's own source line under its `FAIL` line,
/// so the report names what was asserted, not only where.
#[test]
fn a_failed_assert_prints_its_source_line() {
    let output = Project::single(
        "test \"one is two\" {\n    assert 1 == 2\n}\n\ntest \"one is one\" {\n    assert 1 == 1\n}\n",
    )
    .run_cli("assert-source-line", &["test"]);
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert_eq!(
        output.stdout_text(),
        "ok    one is one\n\
         FAIL  one is two (run.assert at 2:5)\n\
         \x20   assert 1 == 2\n\
         1 passed, 1 failed, 0 errored (2/2 selected)\n"
    );
}

/// `assert` outside a `test` body is a source diagnostic, not a runtime concept.
#[test]
fn assert_outside_a_test_is_a_check_diagnostic() {
    let output = Project::single(
        r#"pub fn bad(): int {
    assert true
    return 0
}
"#,
    )
    .run_cli("assert-outside", &["run", "bad", "--format", "jsonl"]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{output:?}");
    assert!(stdout.contains("check.assert_outside_test"), "{output:?}");
}

/// The identity ledger for the durable `counters` resource used below.
const COUNTERS_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A private observer reads an empty fresh attachment through the test's verified
/// demand. Its false existence result passes beside a storeless arithmetic test.
#[test]
fn a_durable_read_test_runs_against_a_fresh_attachment() {
    let output = Project::single(
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

fn present(): bool {
    return exists(^counters[1])
}

test "storeless holds" {
    assert 1 + 1 == 2
}

test "durable probe" {
    assert present() == false
}
"#,
    )
    .ids(COUNTERS_IDS)
    .run_cli("durable-test", &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Every test passes, so the command exits zero.
    assert!(output.status.success(), "{output:?}");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();

    let storeless = lines
        .iter()
        .find(|l| l.contains(r#""name":"storeless holds""#))
        .unwrap_or_else(|| panic!("no storeless record: {stdout}"));
    assert!(storeless.contains(r#""outcome":"passed""#), "{storeless}");

    let durable = lines
        .iter()
        .find(|l| l.contains(r#""name":"durable probe""#))
        .unwrap_or_else(|| panic!("no durable record: {stdout}"));
    assert!(durable.contains(r#""outcome":"passed""#), "{durable}");
    assert!(!stdout.contains("cli.durable_unsupported"), "{stdout}");
}

/// An assertion on a private observer's false result reports `failed` with
/// `run.assert`, distinct from an operational error.
#[test]
fn a_failing_durable_assert_reports_run_assert() {
    let output = Project::single(
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

fn present(): bool {
    return exists(^counters[1])
}

test "present on empty" {
    assert present()
}
"#,
    )
    .ids(COUNTERS_IDS)
    .run_cli("durable-fail", &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{output:?}");
    let durable = stdout
        .lines()
        .find(|l| l.contains(r#""name":"present on empty""#))
        .unwrap_or_else(|| panic!("no durable record: {stdout}"));
    assert!(durable.contains(r#""outcome":"failed""#), "{durable}");
    assert!(durable.contains("run.assert"), "{durable}");
}

/// Ordinary owners seed and update fields; private observers expose committed
/// entry/field presence, optional values, sparse writes and last-write-wins.
/// Binding guards retain both present and absent branches. The isolation case
/// checks its own key 88 sentinel while key 77 from the companion case stays absent.
#[test]
fn flat_durable_entry_behaviors_run_as_source_tests() {
    let output = Project::single(
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn seed(id: int, value: int) {
    transaction {
        ^counters[id] = Counter(value: value)
    }
}

pub fn replaceValue(value: int) {
    transaction {
        ^counters[1] = Counter(value: 1)
        ref c = ^counters[1] else { unreachable("created") }
        c.value = value
    }
}

pub fn writeLabel() {
    transaction {
        ^counters[1] = Counter(value: 1)
        ref c = ^counters[1] else { unreachable("created") }
        c.label = "hi"
    }
}

fn entryExists(id: int): bool {
    return exists(^counters[id])
}

fn valueExists(id: int): bool {
    return exists(^counters[id].value)
}

fn valueOf(id: int): int? {
    return ^counters[id].value
}

fn labelOf(id: int): string? {
    return ^counters[id].label
}

test "entry absent on a fresh attachment" {
    assert entryExists(9) == false
}

test "field absent on a fresh attachment" {
    assert valueExists(1) == false
}

test "field present after a write" {
    seed(1, 5)
    assert valueExists(1)
}

test "field coalesce returns the default when absent" {
    assert valueOf(1) ?? 0 == 0
}

test "field coalesce returns the value when present" {
    seed(1, 5)
    assert valueOf(1) ?? 0 == 5
}

test "required field write persists and reads back" {
    replaceValue(7)
    assert valueOf(1) ?? 0 == 7
}

test "sparse field write persists and reads back" {
    writeLabel()
    assert labelOf(1) ?? "x" == "hi"
}

test "sparse field coalesce returns the default when absent" {
    assert labelOf(1) ?? "none" == "none"
}

test "overwrite keeps the last write" {
    replaceValue(2)
    assert valueOf(1) ?? 0 == 2
}

test "binding guard skips an absent field" {
    if const v = valueOf(1) {
        assert false
    }
    assert true
}

test "binding guard reads a present field" {
    seed(1, 42)
    if const v = valueOf(1) {
        assert v == 42
    } else {
        assert false
    }
}

test "one test writes a field" {
    seed(77, 1)
    assert valueOf(77) ?? 0 == 1
}

test "a fresh attachment does not observe another test's write" {
    seed(88, 2)
    assert valueOf(88) ?? -1 == 2
    assert valueOf(77) ?? -1 == -1
}
"#,
    )
    .ids(COUNTERS_IDS)
    .run_cli("flat-durable-extraction", &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "every extracted durable behavior must pass: {output:?}"
    );
    // No block was dropped to `cli.durable_unsupported`, and none failed or turned
    // into a diagnostic: the summary accounts for all thirteen as passed.
    assert!(!stdout.contains("cli.durable_unsupported"), "{stdout}");
    assert!(!stdout.contains(r#""outcome":"failed""#), "{stdout}");
    assert!(!stdout.contains(r#""outcome":"diagnostic""#), "{stdout}");
    assert!(!stdout.contains(r#""outcome":"error""#), "{stdout}");
    let summary = stdout
        .lines()
        .find(|l| l.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""total":13"#), "{summary}");
    assert!(summary.contains(r#""passed":13"#), "{summary}");
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
}

/// A driver test drives the application's exports: it calls a mutating export, then
/// reads the result back through a reading export, each call its own invocation
/// boundary. The mutating export commits to the test's fresh attachment and the later
/// read observes the committed value, with no raw seeding.
#[test]
fn a_driver_test_drives_a_mutating_export_and_reads_it_back() {
    let output = Project::single(
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn set(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: v)
    }
}

pub fn valueOf(id: int): int? {
    return ^counters[id].value
}

test "driver sets then reads back" {
    set(1, 42)
    assert valueOf(1) ?? 0 == 42
}
"#,
    )
    .ids(COUNTERS_IDS)
    .run_cli("driver", &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    let record = stdout
        .lines()
        .find(|l| l.contains(r#""name":"driver sets then reads back""#))
        .unwrap_or_else(|| panic!("no driver record: {stdout}"));
    assert!(record.contains(r#""outcome":"passed""#), "{record}");
    assert!(!stdout.contains("cli.durable_unsupported"), "{stdout}");
}

/// A test body may not perform a durable operation directly, including after an
/// ordinary transaction-owning call. The observation belongs in a read function.
#[test]
fn mixing_a_direct_durable_op_and_driving_an_export_is_a_check_diagnostic() {
    let output = Project::single(
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn set(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: v)
    }
}

test "mixed body" {
    set(1, 5)
    assert exists(^counters[1])
}
"#,
    )
    .ids(COUNTERS_IDS)
    .run_cli("mixed-body", &["test", "--format", "jsonl"]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{stdout}");
    assert!(stdout.contains("check.test_durable_operation"), "{stdout}");
}

/// A `transaction` block in a one-armed `if` is a source diagnostic at the block, not
/// an image the verifier rejects: `marrow test` reports the typed check code and span
/// and never publishes the image.
#[test]
fn a_conditional_transaction_is_a_check_diagnostic_not_an_image_rejection() {
    const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn valueOf(id: int): int? {
    return ^counters[id].value
}

pub fn maybe(id: int, go: bool) {
    if go {
        transaction {
            ^counters[id] = Counter(value: 7)
        }
    }
}

test "conditional writer" {
    maybe(1, true)
    assert valueOf(1) ?? 0 == 7
}
"#;
    const BLOCK: &str = "{\n            ^counters[id] = Counter(value: 7)\n        }";
    let output = Project::single(SOURCE)
        .ids(COUNTERS_IDS)
        .run_cli("conditional-transaction", &["test", "--format", "jsonl"]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let record: serde_json::Value = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("each stdout line is one JSON record"))
        .find(|record: &serde_json::Value| record["code"] == "check.transaction_conditional")
        .unwrap_or_else(|| panic!("no conditional-transaction diagnostic: {stdout}"));
    let start = SOURCE.find(BLOCK).expect("the block is in the source");
    let line = SOURCE[..start].matches('\n').count() + 1;
    let column = start - SOURCE[..start].rfind('\n').map_or(0, |at| at + 1) + 1;
    assert_eq!(record["outcome"], "diagnostic", "{record}");
    assert_eq!(record["span"]["line"], line, "{record}");
    assert_eq!(record["span"]["column"], column, "{record}");
    assert!(!stdout.contains("image.flow"), "{stdout}");
}

/// `--filter` selects tests by a substring of their name and fails when none match.
#[test]
fn filter_selects_a_subset_by_name() {
    let workspace = Project::single(
        r#"test "alpha check" {
    assert true
}

test "beta check" {
    assert true
}
"#,
    )
    .materialize("filter");
    let output = workspace.marrow(&["test", "--format", "jsonl", "--filter", "alpha"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""name":"alpha check""#), "{stdout}");
    assert!(!stdout.contains(r#""name":"beta check""#), "{stdout}");

    let none = workspace.marrow(&["test", "--filter", "gamma"]);
    assert!(!none.status.success(), "a filter matching nothing fails");
}
