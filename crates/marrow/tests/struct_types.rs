//! End-to-end dense-`struct` tests: `struct Name` with `name: Type` fields
//! travels the real production path (capture -> compile -> encode -> verify ->
//! VM) through the built binary, via the `struct_types` conformance fixture and
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
        let root = std::env::temp_dir().join(format!(
            "marrow-c02-struct-{name}-{}-{nanos}",
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
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance/struct_types")
}

/// The struct conformance fixture passes end to end: named-only construction,
/// order-independent field arguments, value/copy semantics through locals and a
/// `var` rebind, field reads across module boundaries, several scalar field
/// types, and an alias-typed field all report `passed` through the production
/// path.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn struct_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "struct fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":15"#), "{summary}");
}

/// A field read reaches the constructed value through the VM: `run` on an export
/// that builds a struct and returns one field yields that field's value.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_field_read_flows_through_the_vm() {
    let temp = TempDir::new("field-read");
    project(
        &temp,
        "struct Point\n\
         \x20   x: int\n\
         \x20   y: int\n\
         \n\
         pub fn originX(): int\n\
         \x20   const p = Point(x: 3, y: 4)\n\
         \x20   return p.x\n",
    );
    let output = run_in(&temp, &["run", "originX", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":3"#), "{stdout}");
}

/// A malformed construction — an unknown field, a missing field, a duplicated
/// field, a positional (unnamed) argument, or a wrong-typed field — is a typed
/// `check.type` at the literal.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_malformed_construction_is_a_check_type_diagnostic() {
    for body in [
        "const p = Point(x: 1, z: 2)",
        "const p = Point(x: 1)",
        "const p = Point(x: 1, x: 2)",
        "const p = Point(1, 2)",
        "const p = Point(x: \"s\", y: 2)",
    ] {
        let temp = TempDir::new("bad-construct");
        project(
            &temp,
            &format!(
                "struct Point\n\
                 \x20   x: int\n\
                 \x20   y: int\n\
                 \n\
                 pub fn f(): int\n\
                 \x20   {body}\n\
                 \x20   return 0\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{body}: {stdout}"
        );
    }
}

/// Reading a field a struct does not declare is a typed `check.type`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn reading_an_unknown_field_is_a_check_type_diagnostic() {
    let temp = TempDir::new("unknown-field");
    project(
        &temp,
        "struct Point\n\
         \x20   x: int\n\
         \n\
         pub fn f(): int\n\
         \x20   const p = Point(x: 1)\n\
         \x20   return p.z\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// A struct field body that is not the bare `name: Type` form over a value type —
/// a group, a keyed field, the `required` keyword, an optional type, or an unknown
/// type — is a typed `check.unsupported` at the offending member. A struct-typed or
/// enum-typed field is admitted (covered by the nesting tests), so the rejected
/// non-scalar case here is an unknown type name.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_non_bare_scalar_field_is_a_check_unsupported_diagnostic() {
    for source in [
        // `required` keyword.
        "struct P\n\x20   required x: int\n\npub fn f(): int\n\x20   return 1\n",
        // A group.
        "struct P\n\x20   x: int\n\x20   g\n\x20       y: int\n\npub fn f(): int\n\x20   return 1\n",
        // A keyed field.
        "struct P\n\x20   scores(k: string): int\n\npub fn f(): int\n\x20   return 1\n",
        // An optional field type.
        "struct P\n\x20   x: int?\n\npub fn f(): int\n\x20   return 1\n",
        // An unknown field type name.
        "struct B\n\x20   a: Nonexistent\n\npub fn f(): int\n\x20   return 1\n",
    ] {
        let temp = TempDir::new("non-bare-field");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.unsupported""#),
            "{source:?}: {stdout}"
        );
    }
}

/// A struct name that collides with an alias, nominal, resource, or another
/// struct is a `check.name_conflict`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_name_collision_is_a_check_name_conflict() {
    for source in [
        "alias P = int\nstruct P\n\x20   x: int\n\npub fn f(): int\n\x20   return 1\n",
        "type P: int in 0..=1\nstruct P\n\x20   x: int\n\npub fn f(): int\n\x20   return 1\n",
        "struct P\n\x20   x: int\nresource P\n\x20   required y: int\n\npub fn f(): int\n\x20   return 1\n",
        "struct P\n\x20   x: int\nstruct P\n\x20   y: int\n\npub fn f(): int\n\x20   return 1\n",
    ] {
        let temp = TempDir::new("name-conflict");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.name_conflict""#),
            "{source:?}: {stdout}"
        );
    }
}

