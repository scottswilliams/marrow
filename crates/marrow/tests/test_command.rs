//! End-to-end `marrow test` tests: `test "name"` declarations and their owned
//! `assert` statement travel the real production path (capture → compile-with-tests
//! → encode → verify → VM) through the built binary and report typed JSONL.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("marrow-p00b-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn project(dir: &Path, source: &str) {
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(&dir.join("src").join("main.mw"), source);
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

/// One passing and one failing test drive `marrow test --format jsonl`: the
/// passing test reports `passed`, the failing one reports `failed` with the
/// `run.assert` code, and the run ends with a typed summary. The command exits
/// nonzero because a test failed.
#[test]
fn passing_and_failing_tests_report_typed_jsonl() {
    let temp = TempDir::new("pass-fail");
    project(
        &temp,
        r#"test "one plus one" {
    assert 1 + 1 == 2
}

test "one is two" {
    assert 1 == 2
}
"#,
    );

    let output = run_in(&temp, &["test", "--format", "jsonl"]);
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

/// `assert` outside a `test` body is a source diagnostic, not a runtime concept.
#[test]
fn assert_outside_a_test_is_a_check_diagnostic() {
    let temp = TempDir::new("assert-outside");
    project(
        &temp,
        r#"pub fn bad(): int {
    assert true
    return 0
}
"#,
    );
    let output = run_in(&temp, &["run", "bad", "--format", "jsonl"]);
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

