//! End-to-end resource-value function-boundary tests: a resource value is an
//! ordinary by-value value. It is named by a `const`/`var` annotation, passed to a
//! function parameter, and returned from a function, sharing the record
//! representation. Passing and returning copy by value, so a callee's mutation
//! never reaches the caller. A resource carrying a `group` crosses whole. The
//! `resource_values` conformance fixture travels the production path (capture ->
//! compile -> encode -> verify -> VM) through the built binary; inline projects pin
//! the boundary of the admitted subset (an optional resource stays refused).

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
            "marrow-resource-values-{name}-{}-{nanos}",
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

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance/resource_values")
}

/// The resource-value conformance fixture passes end to end: a resource
/// annotation, a resource parameter, a resource return, the copy-part-and-save-back
/// helper shape (group leaf included), and value semantics across the call.
#[test]
fn resource_value_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "resource-value fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":6"#), "{summary}");
}

/// A resource value passed by value is not aliased: a mutation on the callee's
/// local working copy leaves the caller's binding unchanged. The whole run travels
/// the production path so the value semantics are observed at runtime, not asserted
/// structurally.
#[test]
fn a_callee_mutation_does_not_alias_the_caller_resource() {
    let temp = TempDir::new("no-alias");
    project(
        &temp,
        r#"module main

resource Book {
    required title: string
    subtitle: string
}

fn mutateCopy(b: Book): Book {
    var working = b
    working.subtitle = "changed"
    return working
}

pub fn make(): string {
    const original = Book(title: "t")
    const other = mutateCopy(original)
    return original.subtitle ?? "untouched"
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "no-alias run must succeed: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":"untouched""#),
        "the caller's resource must survive the callee's mutation: {stdout}"
    );
}

/// The boundary of the admitted subset: an *optional* resource value
/// (`Book?`) is not composed today — a resource record is not a value argument to
/// the reserved `Option` template — so an optional-resource parameter is a typed
/// `check.unsupported` rejection rather than a silent acceptance. Ledgered so the
/// lane that composes optional resources flips this expectation.
#[test]
fn an_optional_resource_parameter_stays_refused() {
    let temp = TempDir::new("optional-resource-param");
    project(
        &temp,
        r#"module main

resource Book {
    required title: string
}

pub fn describe(b: Book?): string {
    return "x"
}

pub fn make(): string {
    return "x"
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "an optional-resource parameter must be refused: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.unsupported""#),
        "expected a check.unsupported for the optional-resource parameter: {stdout}"
    );
}
