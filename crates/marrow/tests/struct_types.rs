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
    assert!(summary.contains(r#""total":7"#), "{summary}");
}

/// A field read reaches the constructed value through the VM: `run` on an export
/// that builds a struct and returns one field yields that field's value.
#[test]
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

/// A struct field body that is not the bare `name: scalar` form — a group, a
/// keyed field, the `required` keyword, an optional type, or a non-scalar type —
/// is a typed `check.unsupported` at the offending member.
#[test]
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
        // A non-scalar (struct) field type.
        "struct A\n\x20   x: int\nstruct B\n\x20   a: A\n\npub fn f(): int\n\x20   return 1\n",
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

/// A struct is not yet admitted as a parameter or return type: each position is
/// `check.unsupported`, as the reference documents, until image return-shape
/// growth lands.
#[test]
fn structs_are_unsupported_in_param_and_return_positions() {
    for source in [
        // Parameter type.
        "struct P\n\x20   x: int\n\nfn g(p: P): int\n\x20   return 1\n\npub fn f(): int\n\x20   return 1\n",
        // Return type.
        "struct P\n\x20   x: int\n\npub fn make(): P\n\x20   return P(x: 1)\n",
    ] {
        let temp = TempDir::new("struct-position");
        project(&temp, source);
        let export = if source.contains("make") { "make" } else { "f" };
        let output = run_in(&temp, &["run", export, "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.unsupported""#),
            "{source:?}: {stdout}"
        );
    }
}

/// A dense struct and a durable resource coexist: the struct is a value the VM
/// constructs and reads, while the resource is written under a transaction, both
/// verifying in one image.
#[test]
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
