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
/// iteration, length/isEmpty, map insert/get/replace/remove, key-ordered iteration, nested
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
    assert!(summary.contains(r#""total":30"#), "{summary}");
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
    m["grace"] = 12
    m["ada"] = 10
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

/// The current local Map key domain stays aligned across the compiler, verifier,
/// VM, and both claims in the language reference.
#[test]
fn map_key_domain_and_reference_agree() {
    let accepted = TempDir::new("map-key-domain");
    project(
        &accepted,
        r#"module main

type Rank: int in 1..=8

pub fn values(): string {
    var ints: Map<int, int> = Map()
    ints[1] = 1
    var bools: Map<bool, int> = Map()
    bools[true] = 2
    var strings: Map<string, int> = Map()
    strings["key"] = 3
    var byteStrings: Map<bytes, int> = Map()
    byteStrings[bytes("key")] = 4
    var dates: Map<date, int> = Map()
    dates[date("2026-07-18")] = 5
    var instants: Map<instant, int> = Map()
    instants[instant("2026-07-18T12:00:00Z")] = 6
    var durations: Map<duration, int> = Map()
    durations[duration("PT1S")] = 7
    const rankLow = Rank(1)
    const rankHigh = Rank(8)
    var ranks: Map<Rank, int> = Map()
    ranks[rankHigh] = 8
    ranks[rankLow] = 1
    var rankOrder: int = 0
    for key, value in ranks {
        rankOrder = rankOrder * 10 + value
    }
    if rankOrder != 18 {
        return "rank-order"
    }
    var result: string = ""
    result = result + string(ints[1] ?? 0)
    result = result + string(bools[true] ?? 0)
    result = result + string(strings["key"] ?? 0)
    result = result + string(byteStrings[bytes("key")] ?? 0)
    result = result + string(dates[date("2026-07-18")] ?? 0)
    result = result + string(instants[instant("2026-07-18T12:00:00Z")] ?? 0)
    result = result + string(durations[duration("PT1S")] ?? 0)
    result = result + string(ranks[rankHigh] ?? 0)
    return result
}
"#,
    );
    let output = run_in(&accepted, &["run", "values"]);
    assert!(
        output.status.success(),
        "all admitted Map keys must compile, verify, and run: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(output.stdout, b"12345678\n");

    let rejected = [
        (
            "decimal",
            r#"module main

pub fn value(): int {
    const values: Map<decimal, int> = Map()
    return 0
}
"#,
            "check.unsupported",
        ),
        (
            "ErrorCode",
            r#"module main

pub fn value(): int {
    const values: Map<ErrorCode, int> = Map()
    return 0
}
"#,
            "check.unsupported",
        ),
        (
            "generic parameter",
            r#"module main

fn f<K supports order>(key: K): int {
    const values: Map<K, int> = Map()
    return 0
}

pub fn value(): int {
    return f(1)
}
"#,
            "check.unsupported",
        ),
        (
            "plain int for nominal key",
            r#"module main

type Rank: int in 1..=8

pub fn value(): int {
    var values: Map<Rank, int> = Map()
    values[1] = 1
    return 0
}
"#,
            "check.type",
        ),
    ];
    for (name, source, expected_code) in rejected {
        let temp = TempDir::new("map-key-rejection");
        project(&temp, source);
        let output = run_in(&temp, &["run", "value", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "{name} must be rejected: {stdout}"
        );
        let codes: Vec<&str> = stdout
            .lines()
            .filter_map(|line| {
                let (_, rest) = line.split_once(r#""code":""#)?;
                rest.split_once('"').map(|(code, _)| code)
            })
            .collect();
        assert_eq!(codes, [expected_code], "{name}: {stdout}");
    }

    let reference = include_str!("../../../docs/language/types-and-values.md");
    let (_, lists_and_maps) = reference
        .split_once("## Lists And Maps")
        .expect("Lists And Maps section");
    let (lists_and_maps, key_types) = lists_and_maps
        .split_once("## Key Types")
        .expect("Key Types section");
    let (key_types, _) = key_types
        .split_once("## Entry Identity")
        .expect("Entry Identity section");
    let lists_and_maps = lists_and_maps
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let key_types = key_types.split_whitespace().collect::<Vec<_>>().join(" ");
    let local_domain = "`int`, `bool`, `string`, `bytes`, `date`, `instant`, and `duration`";
    let nominal_rule = "A nominal Map key retains its source type and uses its base scalar for representation and ordering.";
    for (name, section) in [
        ("Lists And Maps", lists_and_maps.as_str()),
        ("Key Types", key_types.as_str()),
    ] {
        assert!(section.contains(local_domain), "{name}: {section}");
        assert!(section.contains("nominal int type"), "{name}: {section}");
        assert!(section.contains(nominal_rule), "{name}: {section}");
        assert!(
            section.contains("`ErrorCode` is not a local Map key"),
            "{name}: {section}"
        );
    }
    assert!(
        key_types.contains(
            "Durable key positions use `int`, `bool`, `string`, `bytes`, `date`, or `instant`"
        ),
        "{key_types}"
    );
    assert!(
        key_types.contains(
            "Managed-index key positions use `int`, `bool`, `string`, `bytes`, `date`, or `instant`"
        ),
        "{key_types}"
    );
    assert!(
        key_types.contains("`duration` and nominal source types are not durable keys."),
        "{key_types}"
    );
    assert!(
        key_types.contains("A nominal stored field projects through its base scalar."),
        "{key_types}"
    );
    let normalized_reference = reference.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !normalized_reference.contains(concat!("Store identity col", "umns")),
        "the reference must not use the stale identity-key shape analogy"
    );
    assert!(
        !normalized_reference.contains(concat!("ordered lexicographically by col", "umn")),
        "the reference must describe tuple order by key position"
    );
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
fn a_bare_constructor_without_expected_type_is_a_check_type() {
    for body in ["const xs = List()", "const m = Map()"] {
        let temp = TempDir::new("bare-ctor");
        project(
            &temp,
            &format!("module main\n\npub fn f(): int {{\n\x20   {body}\n\x20   return 0\n}}\n"),
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

/// A non-key map key type (a struct), an `append` on a map, and a wrong-typed
/// element are typed diagnostics, not silent acceptance.
#[test]
fn misused_collection_operations_are_typed_diagnostics() {
    // A struct key type is not admitted: `check.unsupported` at the annotation.
    let cases: [(&str, &str); 3] = [
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

/// Variadic `List(...)` is the literal-contents form; a `Map(...)` literal is
/// deferred, mixed element types do not unify, and a named element argument is not a
/// list element. Each is a typed `check.type`.
#[test]
fn variadic_construction_rejections_are_typed() {
    let cases: [&str; 3] = [
        "const m = Map(1, 2)",
        "const xs = List(1, \"two\")",
        "const xs = List(a: 1)",
    ];
    for body in cases {
        let temp = TempDir::new("variadic-reject");
        project(
            &temp,
            &format!("module main\n\npub fn f(): int {{\n\x20   {body}\n\x20   return 0\n}}\n"),
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

/// `List` and `Map` are reserved type names; redeclaring one is a
/// `check.name_conflict`.
#[test]
fn redeclaring_a_reserved_collection_name_is_a_conflict() {
    for name in ["List", "Map"] {
        let temp = TempDir::new("reserved");
        project(
            &temp,
            &format!(
                "module main\n\nstruct {name} {{\n\x20   x: int\n}}\n\npub fn f(): int {{\n\x20   return 0\n}}\n"
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
fn redeclaring_a_text_floor_builtin_is_a_conflict() {
    for name in ["split", "lines", "join"] {
        let temp = TempDir::new("reserved-floor");
        project(
            &temp,
            &format!(
                "module main\n\nfn {name}(): int {{\n\x20   return 0\n}}\n\npub fn f(): int {{\n\x20   return 0\n}}\n"
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
