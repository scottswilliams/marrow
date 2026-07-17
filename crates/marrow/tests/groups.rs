//! End-to-end group materialized-value tests: an unkeyed `group` block is a
//! nested sub-record value inside its resource. Its scalar leaves participate in
//! the materialized value, `entry.group.field` reads and assigns, the whole group
//! reads and copies as a value unit, and a required group descendant is a required
//! member of the containing resource. The `groups` conformance fixture travels the
//! production path (capture -> compile -> encode -> verify -> VM) through the built
//! binary; inline projects assert the completeness rejection.

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
            std::env::temp_dir().join(format!("marrow-gv01-{name}-{}-{nanos}", std::process::id()));
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
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance/groups")
}

/// The group conformance fixture passes end to end: a fresh group leaf reads
/// absent, an assignment sets a leaf present, the whole group reads and copies as
/// a value unit with value semantics, and `unset` clears a leaf.
#[test]
fn group_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "group fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":5"#), "{summary}");
}

/// Required-completeness over group descendants: a resource whose group declares a
/// `required` leaf cannot be constructed without supplying it, and the constructor
/// has no group-argument surface, so `Book(title: ...)` is a `check.type`
/// rejection naming the missing group member rather than a silent incomplete value.
#[test]
fn a_required_group_leaf_makes_construction_incomplete() {
    let temp = TempDir::new("required-group-leaf");
    project(
        &temp,
        r#"module main

resource Book {
    required title: string
    details {
        required pages: int
    }
}

pub fn make(): string {
    const b = Book(title: "x")
    return b.title
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "construction with an unsatisfiable required group leaf must be rejected: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.type""#),
        "expected a check.type rejection: {stdout}"
    );
}

/// A group with only sparse leaves constructs, and reading an unset leaf yields
/// absent through the production path.
#[test]
fn a_sparse_group_constructs_and_reads_absent() {
    let temp = TempDir::new("sparse-group");
    project(
        &temp,
        r#"module main

resource Book {
    required title: string
    details {
        pages: int
    }
}

pub fn make(): int {
    const b = Book(title: "x")
    return b.details.pages ?? 5
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "sparse group must construct: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":5"#),
        "unset group leaf must read absent (falls through to 5): {stdout}"
    );
}
