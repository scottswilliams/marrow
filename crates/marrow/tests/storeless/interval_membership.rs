//! End-to-end interval-membership tests: `value in lo..hi` / `value not in lo..hi`
//! travels the real production path (capture → compile → encode → verify → VM) through
//! the built binary. The half-open/inclusive boundaries and the `not in` negation are
//! exercised over a parameterized `f(x: int): bool`; the malformed forms assert their
//! typed diagnostic codes.

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
            "marrow-sx03-in-{name}-{}-{nanos}",
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

fn project(dir: &Path, body: &str) {
    fs::create_dir_all(dir.join("src")).expect("create src");
    fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("toml");
    let source = format!("module main\n\npub fn f(x: int): bool {{\n    return {body}\n}}\n");
    fs::write(dir.join("src").join("main.mw"), source).expect("source");
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

/// Evaluate `f(x): bool` whose body is `body`, at argument `x`, returning the value.
fn eval(name: &str, body: &str, x: i64) -> bool {
    let temp = TempDir::new(name);
    project(&temp, body);
    let output = run_in(
        &temp,
        &["run", "f", "--format", "jsonl", "--", &x.to_string()],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{body} at {x}: {stdout}");
    if stdout.contains(r#""data":true"#) {
        true
    } else if stdout.contains(r#""data":false"#) {
        false
    } else {
        panic!("no bool value for {body} at {x}: {stdout}");
    }
}

/// Compile a body expected to fail; return the typed diagnostic code.
fn reject(name: &str, body: &str) -> String {
    let temp = TempDir::new(name);
    project(&temp, body);
    let output = run_in(&temp, &["run", "f", "--format", "jsonl", "--", "0"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{body} must fail: {stdout}");
    stdout
        .split(r#""code":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no code for {body}: {stdout}"))
        .to_string()
}

#[test]
fn a_half_open_range_excludes_its_upper_bound() {
    let body = "x in 0..10";
    assert!(!eval("half-open", body, -1));
    assert!(eval("half-open", body, 0));
    assert!(eval("half-open", body, 9));
    assert!(!eval("half-open", body, 10));
    assert!(!eval("half-open", body, 11));
}

#[test]
fn an_inclusive_range_includes_its_upper_bound() {
    let body = "x in 0..=10";
    assert!(eval("inclusive", body, 10));
    assert!(!eval("inclusive", body, 11));
}

#[test]
fn not_in_is_the_negation() {
    let body = "x not in 0..10";
    assert!(!eval("not-in", body, 5));
    assert!(eval("not-in", body, 20));
    assert!(eval("not-in", body, -1));
}

#[test]
fn a_non_range_right_operand_is_a_type_error() {
    assert_eq!(reject("non-range", "x in 5"), "check.type");
}

#[test]
fn a_membership_range_takes_no_step() {
    assert_eq!(reject("step", "x in 0..10 by 2"), "check.type");
}

#[test]
fn an_open_ended_membership_range_is_a_type_error() {
    assert_eq!(reject("open", "x in 0.."), "check.type");
}

#[test]
fn a_chained_membership_is_a_parse_error() {
    assert_eq!(reject("chained", "x in 0..10 in 0..3"), "parse.syntax");
}
