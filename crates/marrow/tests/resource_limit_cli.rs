//! CRES01 command-surface bytes: a program that exhausts an aggregate compiler
//! resource bound (more than `MAX_FUNCTIONS` functions, no single source construct at
//! fault) surfaces through the real `marrow` binary as the fixed `cli.compiler_
//! resource_limit` outcome — a payload-free operational record on `run`/`test` and a
//! fixed bounded stderr line on `client`, with no image, identity mint, diagnostic,
//! or partial output.

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
            "marrow-cres01-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

/// A storeless program with more functions than the fixed limit admits: an aggregate
/// exhaustion with no single offending declaration.
fn over_bound_project(dir: &Path) {
    std::fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("write manifest");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    let mut source = String::from("module main\n\n");
    for i in 0..64 {
        source.push_str(&format!("fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    source.push_str("pub fn main(): int {\n    return 0\n}\n");
    std::fs::write(dir.join("src").join("main.mw"), source).expect("write source");
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

#[test]
fn run_text_emits_the_fixed_resource_limit_record() {
    let dir = TempDir::new("run-text");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["run", "main"]);
    assert!(!output.status.success(), "an exhausted bound fails the run");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "cli.compiler_resource_limit\n"
    );
}

#[test]
fn run_jsonl_emits_the_fixed_operational_record() {
    let dir = TempDir::new("run-jsonl");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["run", "main", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "{\"code\":\"cli.compiler_resource_limit\",\"kind\":\"run\",\"outcome\":\"error\"}\n"
    );
}

#[test]
fn test_command_emits_the_fixed_operational_record() {
    let dir = TempDir::new("test-jsonl");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["test", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "{\"code\":\"cli.compiler_resource_limit\",\"kind\":\"run\",\"outcome\":\"error\"}\n"
    );
}

#[test]
fn client_emits_the_fixed_stderr_line_and_no_stdout() {
    let dir = TempDir::new("client");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["client", "typescript"]);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "the generator writes no client on a resource limit"
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("utf8 stderr"),
        "cli.compiler_resource_limit: the compiler reached a fixed resource limit\n"
    );
}
