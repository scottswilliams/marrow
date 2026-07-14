//! End-to-end `marrow run` tests: source travels the real production path
//! (capture → compile → encode → verify → VM) through the built binary.

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
            std::env::temp_dir().join(format!("marrow-t01-{name}-{}-{nanos}", std::process::id()));
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

/// Create a project rooted at `dir` with one module `src/main.mw`.
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

#[test]
fn return_const_travels_the_full_production_path() {
    let temp = TempDir::new("return-const");
    project(&temp, "pub fn answer(): int\n    return 42\n");

    let output = run_in(&temp, &["run", "answer"]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

#[test]
fn return_const_jsonl_is_canonical() {
    let temp = TempDir::new("return-const-jsonl");
    project(&temp, "pub fn answer(): int\n    return 42\n");

    let output = run_in(&temp, &["run", "answer", "--format", "jsonl"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"data\":42,\"kind\":\"run\",\"outcome\":\"value\"}\n"
    );
}

#[test]
fn a_type_mismatch_is_a_source_diagnostic() {
    let temp = TempDir::new("type-mismatch");
    project(&temp, "pub fn answer(): int\n    return true\n");

    let output = run_in(&temp, &["run", "answer", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(r#""outcome":"diagnostic""#),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("check.type"),
        "{output:?}"
    );
}

#[test]
fn a_missing_export_is_a_usage_error() {
    let temp = TempDir::new("missing-export");
    project(&temp, "pub fn answer(): int\n    return 42\n");

    let output = run_in(&temp, &["run", "nope"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn locals_arithmetic_and_control_flow_compute_a_value() {
    let temp = TempDir::new("compute");
    project(
        &temp,
        "pub fn compute(): int\n\
         \x20   const a = 3\n\
         \x20   var b = 4\n\
         \x20   b = b * a\n\
         \x20   if b > 10\n\
         \x20       return b + 1\n\
         \x20   return b\n",
    );

    let output = run_in(&temp, &["run", "compute"]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // b = 12, 12 > 10, so returns 13.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "13\n");
}

#[test]
fn a_while_loop_sums() {
    let temp = TempDir::new("sum-loop");
    project(
        &temp,
        "pub fn total(): int\n\
         \x20   var sum = 0\n\
         \x20   var i = 0\n\
         \x20   while i < 5\n\
         \x20       sum = sum + i\n\
         \x20       i = i + 1\n\
         \x20   return sum\n",
    );

    let output = run_in(&temp, &["run", "total"]);
    assert!(output.status.success(), "{output:?}");
    // 0 + 1 + 2 + 3 + 4 = 10.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "10\n");
}

#[test]
fn short_circuit_boolean_logic() {
    let temp = TempDir::new("andor");
    project(
        &temp,
        "pub fn ok(): bool\n\
         \x20   const t = true\n\
         \x20   const f = false\n\
         \x20   return t and (f or t)\n",
    );

    let output = run_in(&temp, &["run", "ok"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "true\n");
}

#[test]
fn runtime_overflow_is_a_source_mapped_fault() {
    let temp = TempDir::new("overflow");
    project(
        &temp,
        "pub fn over(): int\n\
         \x20   const big = 9223372036854775807\n\
         \x20   return big + 1\n",
    );

    let output = run_in(&temp, &["run", "over", "--format", "jsonl"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"fault""#), "{output:?}");
    assert!(stdout.contains("run.overflow"), "{output:?}");
}

/// A project whose resource, constructor, field reads, optional coalescing, and
/// `if const` guard travel the full path. One source file drives several exports.
const RECORDS_SOURCE: &str = "resource Note\n\
     \x20   required title: string\n\
     \x20   body: string\n\
     \n\
     pub fn titleOf(): string\n\
     \x20   const n = Note(title: \"hello\")\n\
     \x20   return n.title\n\
     \n\
     pub fn bodyOrDefault(): string\n\
     \x20   const n = Note(title: \"hi\", body: \"there\")\n\
     \x20   return n.body ?? \"none\"\n\
     \n\
     pub fn missingBody(): string\n\
     \x20   const n = Note(title: \"hi\")\n\
     \x20   return n.body ?? \"none\"\n\
     \n\
     pub fn guardedBody(): string\n\
     \x20   const n = Note(title: \"hi\", body: \"yo\")\n\
     \x20   if const b = n.body\n\
     \x20       return b\n\
     \x20   return \"none\"\n\
     \n\
     pub fn maybe(): string?\n\
     \x20   return absent\n";

fn run_records(export: &str) -> String {
    let temp = TempDir::new(&format!("records-{export}"));
    project(&temp, RECORDS_SOURCE);
    let output = run_in(&temp, &["run", export]);
    assert!(
        output.status.success(),
        "run {export} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn required_field_read() {
    assert_eq!(run_records("titleOf"), "hello\n");
}

#[test]
fn present_sparse_field_coalesces_to_itself() {
    assert_eq!(run_records("bodyOrDefault"), "there\n");
}

#[test]
fn vacant_sparse_field_coalesces_to_default() {
    assert_eq!(run_records("missingBody"), "none\n");
}

#[test]
fn if_const_binds_a_present_optional() {
    assert_eq!(run_records("guardedBody"), "yo\n");
}

#[test]
fn an_absent_optional_return_renders_absent() {
    assert_eq!(run_records("maybe"), "absent\n");
}
