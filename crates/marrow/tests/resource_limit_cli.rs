//! CRES01 command-surface bytes: a program that exhausts an aggregate compiler
//! resource bound (no single source construct at fault) surfaces through the real
//! `marrow` binary as the fixed `cli.compiler_resource_limit` outcome carrying the typed
//! kind detail — which bound fired — on `run`/`test` records and the `client` stderr
//! line, with no image, identity mint, diagnostic, numeric limit, source location, or
//! partial output. Over `MAX_FUNCTIONS` functions reports `Functions`; over `MAX_EXPORTS`
//! public functions reports `Exports`. Single-file `marrow fmt` reuses the same typed
//! code for its stat-first `ProjectFileBytes` module-size admission (A9), pinned here
//! beside the aggregate bounds.

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
    for i in 0..4097 {
        source.push_str(&format!("fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    source.push_str("pub fn main(): int {\n    return 0\n}\n");
    std::fs::write(dir.join("src").join("main.mw"), source).expect("write source");
}

/// A storeless program with more public functions than `MAX_EXPORTS` (256) admits, yet
/// well under `MAX_FUNCTIONS`: the export table is the aggregate bound that fills first,
/// so the outcome names the `Exports` kind. Includes `main` as the run entry.
fn over_export_project(dir: &Path) {
    std::fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("write manifest");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    let mut source = String::from("module main\n\n");
    for i in 0..257 {
        source.push_str(&format!("pub fn f{i}(): int {{\n    return 0\n}}\n\n"));
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
fn run_text_emits_the_kinded_resource_limit_record() {
    let dir = TempDir::new("run-text");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["run", "main"]);
    assert!(!output.status.success(), "an exhausted bound fails the run");
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "cli.compiler_resource_limit: Functions\n"
    );
}

#[test]
fn run_jsonl_emits_the_kinded_operational_record() {
    let dir = TempDir::new("run-jsonl");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["run", "main", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "{\"code\":\"cli.compiler_resource_limit\",\"kind\":\"run\",\"kind_detail\":\"Functions\",\"outcome\":\"error\"}\n"
    );
}

#[test]
fn test_command_emits_the_kinded_operational_record() {
    let dir = TempDir::new("test-jsonl");
    over_bound_project(&dir.root);
    let output = run_in(&dir.root, &["test", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "{\"code\":\"cli.compiler_resource_limit\",\"kind\":\"run\",\"kind_detail\":\"Functions\",\"outcome\":\"error\"}\n"
    );
}

#[test]
fn client_emits_the_kinded_stderr_line_and_no_stdout() {
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
        "cli.compiler_resource_limit: the compiler reached a fixed resource limit (Functions)\n"
    );
}

/// A9: single-file `marrow fmt` admits at most the compiler's `ProjectFileBytes`
/// module byte limit, refusing with that admission's exact typed code from the stat
/// alone — before any open, read, or allocation. The oversized target is unreadable
/// (mode `0o000`), so a route that read first would report `io.read` instead: the
/// typed refusal is the proof no read occurred.
#[cfg(unix)]
#[test]
fn fmt_refuses_an_over_limit_file_before_reading_it() {
    use std::os::unix::fs::PermissionsExt;

    let limit = marrow_project::CaptureLimits::DEFAULT.max_file_bytes() as u64;
    let dir = TempDir::new("fmt-file-bound");
    let path = dir.root.join("big.mw");
    let file = std::fs::File::create(&path).expect("create oversized source");
    file.set_len(limit + 1).expect("size oversized source");
    drop(file);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
        .expect("make source unreadable");

    let output = run_in(&dir.root, &["fmt", "--check", "big.mw"]);
    assert!(!output.status.success(), "an over-limit file must refuse");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("cli.compiler_resource_limit") && stderr.contains("ProjectFileBytes"),
        "the refusal reuses the module-size admission's typed code: {stderr}"
    );
    assert!(
        !stderr.contains("io.read"),
        "the file must never be opened or read: {stderr}"
    );
}

/// The admission bound is exclusive at the limit: a file of exactly the module byte
/// limit passes the stat-first admission (and only then fails its unreadable open as
/// `io.read`), so the guard never over-refuses an admissible file.
#[cfg(unix)]
#[test]
fn fmt_admits_a_file_of_exactly_the_module_limit() {
    use std::os::unix::fs::PermissionsExt;

    let limit = marrow_project::CaptureLimits::DEFAULT.max_file_bytes() as u64;
    let dir = TempDir::new("fmt-file-at-bound");
    let path = dir.root.join("exact.mw");
    let file = std::fs::File::create(&path).expect("create at-bound source");
    file.set_len(limit).expect("size at-bound source");
    drop(file);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
        .expect("make source unreadable");

    let output = run_in(&dir.root, &["fmt", "--check", "exact.mw"]);
    assert!(!output.status.success(), "the unreadable open still fails");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("io.read"),
        "an at-limit file passes admission and reaches the read: {stderr}"
    );
    assert!(
        !stderr.contains("cli.compiler_resource_limit"),
        "the bound must not over-refuse an admissible file: {stderr}"
    );
}

/// A program that exhausts the export bound specifically (more than `MAX_EXPORTS` public
/// functions, still under `MAX_FUNCTIONS`) reports the `Exports` kind — so the record
/// distinguishes which aggregate bound fired, the DX finding this lane closes.
#[test]
fn run_jsonl_names_the_export_bound_kind() {
    let dir = TempDir::new("run-exports");
    over_export_project(&dir.root);
    let output = run_in(&dir.root, &["run", "main", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf8 stdout"),
        "{\"code\":\"cli.compiler_resource_limit\",\"kind\":\"run\",\"kind_detail\":\"Exports\",\"outcome\":\"error\"}\n"
    );
}
