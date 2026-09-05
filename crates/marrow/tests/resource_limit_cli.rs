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

/// One export and 257 uniquely titled tests: the test-inclusive image crosses
/// `MAX_TEST_ENTRIES` (256) while the production image, which excludes them, fits.
fn over_test_entries_project(dir: &Path) {
    std::fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("write manifest");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    let mut source = String::from("module main\n\npub fn main(): int {\n    return 0\n}\n\n");
    for i in 0..257 {
        source.push_str(&format!("test \"t{i}\" {{\n    assert true\n}}\n\n"));
    }
    std::fs::write(dir.join("src").join("main.mw"), source).expect("write source");
}

/// Thirty-two bodies of 512 accumulating statements: the settled bodies alone cross
/// the image byte ceiling, so the drive stops before it finishes lowering.
fn over_image_bytes_project(dir: &Path) {
    std::fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("write manifest");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    let mut source = String::from("module main\n\n");
    for index in 0..32 {
        source.push_str(&format!("pub fn f{index}(): int {{\n    var total = 0\n"));
        for _ in 0..512 {
            source.push_str("    total += 1\n");
        }
        source.push_str("    return total\n}\n\n");
    }
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
        "cli.compiler_resource_limit: the function table is full\n",
        "text output names the bound in words; the Rust variant name stays on the \
         machine-readable `kind_detail` surface"
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
        "cli.compiler_resource_limit: the compiler reached a fixed resource limit: the \
         function table is full\n"
    );
}

/// `check` names the exhausted bound in the same words its stderr siblings use. It
/// reported only that the project could not be checked, which is the outcome, not the
/// cause: a reader had no way to tell an exhausted bound from any other refusal.
#[test]
fn check_emits_the_same_kinded_stderr_line_as_its_siblings() {
    let dir = TempDir::new("check-stderr");
    over_bound_project(&dir.root);
    let checked = run_in(&dir.root, &["check", "."]);
    assert!(!checked.status.success());
    let checked_stderr = String::from_utf8(checked.stderr).expect("utf8 stderr");
    assert_eq!(
        checked_stderr,
        "cli.compiler_resource_limit: the compiler reached a fixed resource limit: the \
         function table is full\n"
    );

    let generated = run_in(&dir.root, &["client", "typescript"]);
    assert_eq!(
        checked_stderr,
        String::from_utf8(generated.stderr).expect("utf8 stderr"),
        "one bound must read the same whichever command reports it on stderr"
    );
}

/// An export ceiling is a verdict of the production projection, and `check` runs the
/// analysis floor before it projects. So the floor reports nothing — the program checks —
/// and the only line a reader sees is the projection's fixed bound, in the words its
/// stderr siblings use. A diagnostic printed here would mean the analysis floor had
/// adopted an image bound as a source-level problem.
#[test]
fn check_reports_an_export_ceiling_with_no_diagnostic() {
    let dir = TempDir::new("check-exports");
    over_export_project(&dir.root);
    let checked = run_in(&dir.root, &["check", "."]);
    assert!(
        !checked.status.success(),
        "an exhausted bound fails the check"
    );
    assert!(
        checked.stdout.is_empty(),
        "no demand summary is described for a project with no image"
    );
    let checked_stderr = String::from_utf8(checked.stderr).expect("utf8 stderr");
    assert_eq!(
        checked_stderr,
        "cli.compiler_resource_limit: the compiler reached a fixed resource limit: the \
         export table is full\n",
        "the bound is the only thing reported: no diagnostic, location, or count"
    );

    let generated = run_in(&dir.root, &["client", "typescript"]);
    assert_eq!(
        checked_stderr,
        String::from_utf8(generated.stderr).expect("utf8 stderr"),
        "one bound must read the same whichever command reports it on stderr"
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

    let limit = marrow_compile::MAX_PARSED_FILE_BYTES as u64;
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
    assert_eq!(
        stderr,
        format!(
            "cli.compiler_resource_limit: `big.mw` is {} bytes, over the per-file byte limit \
             ({limit})\n",
            limit + 1
        ),
        "the refusal carries the admission's typed code and the project path's own \
         sentence for this bound — never a Rust identifier"
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

    let limit = marrow_compile::MAX_PARSED_FILE_BYTES as u64;
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

/// `check` drives once with tests included and encodes that image, so a bound only the
/// test entries cross refuses the check in the bound owner's words, with no diagnostic
/// and no demand summary — while the production `run` of the same project, whose image
/// excludes the tests, succeeds. The two outcomes are stated together because the
/// difference is the documented contract, not an accident of either command.
#[test]
fn check_refuses_a_test_entry_ceiling_that_the_production_run_does_not_reach() {
    let dir = TempDir::new("check-test-entries");
    over_test_entries_project(&dir.root);
    let checked = run_in(&dir.root, &["check", "."]);
    assert!(
        !checked.status.success(),
        "the test-inclusive image is refused"
    );
    assert!(
        checked.stdout.is_empty(),
        "no demand summary without an image"
    );
    assert_eq!(
        String::from_utf8(checked.stderr).expect("utf8 stderr"),
        "cli.compiler_resource_limit: the compiler reached a fixed resource limit: the \
         test entry table is full\n"
    );

    let ran = run_in(&dir.root, &["run", "main"]);
    assert!(
        ran.status.success(),
        "the production image excludes the tests and fits: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(String::from_utf8(ran.stdout).expect("utf8 stdout"), "0\n");
}

/// Two hundred exports each returning a distinct literal near the per-string bound:
/// the string pool alone carries the encoded image past the byte ceiling while the
/// settled bodies stay far inside the charge, so the ceiling is reported late, by the
/// encoder over a finished program.
fn over_image_bytes_through_strings_project(dir: &Path) {
    std::fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("write manifest");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    let mut source = String::from("module main\n\n");
    for index in 0..200 {
        let literal = format!("{index:04}").repeat(750);
        source.push_str(&format!(
            "pub fn s{index}(): string {{\n    return \"{literal}\"\n}}\n\n"
        ));
    }
    std::fs::write(dir.join("src").join("main.mw"), source).expect("write source");
}

/// The image byte ceiling reads the same from `check` whichever owner reports it: the
/// settled-body stop inside the drive, or the encoder's verdict over a finished program
/// whose string pool alone is too large. Both are the one bound, in its own words.
#[test]
fn check_reports_the_image_byte_ceiling_in_one_sentence_from_either_owner() {
    for (name, project) in [
        ("check-capacity-stop", over_image_bytes_project as fn(&Path)),
        (
            "check-string-pool",
            over_image_bytes_through_strings_project,
        ),
    ] {
        let dir = TempDir::new(name);
        project(&dir.root);
        let checked = run_in(&dir.root, &["check", "."]);
        assert!(!checked.status.success(), "{name}");
        assert!(checked.stdout.is_empty(), "{name}");
        assert_eq!(
            String::from_utf8(checked.stderr).expect("utf8 stderr"),
            "cli.compiler_resource_limit: the compiler reached a fixed resource limit: the \
             program image is too large\n",
            "{name}"
        );
    }
}