/// A struct is admitted as a parameter and a return type: it travels by value into
/// and out of a function through the production path. An export returning a struct
/// renders it as a JSON object (keys ascending) under `--format jsonl` and as
/// `{field: value, ...}` in text.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_returned_struct_renders_through_the_run_path() {
    let temp = TempDir::new("struct-return");
    project(
        &temp,
        "struct Point\n\
         \x20   x: int\n\
         \x20   y: int\n\
         \n\
         fn shift(p: Point, dx: int): Point\n\
         \x20   return Point(x: p.x + dx, y: p.y)\n\
         \n\
         pub fn moved(): Point\n\
         \x20   return shift(Point(x: 1, y: 2), 10)\n",
    );
    let jsonl = run_in(&temp, &["run", "moved", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":{"x":11,"y":2}"#), "{stdout}");

    let text = run_in(&temp, &["run", "moved"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("{x: 11, y: 2}"), "{stdout}");
}

/// A struct has no command-line spelling, so an export taking a struct parameter
/// cannot be run from the terminal: the argument decode is a usage error (exit 2).
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_argument_cannot_be_passed_on_the_command_line() {
    let temp = TempDir::new("struct-arg");
    project(
        &temp,
        "struct Point\n\
         \x20   x: int\n\
         \n\
         pub fn takesPoint(p: Point): int\n\
         \x20   return p.x\n",
    );
    let output = run_in(&temp, &["run", "takesPoint", "--", "5"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

/// A resource record is still not admitted as a return type: that vertical is
/// deferred, so it remains a `check.unsupported` diagnostic.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_resource_return_is_still_unsupported() {
    let temp = TempDir::new("resource-return");
    project(
        &temp,
        "resource Book\n\
         \x20   required title: string\n\
         \n\
         pub fn make(): Book\n\
         \x20   return Book(title: \"t\")\n",
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// A struct field may itself be a struct: a nested value constructs, reads through
/// two field hops, and renders as nested JSON. Behind the acyclicity proof, nesting
/// is admitted with no depth restriction other than the value-graph having no cycle.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_field_may_be_a_struct() {
    let temp = TempDir::new("nested-struct");
    project(
        &temp,
        "struct Inner\n\
         \x20   v: int\n\
         struct Outer\n\
         \x20   inner: Inner\n\
         \x20   tag: int\n\
         \n\
         pub fn sum(): int\n\
         \x20   const o = Outer(inner: Inner(v: 7), tag: 3)\n\
         \x20   return o.inner.v + o.tag\n\
         \n\
         pub fn whole(): Outer\n\
         \x20   return Outer(inner: Inner(v: 9), tag: 1)\n",
    );
    let sum = run_in(&temp, &["run", "sum", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&sum.stdout);
    assert!(sum.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":10"#), "{stdout}");

    let whole = run_in(&temp, &["run", "whole", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&whole.stdout);
    assert!(whole.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""data":{"inner":{"v":9},"tag":1}"#),
        "{stdout}"
    );
}

/// A struct field may name a struct declared later in the file: because every value
/// type is declared before any field is resolved, a forward reference resolves
/// regardless of declaration order. The chain `A -> B -> C` is acyclic and travels
/// through the VM.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_field_may_name_a_later_declared_struct() {
    let temp = TempDir::new("forward-ref");
    project(
        &temp,
        "struct A\n\
         \x20   b: B\n\
         struct B\n\
         \x20   c: C\n\
         struct C\n\
         \x20   v: int\n\
         \n\
         pub fn f(): int\n\
         \x20   const a = A(b: B(c: C(v: 42)))\n\
         \x20   return a.b.c.v\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":42"#), "{stdout}");
}

/// A struct field may be a user enum: the field constructs, and a `match` over the
/// field read resolves against the enum's members (the field-derived scrutinee
/// keeps its enum identity through `FieldGet`).
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_field_may_be_an_enum_and_match_over_the_field_read() {
    let temp = TempDir::new("struct-enum-field");
    project(
        &temp,
        "enum Color\n\
         \x20   red\n\
         \x20   green\n\
         struct Pen\n\
         \x20   tint: Color\n\
         \n\
         pub fn name(): string\n\
         \x20   const p = Pen(tint: Color::green)\n\
         \x20   match p.tint\n\
         \x20       red\n\
         \x20           return \"r\"\n\
         \x20       green\n\
         \x20           return \"g\"\n",
    );
    let output = run_in(&temp, &["run", "name", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"g""#), "{stdout}");
}

/// A value type that contains itself directly or transitively is a typed
/// `check.recursion` at each struct on the cycle (naming the cycle path), never a
/// silent infinite type or a deferred artifact rejection.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_value_type_cycle_is_a_check_recursion_diagnostic() {
    for source in [
        // Self-reference.
        "struct Node\n\x20   next: Node\n\npub fn f(): int\n\x20   return 1\n",
        // Two-struct cycle.
        "struct A\n\x20   b: B\nstruct B\n\x20   a: A\n\npub fn f(): int\n\x20   return 1\n",
        // A cycle routed through an `Option` field (a `some(A)` reaches A).
        "struct A\n\x20   v: int\n\x20   me: Option[A]\n\npub fn f(): int\n\x20   return 1\n",
    ] {
        let temp = TempDir::new("value-cycle");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.recursion""#),
            "{source:?}: {stdout}"
        );
    }
}

/// A dense struct and a durable resource coexist: the struct is a value the VM
/// constructs and reads, while the resource is written under a transaction, both
/// verifying in one image.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_and_a_resource_coexist() {
    let temp = TempDir::new("coexist");
    project(
        &temp,
        "struct Point\n\
         \x20   x: int\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \n\
         store ^books(id: int): Book\n\
         \n\
         pub fn pointX(): int\n\
         \x20   const p = Point(x: 5)\n\
         \x20   return p.x\n\
         \n\
         pub fn writer(id: int)\n\
         \x20   transaction\n\
         \x20       ^books(id).title = \"t\"\n",
    );
    let output = run_in(&temp, &["run", "pointX", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":5"#), "{stdout}");
}
