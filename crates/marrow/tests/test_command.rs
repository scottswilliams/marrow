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
        let root = std::env::temp_dir().join(format!(
            "marrow-p00b-{name}-{}-{nanos}",
            std::process::id()
        ));
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
        "test \"one plus one\"\n\
         \x20   assert 1 + 1 == 2\n\
         \n\
         test \"one is two\"\n\
         \x20   assert 1 == 2\n",
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
        "pub fn bad(): int\n\
         \x20   assert true\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "bad", "--format", "jsonl"]);
    assert!(!output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{output:?}");
    assert!(stdout.contains("check.assert_outside_test"), "{output:?}");
}

/// `--filter` selects tests by a substring of their name and fails when none match.
#[test]
fn filter_selects_a_subset_by_name() {
    let temp = TempDir::new("filter");
    project(
        &temp,
        "test \"alpha check\"\n\
         \x20   assert true\n\
         \n\
         test \"beta check\"\n\
         \x20   assert true\n",
    );
    let output = run_in(&temp, &["test", "--format", "jsonl", "--filter", "alpha"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""name":"alpha check""#), "{stdout}");
    assert!(!stdout.contains(r#""name":"beta check""#), "{stdout}");

    let none = run_in(&temp, &["test", "--filter", "gamma"]);
    assert!(!none.status.success(), "a filter matching nothing fails");
}
