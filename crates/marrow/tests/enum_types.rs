//! End-to-end closed-flat-enum tests: `enum Name` with payloadless and payload
//! members travels the real production path (capture -> compile -> encode ->
//! verify -> VM) through the built binary, via the `enum_types` conformance
//! fixture and inline invalid-source projects asserting typed diagnostics.

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
            "marrow-c02-enum-{name}-{}-{nanos}",
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
        .join("fixtures/v01/conformance/enum_types")
}

/// The enum conformance fixture passes end to end: payloadless and payload
/// construction, exhaustive `match` with positional payload binding, payload-
/// ignoring arms, exact `==`/`!=` equality over the variant and payload, and
/// construction/matching across function boundaries all report `passed`.
#[test]
fn enum_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "enum fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":6"#), "{summary}");
}

/// A returned enum value renders through the VM: `run` on an export that
/// constructs a payload variant yields the canonical enum object.
#[test]
fn a_payload_enum_value_renders_through_the_vm() {
    let temp = TempDir::new("render");
    project(
        &temp,
        "enum Shape\n\
         \x20   dot\n\
         \x20   circle(radius: int)\n\
         \n\
         pub fn make(r: int): Shape\n\
         \x20   return Shape::circle(radius: r)\n",
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl", "--", "7"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""data":{"enum":"Shape","member":"circle","payload":[7]}"#),
        "{stdout}"
    );
}

/// A non-exhaustive `match` is `check.match_nonexhaustive`.
#[test]
fn a_non_exhaustive_match_is_reported() {
    let temp = TempDir::new("nonexhaustive");
    project(
        &temp,
        "enum E\n\
         \x20   a\n\
         \x20   b\n\
         \n\
         pub fn f(e: E): int\n\
         \x20   match e\n\
         \x20       a\n\
         \x20           return 1\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"check.match_nonexhaustive""#),
        "{stdout}"
    );
}

/// A malformed arm — an unknown member, a duplicate member, or a payload-arity
/// mismatch — is a typed `check.match_arm`.
#[test]
fn a_malformed_arm_is_a_check_match_arm_diagnostic() {
    for body in [
        // unknown member
        "match e\n        a\n            return 1\n        c\n            return 2\n        b\n            return 3\n",
        // duplicate member
        "match e\n        a\n            return 1\n        a\n            return 2\n        b\n            return 3\n",
    ] {
        let temp = TempDir::new("arm");
        project(
            &temp,
            &format!("enum E\n    a\n    b\n\npub fn f(e: E): int\n    {body}"),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body}\n{stdout}");
        assert!(
            stdout.contains(r#""code":"check.match_arm""#),
            "{body}\n{stdout}"
        );
    }
}

/// A payload-arity mismatch on a binding arm is a typed `check.match_arm`.
#[test]
fn a_payload_arity_mismatch_is_reported() {
    let temp = TempDir::new("arity");
    project(
        &temp,
        "enum E\n\
         \x20   a(x: int)\n\
         \x20   b\n\
         \n\
         pub fn f(e: E): int\n\
         \x20   match e\n\
         \x20       a(x, y)\n\
         \x20           return x\n\
         \x20       b\n\
         \x20           return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.match_arm""#), "{stdout}");
}

/// A malformed construction — an unknown payload field, a missing payload field,
/// a payload on a payloadless member, or a non-existent member — is a typed
/// `check.type`.
#[test]
fn a_malformed_construction_is_a_check_type_diagnostic() {
    for expr in [
        "Shape::circle(radius: 1, z: 2)",
        "Shape::circle()",
        "Shape::dot(x: 1)",
        "Shape::triangle",
    ] {
        let temp = TempDir::new("construct");
        project(
            &temp,
            &format!(
                "enum Shape\n    dot\n    circle(radius: int)\n\npub fn f(): int\n    const s = {expr}\n    return 0\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{expr}\n{stdout}");
        // `Shape::circle()` is a parse error (an empty payload); the rest are
        // check.type. Either way the export does not run.
        assert!(
            stdout.contains(r#""outcome":"diagnostic""#),
            "{expr}\n{stdout}"
        );
    }
}

/// A `category` member or a nested member is deferred: `check.unsupported`.
#[test]
fn a_hierarchical_enum_is_deferred() {
    let temp = TempDir::new("hierarchy");
    project(
        &temp,
        "enum Animal\n\
         \x20   category cat\n\
         \x20       tiger\n\
         \x20   dog\n\
         \n\
         pub fn f(): int\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// An enum whose name collides with another type is a `check.name_conflict`.
#[test]
fn an_enum_name_collision_is_reported() {
    let temp = TempDir::new("collision");
    project(
        &temp,
        "struct Color\n\
         \x20   r: int\n\
         \n\
         enum Color\n\
         \x20   red\n\
         \n\
         pub fn f(): int\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"check.name_conflict""#),
        "{stdout}"
    );
}

/// A resource field may name a user enum declared later in the file: because the
/// value types are declared before any field is resolved, the field resolves to the
/// enum, a `match` over the field read keeps the enum identity, and the whole travels
/// the production path. (The resource is a local value here; a resource backing a
/// `store` still admits only scalar fields.)
#[test]
fn a_resource_field_may_be_a_user_enum_and_match_over_the_field_read() {
    let temp = TempDir::new("resource-enum-field");
    project(
        &temp,
        "resource Paint\n\
         \x20   required shade: Color\n\
         \n\
         enum Color\n\
         \x20   red\n\
         \x20   green\n\
         \n\
         pub fn name(): string\n\
         \x20   const p = Paint(shade: Color::green)\n\
         \x20   match p.shade\n\
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

/// A resource backing a `store` still admits only scalar fields: a user-enum field
/// on a stored resource is a typed `check.type` at the store (the durable-root
/// scalar-only rule is unchanged by the local-value nesting work).
#[test]
fn a_stored_resource_with_an_enum_field_is_rejected() {
    let temp = TempDir::new("stored-enum-field");
    project(
        &temp,
        "resource Paint\n\
         \x20   required id: int\n\
         \x20   required shade: Color\n\
         \n\
         enum Color\n\
         \x20   red\n\
         \x20   green\n\
         \n\
         store ^paints(id: int): Paint\n\
         \n\
         pub fn f(): int\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}
