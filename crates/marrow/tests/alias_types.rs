//! End-to-end `alias` tests: the transparent `alias Name = Type` declaration
//! travels the real production path (capture → compile → encode → verify → VM)
//! through the built binary, via the `alias_types` conformance fixture and
//! inline invalid-source projects asserting typed diagnostics.

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
        .join("fixtures/v01/conformance/alias_types")
}

/// The alias conformance fixture passes end to end: every `test` declaration
/// using aliases in parameter, return, constant, optional, and resource-field
/// positions reports `passed` through the production path.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn alias_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "alias fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":5"#), "{summary}");
}

/// A cyclic alias chain is a typed `check.recursion` diagnostic, reported once
/// per alias on the cycle, at check time.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_cyclic_alias_chain_is_a_check_recursion_diagnostic() {
    let temp = TempDir::new("alias-cycle");
    project(
        &temp,
        "alias A = B\n\
         alias B = A\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a cycle must fail: {stdout}");
    // One typed record per alias on the cycle, at each declaration's name.
    let recursion_lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains(r#""code":"check.recursion""#))
        .collect();
    assert_eq!(recursion_lines.len(), 2, "{stdout}");
    assert!(recursion_lines[0].contains(r#""line":1"#), "{stdout}");
    assert!(recursion_lines[1].contains(r#""line":2"#), "{stdout}");
}

/// A self-referential alias is the one-element cycle.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_self_referential_alias_is_a_check_recursion_diagnostic() {
    let temp = TempDir::new("alias-self");
    project(
        &temp,
        "alias Loop = Loop?\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a cycle must fail: {stdout}");
    assert!(stdout.contains("check.recursion"), "{stdout}");
}

/// Two aliases with one name collide, as do an alias and a resource: names a
/// type annotation resolves against are unique across the project.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn duplicate_alias_names_are_name_conflicts() {
    let temp = TempDir::new("alias-dup");
    project(
        &temp,
        "alias Count = int\n\
         alias Count = string\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a duplicate must fail: {stdout}");
    assert!(stdout.contains("check.name_conflict"), "{stdout}");

    let temp = TempDir::new("alias-resource-clash");
    project(
        &temp,
        "resource Item\n\
         \x20   required count: int\n\
         \n\
         alias Item = int\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "an alias/resource clash must fail: {stdout}"
    );
    assert!(stdout.contains("check.name_conflict"), "{stdout}");
}

/// An alias whose expansion names no known type is a typed `check.type`
/// diagnostic at the alias declaration, even when the alias is unused.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_alias_to_an_unknown_type_is_a_check_type_diagnostic() {
    let temp = TempDir::new("alias-unknown");
    project(
        &temp,
        "alias Broken = Missing\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "unknown target must fail: {stdout}"
    );
    let diagnostic = stdout
        .lines()
        .find(|line| line.contains(r#""code":"check.type""#))
        .unwrap_or_else(|| panic!("no check.type record: {stdout}"));
    assert!(diagnostic.contains(r#""line":1"#), "{stdout}");
}

/// Alias transparency does not relax the optional-nesting rule: `M?` where `M`
/// expands to `int?` is still a doubled optional and rejects.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_alias_cannot_smuggle_a_nested_optional() {
    let temp = TempDir::new("alias-nested-opt");
    project(
        &temp,
        "alias MaybeInt = int?\n\
         \n\
         pub fn f(v: bool): MaybeInt?\n\
         \x20   return absent\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a doubled optional must fail: {stdout}"
    );
    assert!(stdout.contains("check."), "{stdout}");
}

/// A keyword cannot name an alias; the parser reports it at the declaration.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_keyword_alias_name_is_a_parse_error() {
    let temp = TempDir::new("alias-keyword");
    project(
        &temp,
        "alias int = string\n\
         \n\
         pub fn f(): int\n\
         \x20   return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a keyword name must fail: {stdout}"
    );
    assert!(stdout.contains("parse.syntax"), "{stdout}");
}
