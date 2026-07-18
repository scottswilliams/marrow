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

/// An optional resource value (`Book?`) is an ordinary optional record: a binding
/// annotated `Book?` holds a present resource and reads through `if const`, exactly
/// as an optional `struct` does. Positive control for the admitted binding position.
#[test]
fn an_optional_resource_binding_is_admitted() {
    let temp = TempDir::new("optional-resource-binding");
    project(
        &temp,
        r#"module main

resource Book {
    required title: string
}

pub fn make(): string {
    const b: Book? = Book(title: "t")
    if const got = b {
        return got.title
    }
    return "none"
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "an optional-resource binding must run: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":"t""#),
        "the optional-resource binding must read present: {stdout}"
    );
}

/// A function may return `Book?`: the present and absent arms both lower, and the
/// caller reads the result through `if const`. Positive control for the admitted
/// return position.
#[test]
fn an_optional_resource_return_is_admitted() {
    let temp = TempDir::new("optional-resource-return");
    project(
        &temp,
        r#"module main

resource Book {
    required title: string
}

fn lookup(found: bool): Book? {
    if found {
        return Book(title: "t")
    }
    return absent
}

pub fn make(): string {
    if const b = lookup(true) {
        return b.title
    }
    return "none"
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "an optional-resource return must run: {stdout}"
    );
    assert!(
        stdout.contains(r#""data":"t""#),
        "the optional-resource return must read present: {stdout}"
    );
}

/// A `Book?` parameter is refused — not for a resource-specific reason, but under
/// the general rule that a parameter is a bare value; an optional parameter of any
/// type is `check.unsupported`. The boundary of the admitted parameter subset.
#[test]
fn an_optional_resource_parameter_is_refused_like_any_optional_parameter() {
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

/// The `Option<Book>` and `List<Book>` generic forms are refused in every position:
/// a resource type is not a value argument to a built-in generic template, so the
/// type spelling itself does not resolve. Each position — binding, return, and
/// parameter — is a typed `check.unsupported` rejection. Ledgered so the lane that
/// composes resources into generics flips these expectations together.
#[test]
fn generic_composition_over_a_resource_is_refused_in_every_position() {
    let cases = [
        (
            "option-binding",
            r#"module main

resource Book {
    required title: string
}

pub fn make(): string {
    const b: Option<Book> = some(Book(title: "t"))
    return "x"
}
"#,
        ),
        (
            "option-return",
            r#"module main

resource Book {
    required title: string
}

fn maybe(): Option<Book> {
    return some(Book(title: "t"))
}

pub fn make(): string {
    return "x"
}
"#,
        ),
        (
            "option-param",
            r#"module main

resource Book {
    required title: string
}

fn take(b: Option<Book>): string {
    return "x"
}

pub fn make(): string {
    return "x"
}
"#,
        ),
        (
            "list-param",
            r#"module main

resource Book {
    required title: string
}

fn take(b: List<Book>): string {
    return "x"
}

pub fn make(): string {
    return "x"
}
"#,
        ),
    ];
    for (name, source) in cases {
        let temp = TempDir::new(name);
        project(&temp, source);
        let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "{name}: generic composition over a resource must be refused: {stdout}"
        );
        assert!(
            stdout.contains(r#""code":"check.unsupported""#),
            "{name}: expected a check.unsupported: {stdout}"
        );
    }
}
