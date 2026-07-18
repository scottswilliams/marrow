//! End-to-end range-`for` tests: `for i in lo..hi` / `lo..=hi [by step]` travels the
//! real production path (capture → compile → encode → verify → VM) through the built
//! binary. Executable semantics assert returned values; the malformed heads assert their
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
            "marrow-sx03-range-{name}-{}-{nanos}",
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

fn project(dir: &Path, body: &str) {
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    let source = format!("module main\n\npub fn f(): int {{\n{body}\n}}\n");
    write(&dir.join("src").join("main.mw"), &source);
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

/// Compile and run `f(): int` whose body is `body`, returning the `jsonl` value line.
fn run_value(name: &str, body: &str) -> i64 {
    let temp = TempDir::new(name);
    project(&temp, body);
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "run failed for {name}: {stdout}");
    let value = stdout
        .lines()
        .find_map(|line| line.strip_prefix(r#"{"data":"#))
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_else(|| panic!("no value line for {name}: {stdout}"));
    value
        .parse()
        .unwrap_or_else(|_| panic!("value for {name}: {stdout}"))
}

/// Compile and run a body expected to fail; return the typed diagnostic code.
fn run_reject(name: &str, body: &str) -> String {
    let temp = TempDir::new(name);
    project(&temp, body);
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{name} must fail: {stdout}");
    let code = stdout
        .lines()
        .find_map(|line| {
            line.split(r#""code":""#)
                .nth(1)
                .and_then(|rest| rest.split('"').next())
        })
        .unwrap_or_else(|| panic!("no code for {name}: {stdout}"));
    code.to_string()
}

#[test]
fn inclusive_range_sums_its_integers() {
    assert_eq!(
        run_value(
            "inclusive",
            "    var s = 0\n    for i in 1..=5 {\n        s += i\n    }\n    return s"
        ),
        15
    );
}

#[test]
fn exclusive_range_stops_before_its_end() {
    assert_eq!(
        run_value(
            "exclusive",
            "    var c = 0\n    for i in 0..4 {\n        c += 1\n    }\n    return c"
        ),
        4
    );
}

#[test]
fn a_positive_step_strides_the_range() {
    // 0, 2, 4, 6, 8 — five positions.
    assert_eq!(
        run_value(
            "step",
            "    var c = 0\n    for i in 0..10 by 2 {\n        c += 1\n    }\n    return c"
        ),
        5
    );
}

#[test]
fn a_dead_range_runs_zero_times() {
    for body in [
        "    var c = 0\n    for i in 5..3 {\n        c += 1\n    }\n    return c",
        "    var c = 0\n    for i in 5..=4 {\n        c += 1\n    }\n    return c",
    ] {
        assert_eq!(run_value("dead", body), 0);
    }
}

#[test]
fn continue_and_break_target_the_range() {
    // 1 + 2 + 4 + 5 == 12 (3 skipped).
    assert_eq!(
        run_value(
            "continue",
            "    var s = 0\n    for i in 1..=5 {\n        if i == 3 { continue }\n        s += i\n    }\n    return s"
        ),
        12
    );
    // 1 + 2 + 3 + 4 == 10, then break.
    assert_eq!(
        run_value(
            "break",
            "    var s = 0\n    for i in 1..=100 {\n        if i > 4 { break }\n        s += i\n    }\n    return s"
        ),
        10
    );
}

#[test]
fn reaching_the_integer_boundary_ends_the_loop_without_a_fault() {
    // An inclusive range up to i64::MAX terminates at the boundary rather than raising
    // `run.overflow` on the final advance.
    assert_eq!(
        run_value(
            "boundary",
            "    var c = 0\n    for i in 9223372036854775805..=9223372036854775807 {\n        c += 1\n    }\n    return c"
        ),
        3
    );
}

#[test]
fn a_zero_step_is_refused() {
    assert_eq!(
        run_reject(
            "by-zero",
            "    var c = 0\n    for i in 0..10 by 0 {\n        c += 1\n    }\n    return c"
        ),
        "check.type"
    );
}

#[test]
fn a_negative_step_is_refused() {
    assert_eq!(
        run_reject(
            "by-neg",
            "    var c = 0\n    for i in 0..10 by -2 {\n        c += 1\n    }\n    return c"
        ),
        "check.type"
    );
}

#[test]
fn an_open_ended_range_is_refused() {
    assert_eq!(
        run_reject(
            "open",
            "    var c = 0\n    for i in 0.. {\n        c += 1\n    }\n    return c"
        ),
        "check.type"
    );
}

#[test]
fn a_range_binds_exactly_one_name() {
    assert_eq!(
        run_reject(
            "two-names",
            "    var c = 0\n    for i, j in 0..10 {\n        c += 1\n    }\n    return c"
        ),
        "check.type"
    );
}

#[test]
fn a_reversed_range_is_unsupported() {
    assert_eq!(
        run_reject(
            "reversed",
            "    var c = 0\n    for i in reversed 0..10 {\n        c += 1\n    }\n    return c"
        ),
        "check.unsupported"
    );
}

#[test]
fn a_non_integer_range_is_a_type_error() {
    assert_eq!(
        run_reject(
            "non-int",
            "    var c = 0\n    for i in \"a\"..\"z\" {\n        c += 1\n    }\n    return c"
        ),
        "check.type"
    );
}
