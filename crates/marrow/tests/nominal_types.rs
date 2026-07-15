//! End-to-end nominal-type tests: `type Name: int in lo..hi supports ...`
//! travels the real production path (capture → compile → encode → verify → VM)
//! through the built binary, via the `nominal_types` conformance fixture and
//! inline invalid-source projects asserting typed diagnostics and faults.

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
            std::env::temp_dir().join(format!("marrow-c02-{name}-{}-{nanos}", std::process::id()));
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

fn fixture_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance/nominal_types")
}

/// The nominal conformance fixture passes end to end: construction across the
/// whole interval, every `supports`-gated operator with interval revalidation,
/// the unguarded same-type difference, boundary-exact `.checked`, and
/// same-nominal comparisons all report `passed` through the production path.
#[test]
fn nominal_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "nominal fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":5"#), "{summary}");
}

/// Boundary revalidation: an out-of-interval construction faults `run.range` at
/// the construction's source span, on both sides of the interval, while both
/// boundary values construct. The fault is a runtime fault (`outcome: fault`),
/// not a source diagnostic or a catchable error.
#[test]
fn out_of_interval_construction_faults_run_range() {
    let temp = TempDir::new("nominal-range-fault");
    project(
        &temp,
        "type Age: int in 0..=150\n\
         \n\
         pub fn make(n: int): int\n\
         \x20   const a = Age(n)\n\
         \x20   return 0\n",
    );
    for (arg, ok) in [("0", true), ("150", true), ("-1", false), ("151", false)] {
        let output = run_in(&temp, &["run", "make", "--format", "jsonl", "--", arg]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if ok {
            assert!(
                output.status.success(),
                "Age({arg}) must construct: {stdout}"
            );
        } else {
            assert!(!output.status.success(), "Age({arg}) must fault: {stdout}");
            let fault = stdout
                .lines()
                .find(|line| line.contains(r#""code":"run.range""#))
                .unwrap_or_else(|| panic!("no run.range fault for {arg}: {stdout}"));
            assert!(fault.contains(r#""outcome":"fault""#), "{stdout}");
            assert!(fault.contains(r#""line":4"#), "{stdout}");
        }
    }
}

/// Every `supports`-gated operator that yields the nominal revalidates the
/// interval: a supported `+`, `-`, or `*` whose int result leaves the interval
/// faults `run.range` at the operation.
#[test]
fn supported_arithmetic_revalidates_the_interval() {
    let temp = TempDir::new("nominal-arith-fault");
    project(
        &temp,
        "type Age: int in 0..=150 supports add, subtract, scale\n\
         \n\
         pub fn older(n: int): int\n\
         \x20   const a = Age(140) + n\n\
         \x20   return 0\n\
         \n\
         pub fn younger(n: int): int\n\
         \x20   const a = Age(10) - n\n\
         \x20   return 0\n\
         \n\
         pub fn scaled(n: int): int\n\
         \x20   const a = Age(50) * n\n\
         \x20   return 0\n",
    );
    for (export, ok_arg, fault_arg) in [
        ("older", "10", "11"),
        ("younger", "10", "11"),
        ("scaled", "3", "4"),
    ] {
        let output = run_in(&temp, &["run", export, "--format", "jsonl", "--", ok_arg]);
        assert!(
            output.status.success(),
            "{export}({ok_arg}) must stay in the interval: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
        let output = run_in(
            &temp,
            &["run", export, "--format", "jsonl", "--", fault_arg],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{export}({fault_arg}) must fault");
        assert!(stdout.contains(r#""code":"run.range""#), "{stdout}");
    }
}

/// An operator without its capability is a typed `check.type` at the
/// operation, for each of add, subtract, scale, and the int+nominal
/// orientation. The JSONL record (code + span) is the typed contract; the CLI
/// run surface carries no diagnostic prose by design.
#[test]
fn a_missing_capability_is_a_check_type_diagnostic() {
    for body in [
        "Age(1) + 2",
        "Age(3) - 2",
        "Age(1) * 2",
        "2 + Age(1)",
        "2 * Age(1)",
    ] {
        let temp = TempDir::new("nominal-missing-cap");
        project(
            &temp,
            &format!(
                "type Age: int in 0..=150\n\
                 \n\
                 pub fn f(): int\n\
                 \x20   const a = {body}\n\
                 \x20   return 0\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body} must fail: {stdout}");
        let line = stdout
            .lines()
            .find(|line| line.contains(r#""code":"check.type""#))
            .unwrap_or_else(|| panic!("no check.type for {body}: {stdout}"));
        assert!(line.contains(r#""line":4"#), "{body}: {stdout}");
    }
}

/// `step` admits exactly `N + 1` and `N - 1` with the int literal `1`: a
/// computed or larger step needs `add`/`subtract`.
#[test]
fn step_admits_only_the_literal_one() {
    let temp = TempDir::new("nominal-step");
    project(
        &temp,
        "type Age: int in 0..=150 supports step\n\
         \n\
         pub fn next(): int\n\
         \x20   const a = Age(1) + 1\n\
         \x20   const b = a - 1\n\
         \x20   return b - Age(0)\n",
    );
    let output = run_in(&temp, &["run", "next", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // `b - Age(0)` needs subtract, which `step` does not grant: check.type at
    // the subtraction on line 6.
    assert!(!output.status.success(), "{stdout}");
    let line = stdout
        .lines()
        .find(|line| line.contains(r#""code":"check.type""#))
        .unwrap_or_else(|| panic!("no check.type: {stdout}"));
    assert!(line.contains(r#""line":6"#), "{stdout}");

    let temp = TempDir::new("nominal-step-two");
    project(
        &temp,
        "type Age: int in 0..=150 supports step\n\
         \n\
         pub fn skip(): int\n\
         \x20   const a = Age(1) + 2\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "skip", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// The nominal is distinct from its base: a plain int where the nominal is
/// required (argument or return position) is a `check.type` mismatch, and a
/// nominal where an int is required is too. The compiler signature layer keeps
/// the nominal identity even though the image records the base scalar.
#[test]
fn a_nominal_is_not_interchangeable_with_int() {
    for body in [
        // int literal into an Age parameter.
        "type Age: int in 0..=150\n\
         \n\
         fn takes(a: Age): int\n\
         \x20   return 0\n\
         \n\
         pub fn f(): int\n\
         \x20   return takes(7)\n",
        // Age into an int parameter.
        "type Age: int in 0..=150\n\
         \n\
         fn takes(n: int): int\n\
         \x20   return 0\n\
         \n\
         pub fn f(): int\n\
         \x20   return takes(Age(7))\n",
        // int returned where Age is declared.
        "type Age: int in 0..=150\n\
         \n\
         pub fn f(): Age\n\
         \x20   return 7\n",
        // Age compared with int.
        "type Age: int in 0..=150\n\
         \n\
         pub fn f(): bool\n\
         \x20   return Age(7) == 7\n",
    ] {
        let temp = TempDir::new("nominal-distinct");
        project(&temp, body);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body:?} must fail: {stdout}");
        assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
    }
}

/// An interval that admits no values, a stepped interval, and non-literal
/// bounds are typed `check.type` diagnostics at the declaration.
#[test]
fn a_malformed_interval_is_a_check_type_diagnostic() {
    for interval in ["10..=1", "5..5", "0..10 by 2", "0..n"] {
        let temp = TempDir::new("nominal-interval");
        project(
            &temp,
            &format!(
                "type Age: int in {interval}\n\
                 \n\
                 pub fn f(): int\n\
                 \x20   return 1\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{interval} must fail: {stdout}");
        let line = stdout
            .lines()
            .find(|line| line.contains(r#""code":"check.type""#))
            .unwrap_or_else(|| panic!("no check.type for {interval}: {stdout}"));
        assert!(line.contains(r#""line":1"#), "{interval}: {stdout}");
    }
}

/// A nominal over a non-int base is `check.unsupported`; an unknown or
/// repeated capability is `check.type`; a name collision with an alias or
/// resource is `check.name_conflict`.
#[test]
fn nominal_declaration_defects_are_typed_diagnostics() {
    for (source, code) in [
        (
            "type Name: string in 0..=1\n\npub fn f(): int\n\x20   return 1\n",
            "check.unsupported",
        ),
        (
            "type Age: int in 0..=1 supports shrink\n\npub fn f(): int\n\x20   return 1\n",
            "check.type",
        ),
        (
            "type Age: int in 0..=1 supports add, add\n\npub fn f(): int\n\x20   return 1\n",
            "check.type",
        ),
        (
            "alias Age = int\ntype Age: int in 0..=1\n\npub fn f(): int\n\x20   return 1\n",
            "check.name_conflict",
        ),
        (
            "resource Age\n\x20   required n: int\n\ntype Age: int in 0..=1\n\npub fn f(): int\n\x20   return 1\n",
            "check.name_conflict",
        ),
        (
            "type Age: int in 0..=1\ntype Age: int in 0..=2\n\npub fn f(): int\n\x20   return 1\n",
            "check.name_conflict",
        ),
    ] {
        let temp = TempDir::new("nominal-decl-defect");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(&format!(r#""code":"{code}""#)),
            "no {code} for {source:?}: {stdout}"
        );
    }
}

/// `.checked` belongs to nominal type names only: on a record, a plain value,
/// or an unknown name it stays a typed check diagnostic.
#[test]
fn checked_on_a_non_nominal_is_rejected() {
    for (body, family) in [
        // `string` is a keyword, so a `.checked` on it never parses as a field.
        (
            "pub fn f(): int\n\x20   return string.checked(1)\n",
            r#""code":"parse.syntax""#,
        ),
        (
            "pub fn f(n: int): int\n\x20   return n.checked(1)\n",
            r#""code":"check."#,
        ),
        (
            "pub fn f(): int\n\x20   return Missing.checked(1)\n",
            r#""code":"check."#,
        ),
    ] {
        let temp = TempDir::new("nominal-checked-miss");
        project(&temp, body);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body:?} must fail: {stdout}");
        assert!(stdout.contains(family), "{body:?}: {stdout}");
    }
}

/// A wrong-shaped construction — a named argument, the wrong arity, or a
/// non-int argument — is a typed `check.type` at the call.
#[test]
fn a_malformed_construction_is_a_check_type_diagnostic() {
    for body in [
        "return Age() - Age(0)",
        "return Age(1, 2) - Age(0)",
        "return Age(n: 1) - Age(0)",
        "return Age(\"x\") - Age(0)",
    ] {
        let temp = TempDir::new("nominal-bad-construct");
        project(
            &temp,
            &format!(
                "type Age: int in 0..=150 supports subtract\n\
                 \n\
                 pub fn f(): int\n\
                 \x20   {body}\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body} must fail: {stdout}");
        assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
    }
}

/// A terminal caller supplies export arguments as base ints; a nominal
/// parameter revalidates its interval on entry, so an out-of-interval terminal
/// argument faults `run.range` instead of entering the type unchecked.
#[test]
fn a_terminal_argument_outside_the_interval_faults_on_entry() {
    let temp = TempDir::new("nominal-entry-guard");
    project(
        &temp,
        "type Age: int in 0..=150 supports subtract\n\
         \n\
         pub fn value(a: Age): int\n\
         \x20   return a - Age(0)\n",
    );
    let output = run_in(&temp, &["run", "value", "--format", "jsonl", "--", "42"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":42"#), "{stdout}");

    let output = run_in(&temp, &["run", "value", "--format", "jsonl", "--", "500"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "an injected 500 must fault: {stdout}"
    );
    assert!(stdout.contains(r#""code":"run.range""#), "{stdout}");
}

/// Nominal types are not yet admitted as constant types or store key types: each
/// position reports `check.unsupported`, as the reference documents, until its
/// owning lane lands. (A nominal *resource field* is admitted — it erases to its
/// base scalar in the durable value shape — so it is not tested here.)
#[test]
fn nominal_types_are_unsupported_in_const_and_key_positions() {
    for source in [
        // Constant type.
        "type Age: int in 0..=150\n\
         \n\
         const A: Age = 5\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
        // Store key type.
        "type Age: int in 0..=150\n\
         \n\
         resource Person\n\
         \x20   required name: string\n\
         \n\
         store ^people(age: Age): Person\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    ] {
        let temp = TempDir::new("nominal-position");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.unsupported""#),
            "{source:?}: {stdout}"
        );
    }
}
