//! End-to-end `alias` tests: the transparent `alias Name = Type` declaration
//! travels the real production path (capture → compile → encode → verify → VM)
//! through the built binary, via the `alias_types` conformance fixture and
//! inline invalid-source projects asserting typed diagnostics.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

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

fn captured_source(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture source")
}

fn source_diagnostics(source: &str) -> Vec<SourceDiagnostic> {
    match compile(&captured_source(source)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
        Ok(_) => panic!("expected source diagnostics, got a compiled image"),
    }
}

/// The alias conformance fixture passes end to end: every `test` declaration
/// using aliases in parameter, return, constant, optional, and resource-field
/// positions reports `passed` through the production path.
#[test]
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
fn a_cyclic_alias_chain_is_a_check_recursion_diagnostic() {
    let temp = TempDir::new("alias-cycle");
    project(
        &temp,
        r#"alias A = B

alias B = A

pub fn f(): int {
    return 1
}
"#,
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
    assert!(recursion_lines[1].contains(r#""line":3"#), "{stdout}");
}

/// A self-referential alias is the one-element cycle.
#[test]
fn a_self_referential_alias_is_a_check_recursion_diagnostic() {
    let temp = TempDir::new("alias-self");
    project(
        &temp,
        r#"alias Loop = Loop?

pub fn f(): int {
    return 1
}
"#,
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a cycle must fail: {stdout}");
    assert!(stdout.contains("check.recursion"), "{stdout}");
}

/// Two aliases with one name collide, as do an alias and a resource: names a
/// type annotation resolves against are unique across the project.
#[test]
fn duplicate_alias_names_are_name_conflicts() {
    let temp = TempDir::new("alias-dup");
    project(
        &temp,
        r#"alias Count = int

alias Count = string

pub fn f(): int {
    return 1
}
"#,
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "a duplicate must fail: {stdout}");
    assert!(stdout.contains("check.name_conflict"), "{stdout}");

    let temp = TempDir::new("alias-resource-clash");
    project(
        &temp,
        r#"resource Item {
    required count: int
}

alias Item = int

pub fn f(): int {
    return 1
}
"#,
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
fn an_alias_to_an_unknown_type_is_a_check_type_diagnostic() {
    let temp = TempDir::new("alias-unknown");
    project(
        &temp,
        r#"alias Broken = Missing

pub fn f(): int {
    return 1
}
"#,
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
fn an_alias_cannot_smuggle_a_nested_optional() {
    let temp = TempDir::new("alias-nested-opt");
    project(
        &temp,
        r#"alias MaybeInt = int?

pub fn f(v: bool): MaybeInt? {
    return absent
}
"#,
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

/// SCC membership is independent of reachability into a cycle. Generic
/// application heads are not alias references; only their arguments are.
#[test]
fn alias_cycle_membership_order_and_spans_follow_the_alias_owner() {
    let diagnostics = source_diagnostics(
        r#"alias Zed = Alpha

alias Alpha = Zed

alias Self = Self?

alias Tail = Alpha

alias Plain = int

alias PlainAlias = Plain

alias Head = Wrapped

alias Wrapped = Head<int>

pub fn f(value: PlainAlias): PlainAlias {
    return value
}
"#,
    );
    let observed: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| {
            let span = diagnostic.span();
            (
                diagnostic.code,
                span.start_byte,
                span.end_byte,
                span.line,
                span.column,
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            ("check.recursion", 25, 30, 3, 7),
            ("check.recursion", 44, 48, 5, 7),
            ("check.recursion", 6, 9, 1, 7),
            ("check.type", 58, 76, 7, 1),
            ("check.unsupported", 123, 143, 13, 1),
            ("check.unsupported", 145, 170, 15, 1),
        ],
        "observed diagnostics: {diagnostics:#?}"
    );
}

/// A small accepted alias chain freezes the pre-repair canonical image bytes by
/// their domain-separated image identity and exact encoded length.
#[test]
fn accepted_alias_image_bytes_remain_frozen() {
    let compiled = compile(&captured_source(
        r#"alias Count = int

alias OtherCount = Count

pub fn identity(value: OtherCount): Count {
    return value
}
"#,
    ))
    .expect("accepted alias fixture compiles");
    assert_eq!(
        (compiled.image.bytes.len(), compiled.image.image_id.to_hex()),
        (
            240,
            "1eed2c79352f2e8cceb830c7115a248d63f3f93df2019a5d6a102ed50de92585".to_string(),
        ),
        "accepted alias fixture image identity changed"
    );
}
