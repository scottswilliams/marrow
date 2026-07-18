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
    assert!(summary.contains(r#""total":15"#), "{summary}");
}

/// A field read reaches the constructed value through the VM: `run` on an export
/// that builds a struct and returns one field yields that field's value.
#[test]
fn a_struct_field_read_flows_through_the_vm() {
    let temp = TempDir::new("field-read");
    project(
        &temp,
        r#"struct Point {
    x: int
    y: int
}

pub fn originX(): int {
    const p = Point(x: 3, y: 4)
    return p.x
}
"#,
    );
    let output = run_in(&temp, &["run", "originX", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":3"#), "{stdout}");
}

/// A record-typed optional is a valid `VacantLoad` operand end to end: the
/// verifier admits an optional record just as it admits an optional scalar, enum,
/// or collection. Two lowerings reach it — an explicit `absent` bound to a
/// struct-typed optional local, and a sparse struct-typed resource field omitted
/// at construction — and both must compile, verify, and run.
#[test]
fn a_record_typed_optional_vacant_load_verifies_and_runs() {
    // D1: an explicit `absent` in a struct-typed optional local.
    let temp = TempDir::new("record-optional-absent");
    project(
        &temp,
        r#"struct Point {
    x: int
    y: int
}

pub fn pick(hit: bool): int {
    var p: Point? = absent
    if hit {
        p = Point(x: 1, y: 2)
    }
    if const q = p {
        return q.x
    }
    return -1
}
"#,
    );
    let absent = run_in(&temp, &["run", "pick", "--format", "jsonl", "--", "false"]);
    let absent_out = String::from_utf8_lossy(&absent.stdout);
    assert!(absent.status.success(), "{absent_out}");
    assert!(absent_out.contains(r#""data":-1"#), "{absent_out}");
    let present = run_in(&temp, &["run", "pick", "--format", "jsonl", "--", "true"]);
    let present_out = String::from_utf8_lossy(&present.stdout);
    assert!(present.status.success(), "{present_out}");
    assert!(present_out.contains(r#""data":1"#), "{present_out}");

    // The twin: a sparse struct-typed resource field omitted at construction
    // defaults to a vacant optional record.
    let twin = TempDir::new("record-optional-omitted");
    project(
        &twin,
        r#"struct Addr {
    city: string
}

resource Person {
    required name: string
    addr: Addr
}

store ^people[id: int]: Person

pub fn hasAddr(): int {
    const o = Person(name: "x")
    if const a = o.addr {
        return 1
    }
    return -1
}
"#,
    );
    let output = run_in(&twin, &["run", "hasAddr", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":-1"#), "{stdout}");
}

/// `?.` reads a member through an optional composite value: absent short-circuits
/// to absent, present yields the member wrapped optional, and it chains through a
/// sparse composite field without a double wrap.
#[test]
fn optional_member_read_propagates_absence_and_chains() {
    let temp = TempDir::new("optional-member");
    project(
        &temp,
        r#"struct Inner {
    tag: string
}

struct Outer {
    label: string
    inner: Inner
}

pub fn tagOf(hit: bool): string {
    var o: Outer? = absent
    if hit {
        o = Outer(label: "L", inner: Inner(tag: "T"))
    }
    return o?.inner?.tag ?? "none"
}
"#,
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "tagOf", "--", "true"]).stdout),
        "T\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "tagOf", "--", "false"]).stdout),
        "none\n"
    );
}

/// `?.` on a value that is not optional, or is optional but not a composite, is a
/// typed `check.type` — the operator has exactly one meaning.
#[test]
fn optional_member_read_rejects_a_non_optional_or_non_composite_base() {
    for (source, entry) in [
        (
            "struct P { x: int }\npub fn f(): int {\n    const p = P(x: 3)\n    return p?.x ?? 0\n}\n",
            "f",
        ),
        (
            "pub fn f(): int {\n    var n: int? = absent\n    return n?.x ?? 0\n}\n",
            "f",
        ),
    ] {
        let temp = TempDir::new("optional-member-bad");
        project(&temp, source);
        let output = run_in(&temp, &["run", entry, "--format", "jsonl"]);
        assert!(!output.status.success(), "{source}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("check.type"), "{source}: {stdout}");
    }
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
                "struct Point {{\n\
                 \x20   x: int\n\
                 \x20   y: int\n\
                 }}\n\
                 \n\
                 pub fn f(): int {{\n\
                 \x20   {body}\n\
                 \x20   return 0\n\
                 }}\n"
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
        r#"struct Point {
    x: int
}

pub fn f(): int {
    const p = Point(x: 1)
    return p.z
}
"#,
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
fn a_non_bare_scalar_field_is_a_check_unsupported_diagnostic() {
    for source in [
        // `required` keyword.
        r#"struct P {
    required x: int
}

pub fn f(): int {
    return 1
}
"#,
        // A group.
        r#"struct P {
    x: int
    g {
        y: int
    }
}

pub fn f(): int {
    return 1
}
"#,
        // A keyed field.
        r#"struct P {
    scores[k: string]: int
}

pub fn f(): int {
    return 1
}
"#,
        // An optional field type.
        r#"struct P {
    x: int?
}

pub fn f(): int {
    return 1
}
"#,
        // An unknown field type name.
        r#"struct B {
    a: Nonexistent
}