/// A durable test — one whose body reads durable data — now runs against a fresh
/// ephemeral-memory attachment (E01): its reconstructed demand bounds the test
/// attachment's ceiling, and the read kernel drives the store. On a freshly minted
/// empty attachment `exists(^counters(1))` is false, so the probe passes. The
/// storeless test in the same project runs and passes too.
#[test]
fn a_durable_read_test_runs_against_a_fresh_attachment() {
    let temp = TempDir::new("durable-test");
    project(
        &temp,
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

test "storeless holds" {
    assert 1 + 1 == 2
}

test "durable probe" {
    assert exists(^counters[1]) == false
}
"#,
    );
    write(&temp.join("marrow.ids"), COUNTERS_IDS);

    let output = run_in(&temp, &["test", "--format", "jsonl"]);
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

/// A durable test that asserts against durable state observes an empty store and a
/// false-expecting probe — the failing-assert path through the durable attachment
/// reports `failed` with `run.assert`, distinct from an operational error. Proves
/// the read kernel's runtime fault reaches the test report.
#[test]
fn a_failing_durable_assert_reports_run_assert() {
    let temp = TempDir::new("durable-fail");
    project(
        &temp,
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

test "present on empty" {
    assert exists(^counters[1])
}
"#,
    );
    write(&temp.join("marrow.ids"), COUNTERS_IDS);

    let output = run_in(&temp, &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{output:?}");
    let durable = stdout
        .lines()
        .find(|l| l.contains(r#""name":"present on empty""#))
        .unwrap_or_else(|| panic!("no durable record: {stdout}"));
    assert!(durable.contains(r#""outcome":"failed""#), "{durable}");
    assert!(durable.contains("run.assert"), "{durable}");
}

/// The flat-executable durable read/write behaviors extracted from the frozen
/// prototype's `eval_saved_*`/`eval_keyed_*` runtime families, ported as durable
/// source tests that run against a fresh ephemeral attachment (E01). Each block is
/// one behavioral pair from the tag: entry/field presence, field coalesce
/// present/absent, required and sparse field writes that persist and read back,
/// last-write-wins, the `if const` binding guard over a present and an absent
/// field, and cross-test attachment isolation. No raw seeder mints the state — a
/// block that observes a present value writes it itself, and the fresh-attachment
/// block writes its own sentinel first, then asserts that sentinel reads back while
/// another test's key stays absent — so it proves fresh state regardless of run
/// order (a reordering cannot make it pass vacuously, since a shared attachment
/// would leak the other write).
///
/// The tag's wider families are out of the flat read kernel's scope and stay with
/// their owning lanes: whole-record read/coalesce and the whole-entry
/// marker/presence-after-partial-write law (E03, the marker is written only at
/// commit); the transaction region, required-completeness at commit, nested
/// transactions, and delete/erase (E02); keyed-leaf/sequence collections, nested
/// layers, and saved-root/layer streaming loops (E03/E04); nested `group` hops and
/// widened field values (the group and codec-widening deferrals). The
/// output-only, local-keyed-parameter, error-code, and compile-time key/type
/// families are not durable read-kernel behaviors at all.
#[test]
fn flat_durable_place_behaviors_run_as_source_tests() {
    let temp = TempDir::new("flat-durable-extraction");
    project(
        &temp,
        r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

test "entry absent on a fresh attachment" {
    assert exists(^counters[9]) == false
}

test "field absent on a fresh attachment" {
    assert exists(^counters[1].value) == false
}

test "field present after a write" {
    ^counters[1].value = 5
    assert exists(^counters[1].value)
}

test "field coalesce returns the default when absent" {
    assert ^counters[1].value ?? 0 == 0
}

test "field coalesce returns the value when present" {
    ^counters[1].value = 5
    assert ^counters[1].value ?? 0 == 5
}

test "required field write persists and reads back" {
    ^counters[1].value = 7
    assert ^counters[1].value ?? 0 == 7
}

test "sparse field write persists and reads back" {
    ^counters[1].label = "hi"
    assert ^counters[1].label ?? "x" == "hi"
}

test "sparse field coalesce returns the default when absent" {
    assert ^counters[1].label ?? "none" == "none"
}

test "overwrite keeps the last write" {
    ^counters[1].value = 1
    ^counters[1].value = 2
    assert ^counters[1].value ?? 0 == 2
}

test "binding guard skips an absent field" {
    if const v = ^counters[1].value {
        assert false
    }
    assert true
}

test "binding guard reads a present field" {
    ^counters[1].value = 42
    if const v = ^counters[1].value {
        assert v == 42
    } else {
        assert false
    }
}

test "one test writes a field" {
    ^counters[77].value = 1
    assert ^counters[77].value ?? 0 == 1
}

test "a fresh attachment does not observe another test's write" {
    ^counters[88].value = 2
    assert ^counters[88].value ?? -1 == 2
    assert ^counters[77].value ?? -1 == -1
}
"#,
    );
    write(&temp.join("marrow.ids"), COUNTERS_IDS);

    let output = run_in(&temp, &["test", "--format", "jsonl"]);
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
/// read observes the committed value, with no raw seeding — the E06 app-testing style.
#[test]
fn a_driver_test_drives_a_mutating_export_and_reads_it_back() {
    let temp = TempDir::new("driver");
    project(
        &temp,
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
    );
    write(&temp.join("marrow.ids"), COUNTERS_IDS);

    let output = run_in(&temp, &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    let record = stdout
        .lines()
        .find(|l| l.contains(r#""name":"driver sets then reads back""#))
        .unwrap_or_else(|| panic!("no driver record: {stdout}"));
    assert!(record.contains(r#""outcome":"passed""#), "{record}");
    assert!(!stdout.contains("cli.durable_unsupported"), "{stdout}");
}

/// A test body may not both perform a durable operation directly and drive an export
/// that owns a transaction — the driven export's commit would consume the harness
/// session the direct operation needs. The compiler refuses the mix at check time.
#[test]
fn mixing_a_direct_durable_op_and_driving_an_export_is_a_check_diagnostic() {
    let temp = TempDir::new("mixed-body");
    project(
        &temp,
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
    );
    write(&temp.join("marrow.ids"), COUNTERS_IDS);

    let output = run_in(&temp, &["test", "--format", "jsonl"]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{stdout}");
    assert!(stdout.contains("check.test_driver_mix"), "{stdout}");
}

/// `--filter` selects tests by a substring of their name and fails when none match.
#[test]
fn filter_selects_a_subset_by_name() {
    let temp = TempDir::new("filter");
    project(
        &temp,
        r#"test "alpha check" {
    assert true
}

test "beta check" {
    assert true
}
"#,
    );
    let output = run_in(&temp, &["test", "--format", "jsonl", "--filter", "alpha"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""name":"alpha check""#), "{stdout}");
    assert!(!stdout.contains(r#""name":"beta check""#), "{stdout}");

    let none = run_in(&temp, &["test", "--filter", "gamma"]);
    assert!(!none.status.success(), "a filter matching nothing fails");
}
