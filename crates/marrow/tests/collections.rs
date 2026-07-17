//! End-to-end `List[T]`/`Map[K, V]` tests: finite collection values travel the real
//! production path (capture -> compile -> encode -> verify -> VM) through the built
//! binary, via the `collections` conformance fixture and inline invalid-source
//! projects asserting typed diagnostics.

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
            "marrow-c03-coll-{name}-{}-{nanos}",
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
        .join("fixtures/v01/conformance/collections")
}

/// The collection conformance fixture passes end to end: list construction, append,
/// iteration, length/isEmpty, map insert/get/replace, key-ordered iteration, nested
/// collections, struct/enum element values, and the collection-returning text floor
/// (`split`/`lines`/`join`) all report `passed` through the production path.
#[test]
fn collection_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "collection fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":15"#), "{summary}");
}

/// A returned list renders as a JSON array (insertion order) under `--format jsonl`
/// and as `[a, b, ...]` in text.
#[test]
fn a_returned_list_renders_through_the_run_path() {
    let temp = TempDir::new("list-return");
    project(
        &temp,
        r#"module main

pub fn nums(): List<int> {
    var xs: List<int> = List()
    xs = append(xs, 1)
    xs = append(xs, 2)
    return xs
}
"#,
    );
    let jsonl = run_in(&temp, &["run", "nums", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":[1,2]"#), "{stdout}");

    let text = run_in(&temp, &["run", "nums"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("[1, 2]"), "{stdout}");
}

/// A returned map renders as a JSON object with keys in ascending order under
/// `--format jsonl` and as `[k: v, ...]` in text.
#[test]
fn a_returned_map_renders_in_ascending_key_order() {
    let temp = TempDir::new("map-return");
    project(
        &temp,
        r#"module main

pub fn scores(): Map<string, int> {
    var m: Map<string, int> = Map()
    m = insert(m, "grace", 12)
    m = insert(m, "ada", 10)
    return m
}
"#,
    );
    let jsonl = run_in(&temp, &["run", "scores", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""data":{"ada":10,"grace":12}"#),
        "{stdout}"
    );

    let text = run_in(&temp, &["run", "scores"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("[ada: 10, grace: 12]"), "{stdout}");
}

/// Appending past the aggregate-byte bound faults with `run.collection_limit`, the
/// law-9 typed runtime fault, rather than allocating unboundedly.
#[test]
fn exceeding_the_aggregate_bound_faults() {
    let output = run_in(
        &fixture_dir(),
        &["run", "overflowAggregate", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"run.collection_limit""#),
        "{stdout}"
    );
}

/// Joining a list whose concatenation exceeds the text ceiling faults with
/// `run.text_limit`, the bounded-allocation guard on `join`, rather than
/// materializing an unbounded string.
#[test]
fn exceeding_the_join_text_ceiling_faults() {
    let output = run_in(
        &fixture_dir(),
        &["run", "overflowJoin", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"run.text_limit""#), "{stdout}");
}

/// A bare `List()`/`Map()` with no expected type cannot infer its instantiation and
/// is a typed `check.type`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_bare_constructor_without_expected_type_is_a_check_type() {
    for body in ["const xs = List()", "const m = Map()"] {
        let temp = TempDir::new("bare-ctor");
        project(
            &temp,
            &format!("module main\n\npub fn f(): int\n\x20   {body}\n\x20   return 0\n"),
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

/// A non-key map key type (a struct), an `append` on a map, a `get` on a list, and a
/// wrong-typed element are typed diagnostics, not silent acceptance.
#[test]
fn misused_collection_operations_are_typed_diagnostics() {
    // A struct key type is not admitted: `check.unsupported` at the annotation.
    let cases: [(&str, &str); 4] = [
        (
            r#"struct P {
    x: int
}

pub fn f(): int {
    const m: Map<P, int> = Map()
    return 0
}
"#,
            "check.unsupported",
        ),
        (
            r#"pub fn f(): int {
    var m: Map<string, int> = Map()
    m = append(m, 1)
    return 0
}
"#,
            "check.unsupported",
        ),
        (
            r#"pub fn f(): int {
    var xs: List<int> = List()
    const v = get(xs, 0)
    return 0
}
"#,
            "check.unsupported",
        ),
        (
            r#"pub fn f(): int {
    var xs: List<int> = List()
    xs = append(xs, "s")
    return 0
}
"#,
            "check.type",
        ),
    ];
    for (source, code) in cases {
        let temp = TempDir::new("misuse");
        let full = format!("module main\n\n{source}");
        project(&temp, &full);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(&format!(r#""code":"{code}""#)),
            "{source:?} expected {code}: {stdout}"
        );
    }
}

/// `List` and `Map` are reserved type names; redeclaring one is a
/// `check.name_conflict`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn redeclaring_a_reserved_collection_name_is_a_conflict() {
    for name in ["List", "Map"] {
        let temp = TempDir::new("reserved");
        project(
            &temp,
            &format!(
                "module main\n\nstruct {name}\n\x20   x: int\n\npub fn f(): int\n\x20   return 0\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{name} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.name_conflict""#),
            "{name}: {stdout}"
        );
    }
}

/// The collection-returning text floor built-ins `split`/`lines`/`join` are reserved
/// value-level names, so a colliding value declaration is a `check.name_conflict`
/// (the same closed-floor discipline as `isEmpty`/`contains`/`trim`).
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn redeclaring_a_text_floor_builtin_is_a_conflict() {
    for name in ["split", "lines", "join"] {
        let temp = TempDir::new("reserved-floor");
        project(
            &temp,
            &format!(
                "module main\n\nfn {name}(): int\n\x20   return 0\n\npub fn f(): int\n\x20   return 0\n"
            ),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{name} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.name_conflict""#),
            "{name}: {stdout}"
        );
    }
}

/// The language admits no top-level `collection == collection` operator: equality
/// over two lists (or two maps) is a typed `check.type`, not silent acceptance. A
/// collection reached inside a compared struct or enum payload still participates in
/// that aggregate's equality; only the bare collection comparison is rejected. This
/// pins the recorded decision that collection `==` stays a typed check error rather
/// than a language operator.
#[test]
fn a_top_level_collection_equality_is_a_check_type() {
    let cases = [
        r#"pub fn f(): bool {
    var a: List<int> = List()
    var b: List<int> = List()
    return a == b
}
"#,
        r#"pub fn f(): bool {
    var a: Map<int, int> = Map()
    var b: Map<int, int> = Map()
    return a == b
}
"#,
    ];
    for source in cases {
        let temp = TempDir::new("coll-eq");
        project(&temp, &format!("module main\n\n{source}"));
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source:?} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{source:?}: {stdout}"
        );
    }
}

/// `join` on a list whose element type is not `string` is a typed `check.unsupported`
/// — the text floor joins only a list of string.
#[test]
fn join_on_a_non_string_list_is_unsupported() {
    let temp = TempDir::new("join-misuse");
    project(
        &temp,
        r#"module main

pub fn f(): string {
    var xs: List<int> = List()
    return join(xs, ",")
}
"#,
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}