pub fn f(): int {
    return 1
}
"#,
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
        r#"alias P = int

struct P {
    x: int
}

pub fn f(): int {
    return 1
}
"#,
        r#"type P: int in 0..=1

struct P {
    x: int
}

pub fn f(): int {
    return 1
}
"#,
        r#"struct P {
    x: int
}

resource P {
    required y: int
}

pub fn f(): int {
    return 1
}
"#,
        r#"struct P {
    x: int
}

struct P {
    y: int
}

pub fn f(): int {
    return 1
}
"#,
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
fn a_returned_struct_renders_through_the_run_path() {
    let temp = TempDir::new("struct-return");
    project(
        &temp,
        r#"struct Point {
    x: int
    y: int
}

fn shift(p: Point, dx: int): Point {
    return Point(x: p.x + dx, y: p.y)
}

pub fn moved(): Point {
    return shift(Point(x: 1, y: 2), 10)
}
"#,
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
fn a_struct_argument_cannot_be_passed_on_the_command_line() {
    let temp = TempDir::new("struct-arg");
    project(
        &temp,
        r#"struct Point {
    x: int
}

pub fn takesPoint(p: Point): int {
    return p.x
}
"#,
    );
    let output = run_in(&temp, &["run", "takesPoint", "--", "5"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

/// A resource record is admitted as a return type (RV02): it crosses the boundary
/// by value like any other record. The full boundary matrix — annotation, parameter,
/// return, value semantics — lives in the `resource_values` fixture and test.
#[test]
fn a_resource_return_is_admitted() {
    let temp = TempDir::new("resource-return");
    project(
        &temp,
        r#"resource Book {
    required title: string
}

pub fn make(): string {
    return draft().title
}

fn draft(): Book {
    return Book(title: "t")
}
"#,
    );
    let output = run_in(&temp, &["run", "make", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"t""#), "{stdout}");
}

/// A struct field may itself be a struct: a nested value constructs, reads through
/// two field hops, and renders as nested JSON. Behind the acyclicity proof, nesting
/// is admitted with no depth restriction other than the value-graph having no cycle.
#[test]
fn a_struct_field_may_be_a_struct() {
    let temp = TempDir::new("nested-struct");
    project(
        &temp,
        r#"struct Inner {
    v: int
}

struct Outer {
    inner: Inner
    tag: int
}

pub fn sum(): int {
    const o = Outer(inner: Inner(v: 7), tag: 3)
    return o.inner.v + o.tag
}

pub fn whole(): Outer {
    return Outer(inner: Inner(v: 9), tag: 1)
}
"#,
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
fn a_struct_field_may_name_a_later_declared_struct() {
    let temp = TempDir::new("forward-ref");
    project(
        &temp,
        r#"struct A {
    b: B
}

struct B {
    c: C
}

struct C {
    v: int
}

pub fn f(): int {
    const a = A(b: B(c: C(v: 42)))
    return a.b.c.v
}
"#,
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
fn a_struct_field_may_be_an_enum_and_match_over_the_field_read() {
    let temp = TempDir::new("struct-enum-field");
    project(
        &temp,
        r#"enum Color {
    red
    green
}

struct Pen {
    tint: Color
}

pub fn name(): string {
    const p = Pen(tint: Color::green)
    match p.tint {
        red => return "r"
        green => return "g"
    }
}
"#,
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
fn a_value_type_cycle_is_a_check_recursion_diagnostic() {
    for source in [
        // Self-reference.
        r#"struct Node {
    next: Node
}

pub fn f(): int {
    return 1
}
"#,
        // Two-struct cycle.
        r#"struct A {
    b: B
}

struct B {
    a: A
}

pub fn f(): int {
    return 1
}
"#,
        // A cycle routed through an `Option` field (a `some(A)` reaches A).
        r#"struct A {
    v: int
    me: Option<A>
}

pub fn f(): int {
    return 1
}
"#,
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
fn a_struct_and_a_resource_coexist() {
    let temp = TempDir::new("coexist");
    project(
        &temp,
        r#"struct Point {
    x: int
}

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn pointX(): int {
    const p = Point(x: 5)
    return p.x
}

pub fn writer(id: int) {
    transaction {
        ^books[id].title = "t"
    }
}
"#,
    );
    let output = run_in(&temp, &["run", "pointX", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":5"#), "{stdout}");
}

/// Method-call syntax on a value is rejected: member syntax reaches fields and
/// constructor paths only, so `s.trim()` is a typed `check.unsupported`. The
/// operation on a value is the free function `trim(s)`. The diagnostic's voice is
/// governed by `docs/implementation/diagnostic-voice.md`; the test pins the typed
/// code, not the prose.
#[test]
fn a_method_call_on_a_value_is_a_check_unsupported_diagnostic() {
    let temp = TempDir::new("method-call");
    project(
        &temp,
        r#"pub fn f(s: string): string {
    return s.trim()
}
"#,
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl", "--", "hi"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}
