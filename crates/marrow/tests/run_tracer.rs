//! Language behavior through the production path, and the `marrow run` command
//! surface.
//!
//! Semantics compile and run in process through the same capture -> compile ->
//! verify -> VM pipeline the binary drives. Only a test whose subject is the
//! command surface itself — argument decoding, exit codes, rendered stdout/stderr
//! shape, the identity mint, the durable trough outcome — spawns the binary.

pub mod common;

use common::{CallOutcome, Diagnostics, Project, conformance_dir, marrow_in};
use marrow_codes::Code;
use marrow_vm::Value;

/// A multi-module project: each `(path, source)` is placed at `src/<path>`.
fn modules(files: &[(&str, &str)]) -> Project {
    files
        .iter()
        .fold(Project::new(), |project, (path, source)| {
            project.source(&format!("src/{path}"), source)
        })
}

/// The value one export of a single-source project returns.
fn value(source: &str, export: &str, args: Vec<Value>) -> Option<Value> {
    Project::single(source).session().call(export, args)
}

/// The stable code of the runtime fault one export raises.
fn fault(source: &str, export: &str, args: Vec<Value>) -> Code {
    match Project::single(source).session().try_call(export, args) {
        CallOutcome::Fault { code, .. } => code,
        other => panic!("expected `{export}` to fault, got {other:?}"),
    }
}

/// The typed diagnostics of a project the compiler must refuse.
fn refused(project: Project) -> Diagnostics {
    project
        .try_image()
        .expect_err("expected source diagnostics, got a compiled image")
}

fn text(value: &str) -> Value {
    Value::Text(value.into())
}

// --- Command surface: argument decoding, exit codes, rendered output ---------

/// The rendered text surface of a successful run: the canonical value and a
/// trailing newline, nothing else.
#[test]
fn run_renders_a_value_on_stdout() {
    let outcome = Project::single("pub fn answer(): int {\n    return 42\n}\n")
        .run_cli("return-const", &["run", "answer"]);
    assert!(outcome.success(), "run failed: {}", outcome.stderr_text());
    assert_eq!(outcome.stdout_text(), "42\n");
}

/// The JSONL surface is one canonical record per run.
#[test]
fn return_const_jsonl_is_canonical() {
    let outcome = Project::single("pub fn answer(): int {\n    return 42\n}\n").run_cli(
        "return-const-jsonl",
        &["run", "answer", "--format", "jsonl"],
    );
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(
        outcome.stdout_text(),
        "{\"data\":42,\"kind\":\"run\",\"outcome\":\"value\"}\n"
    );
}

/// A source diagnostic reaches the command as a `diagnostic` outcome carrying the
/// typed code, not as a value or a panic.
#[test]
fn a_type_mismatch_is_a_source_diagnostic() {
    let outcome = Project::single("pub fn answer(): int {\n    return true\n}\n")
        .run_cli("type-mismatch", &["run", "answer", "--format", "jsonl"]);
    assert!(!outcome.success());
    let stdout = outcome.stdout_text();
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{stdout}");
    assert!(stdout.contains("check.type"), "{stdout}");
}

/// Naming an export the project does not declare is a usage error (exit 2), not a
/// run failure.
#[test]
fn a_missing_export_is_a_usage_error() {
    let outcome = Project::single("pub fn answer(): int {\n    return 42\n}\n")
        .run_cli("missing-export", &["run", "nope"]);
    assert_eq!(outcome.code(), Some(2), "{outcome:?}");
}

/// A terminal value literal must be in canonical form: the `bytes` decoder admits
/// only a `0x`-prefixed even-length lowercase-hex string, and the `bool` decoder
/// only `true`/`false`. A noncanonical spelling — uppercase hex, a missing `0x`
/// prefix, an odd hex length, or `1` for a bool — is a usage error (exit 2), never
/// a silent coercion.
#[test]
fn a_noncanonical_terminal_value_literal_is_a_usage_error() {
    let workspace = Project::single(
        r#"pub fn firstByte(b: bytes): int {
    return 0
}

pub fn flag(b: bool): bool {
    return b
}
"#,
    )
    .materialize("noncanonical");
    for (export, arg) in [
        ("firstByte", "0xAB"),  // uppercase hex
        ("firstByte", "abcd"),  // missing 0x prefix
        ("firstByte", "0xabc"), // odd length
        ("flag", "1"),          // bool spelled as an int
        ("flag", "True"),       // bool wrong case
    ] {
        let outcome = workspace.marrow(&["run", export, "--", arg]);
        assert_eq!(
            outcome.code(),
            Some(2),
            "{export} {arg:?} must be a usage error: {outcome:?}"
        );
    }
    // The canonical forms are accepted, so the rejection is of the spelling, not
    // the type.
    assert!(
        workspace
            .marrow(&["run", "firstByte", "--", "0xabcd"])
            .success()
    );
    assert!(workspace.marrow(&["run", "flag", "--", "false"]).success());
}

/// `marrow run` prints an export's result in the canonical text a record, enum, or
/// scalar renders as; `string(...)` admits a narrower domain, so a record is a
/// `check.unsupported` there. The reference states both halves.
#[test]
fn export_output_and_scalar_conversion_rendering_are_distinct() {
    let outcome = modules(&[(
        "main.mw",
        r#"module main

resource Book {
    required title: string
    required author: string
}

pub fn draft(title: string, author: string): Book {
    return Book(title: title, author: author)
}
"#,
    )])
    .run_cli(
        "resource-export-rendering",
        &["run", "main.draft", "--", "Small Gods", "Pratchett"],
    );
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(
        outcome.stdout_text(),
        "{title: Small Gods, author: Pratchett}\n"
    );

    let unsupported = refused(modules(&[(
        "main.mw",
        r#"module main

resource Book {
    required title: string
}

pub fn text(): string {
    return string(Book(title: "Marrow"))
}
"#,
    )]));
    assert!(
        unsupported.has_code("check.unsupported"),
        "{:?}",
        unsupported.all()
    );

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let builtins = normalize(include_str!("../../../docs/language/builtins.md"));
    assert!(
        builtins.contains("`marrow run` prints an export's result in this same canonical text.")
    );
    assert!(builtins.contains(
        "`string(...)` and interpolation use the same scalar, enum, and identity renderings but reject bare aggregates and presence optionals."
    ));
    assert!(!builtins.contains("Resources and local or durable trees have no direct rendering"));
}

/// Reaching an `unreachable` faults with `run.unreachable`; the text output carries
/// the static author text, while the typed JSONL surface stays code and span.
#[test]
fn unreachable_faults_and_carries_static_text() {
    let workspace = Project::single(
        r#"pub fn boom(hit: bool): int {
    if hit {
        unreachable("the invariant broke")
    }
    return 0
}
"#,
    )
    .materialize("unreach-fault");

    let jsonl = workspace.marrow(&["run", "boom", "--format", "jsonl", "--", "true"]);
    assert!(!jsonl.success());
    let jsonl_out = jsonl.stdout_text();
    assert!(jsonl_out.contains(r#""outcome":"fault""#), "{jsonl_out}");
    assert!(jsonl_out.contains("run.unreachable"), "{jsonl_out}");
    assert!(
        !jsonl_out.contains("the invariant broke"),
        "static text stays out of the typed JSONL grammar: {jsonl_out}"
    );

    let text = workspace.marrow(&["run", "boom", "--", "true"]);
    assert!(!text.success());
    let text_out = text.stdout_text();
    assert!(text_out.contains("run.unreachable"), "{text_out}");
    assert!(text_out.contains("the invariant broke"), "{text_out}");
}

/// Reaching a `todo` faults with the distinct `run.todo` code; like `unreachable`,
/// the author text rides the text surface only.
#[test]
fn todo_faults_carry_static_text_on_the_text_surface_only() {
    let workspace = Project::single(
        r#"pub fn classify(n: int): int {
    if n > 0 { return 1 }
    todo("handle non-positive inputs")
}
"#,
    )
    .materialize("todo");

    let jsonl = workspace.marrow(&["run", "classify", "--format", "jsonl", "--", "-1"]);
    assert!(!jsonl.success());
    let jsonl_out = jsonl.stdout_text();
    assert!(jsonl_out.contains(r#""outcome":"fault""#), "{jsonl_out}");
    assert!(jsonl_out.contains("run.todo"), "{jsonl_out}");
    assert!(
        !jsonl_out.contains("handle non-positive"),
        "static text stays out of the typed JSONL grammar: {jsonl_out}"
    );

    let text = workspace.marrow(&["run", "classify", "--", "-1"]);
    let text_out = text.stdout_text();
    assert!(text_out.contains("run.todo"), "{text_out}");
    assert!(
        text_out.contains("handle non-positive inputs"),
        "{text_out}"
    );
}

/// `Option`/`Result` values render through `marrow run` in the canonical enum text,
/// aggregate payloads included, while JSONL preserves the structured value.
#[test]
fn generic_enum_payloads_render_through_the_command() {
    let workspace = Project::single(
        r#"struct Point {
    x: int
    y: int
}

pub fn retOpt(): Option<Point> {
    return some(Point(x: 1, y: 2))
}

pub fn retNone(): Option<Point> {
    return none
}

pub fn retResult(): Result<Point, string> {
    return ok(Point(x: 3, y: 4))
}

pub fn retNested(): Option<Option<int>> {
    return some(some(7))
}
"#,
    )
    .materialize("generic-enum-render");

    for (export, expected) in [
        ("retOpt", "Option::some({x: 1, y: 2})\n"),
        ("retNone", "Option::none\n"),
        // A top-level `ok` is unwrapped; the record inside renders on its own.
        ("retResult", "{x: 3, y: 4}\n"),
        ("retNested", "Option::some(Option::some(7))\n"),
    ] {
        let outcome = workspace.marrow(&["run", export]);
        assert!(outcome.success(), "{export}: {outcome:?}");
        assert_eq!(outcome.stdout_text(), expected, "{export}");
    }

    let jsonl = workspace.marrow(&["run", "retOpt", "--format", "jsonl"]);
    assert!(
        jsonl
            .stdout_text()
            .contains(r#""data":{"enum":"Option","member":"some","payload":[{"x":1,"y":2}]}"#),
        "{jsonl:?}"
    );
}

// --- Expressions, control flow, and the arithmetic faults -------------------

#[test]
fn locals_arithmetic_and_control_flow_compute_a_value() {
    // b = 12, 12 > 10, so returns 13.
    assert_eq!(
        value(
            r#"pub fn compute(): int {
    const a = 3
    var b = 4
    b = b * a
    if b > 10 { return b + 1 }
    return b
}
"#,
            "compute",
            vec![],
        ),
        Some(Value::Int(13))
    );
}

#[test]
fn a_while_loop_sums() {
    // 0 + 1 + 2 + 3 + 4 = 10.
    assert_eq!(
        value(
            r#"pub fn total(): int {
    var sum = 0
    var i = 0
    while i < 5 {
        sum = sum + i
        i = i + 1
    }
    return sum
}
"#,
            "total",
            vec![],
        ),
        Some(Value::Int(10))
    );
}

#[test]
fn short_circuit_boolean_logic() {
    assert_eq!(
        value(
            r#"pub fn andor(): bool {
    const t = true
    const f = false
    return t and (f or t)
}
"#,
            "andor",
            vec![],
        ),
        Some(Value::Bool(true))
    );
}

#[test]
fn runtime_overflow_is_a_source_mapped_fault() {
    assert_eq!(
        fault(
            r#"pub fn over(): int {
    const big = 9223372036854775807
    return big + 1
}
"#,
            "over",
            vec![],
        ),
        Code::RunOverflow
    );
}

#[test]
fn integer_division_by_zero_is_a_source_mapped_fault() {
    assert_eq!(
        fault(
            "pub fn q(a: int, b: int): int {\n    return a / b\n}\n",
            "q",
            vec![Value::Int(1), Value::Int(0)],
        ),
        Code::RunDivideByZero
    );
}

/// Integer `/` truncates toward zero.
#[test]
fn integer_division_truncates_toward_zero() {
    assert_eq!(
        value(
            "pub fn q(a: int, b: int): int {\n    return a / b\n}\n",
            "q",
            vec![Value::Int(-7), Value::Int(2)],
        ),
        Some(Value::Int(-3))
    );
}

/// `string` comparisons order lexicographically.
#[test]
fn string_comparison_orders_lexicographically() {
    const SOURCE: &str = "pub fn before(a: string, b: string): bool {\n    return a < b\n}\n";
    assert_eq!(
        value(SOURCE, "before", vec![text("apple"), text("banana")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        value(SOURCE, "before", vec![text("banana"), text("apple")]),
        Some(Value::Bool(false))
    );
}

/// A non-terminating loop exhausts the per-invocation instruction budget and
/// faults with `run.budget` — the VM's dynamic-limit backstop — rather than running
/// forever. There is no runner or environment override.
#[test]
fn nonterminating_loop_faults_on_the_instruction_budget() {
    assert_eq!(
        fault(
            r#"pub fn spin() {
    var n: int = 0
    while true {
        n = n + 1
    }
}
"#,
            "spin",
            vec![],
        ),
        Code::RunBudget
    );
}

/// The implemented `string` and `bytes` conversions render canonically.
#[test]
fn implemented_scalar_conversions_travel_the_full_path() {
    const SOURCE: &str = r#"pub fn asString(n: int): string {
    return string(n)
}

pub fn flag(b: bool): string {
    return string(b)
}

pub fn asBytes(s: string): bytes {
    return bytes(s)
}
"#;
    assert_eq!(
        value(SOURCE, "asString", vec![Value::Int(-7)]),
        Some(text("-7"))
    );
    assert_eq!(
        value(SOURCE, "flag", vec![Value::Bool(true)]),
        Some(text("true"))
    );
    // "hi" is 0x6869.
    assert_eq!(
        value(SOURCE, "asBytes", vec![text("hi")]),
        Some(Value::Bytes(b"hi"[..].into()))
    );
}

/// Direct byte-literal spelling is parser-recognized but not executable; the
/// current bytes constructor remains available.
#[test]
fn byte_literal_boundary_and_reference_are_exact() {
    let literal = refused(Project::single(
        "pub fn value(): bytes {\n    return b\"key\"\n}\n",
    ));
    assert!(literal.has_code("check.unsupported"), "{:?}", literal.all());

    assert_eq!(
        value(
            "pub fn value(): bytes {\n    return bytes(\"key\")\n}\n",
            "value",
            vec![],
        ),
        Some(Value::Bytes(b"key"[..].into()))
    );

    let reference = include_str!("../../../docs/language/source-and-syntax.md");
    let normalized_reference = reference.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !reference.contains("| Bytes |"),
        "the literal reference must not present bytes as an executable literal"
    );
    assert!(
        !reference.contains("Byte strings accept"),
        "the parser-only escape claim must not be presented as executable behavior"
    );
    assert!(
        reference.contains("`bytes(\"Marrow\")` constructs the UTF-8 bytes of a string"),
        "the current bytes constructor must remain documented"
    );
    assert!(
        normalized_reference
            .contains("**Future:** The parser recognizes direct byte-literal spelling"),
        "the direct byte-literal boundary must remain explicitly labeled"
    );
}

/// `int("1")`, `bool(1)`, `decimal`, and a decimal literal each carry an exact
/// rejection code, and the reference states the same boundary.
#[test]
fn rejected_conversion_and_decimal_literal_codes_are_exact() {
    for (name, source, code) in [
        (
            "int-from-text",
            "module main\n\npub fn value(): int {\n    return int(\"1\")\n}\n",
            "check.unsupported",
        ),
        (
            "bool-from-int",
            "module main\n\npub fn value(): bool {\n    return bool(1)\n}\n",
            "check.unsupported",
        ),
        (
            "decimal-call",
            "module main\n\npub fn value(): int {\n    const converted = decimal(1)\n    return 0\n}\n",
            "check.type",
        ),
        (
            "decimal-literal",
            "module main\n\npub fn value(): int {\n    return 1.5\n}\n",
            "check.unsupported",
        ),
    ] {
        let diagnostics = refused(modules(&[("main.mw", source)]));
        assert!(
            diagnostics.has_code(code),
            "{name}: {:?}",
            diagnostics.all()
        );
    }

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let types = normalize(include_str!("../../../docs/language/types-and-values.md"));
    let syntax = normalize(include_str!("../../../docs/language/source-and-syntax.md"));
    let builtins = normalize(include_str!("../../../docs/language/builtins.md"));

    assert!(types.contains(
        "`int(\"1\")` and `bool(1)` are examples. `decimal` has no current callable scalar owner, so `decimal(1)` reports `check.type`."
    ));
    assert!(syntax.contains("| Decimal (**future**) |"));
    assert!(!syntax.contains("| Decimal |"));
    assert!(!builtins.contains("std::bytes::toText"));
    assert!(!types.contains("| `bool` | `bool`, or `int` equal to `0` or `1` |"));
}

/// `catch` and `throw` are ordinary identifiers, and the removed statement forms
/// are parse errors. The reference carries no exception channel.
#[test]
fn reference_excludes_removed_throwable_channel_and_keywords() {
    assert_eq!(
        modules(&[(
            "main.mw",
            r#"module main

fn catch(value: int): int {
    return value + 1
}

fn throw(value: int): int {
    return value + 1
}

pub fn run(): int {
    return catch(throw(2))
}
"#,
        )])
        .session()
        .call("run", vec![]),
        Some(Value::Int(4))
    );

    for (name, statement) in [
        ("removed-throw-statement", "throw \"failure\""),
        ("removed-catch-clause", "catch { return 0 }"),
    ] {
        let source =
            format!("module main\n\npub fn run(): int {{\n    {statement}\n    return 1\n}}\n");
        let diagnostics = refused(modules(&[("main.mw", &source)]));
        assert!(
            diagnostics.has_code("parse.syntax"),
            "{name}: {:?}",
            diagnostics.all()
        );
    }

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let readme = normalize(include_str!("../../../docs/language/README.md"));
    let standard = normalize(include_str!("../../../docs/language/builtins.md"));
    let source = normalize(include_str!("../../../docs/language/source-and-syntax.md"));
    let functions = normalize(include_str!(
        "../../../docs/language/modules-and-functions.md"
    ));
    let types = normalize(include_str!("../../../docs/language/types-and-values.md"));
    let control = normalize(include_str!("../../../docs/language/control-flow.md"));

    assert!(!readme.contains("defines thrown values"));
    assert!(!readme.contains("catchable faults"));
    assert!(!standard.contains("error constructors"));
    assert!(!source.contains("resource and `Error` constructors"));
    assert!(!functions.contains("catch bindings"));
    assert!(!functions.contains("may read or write durable paths, throw"));
    assert!(!types.contains("and `Error` values have no"));
    assert!(!types.contains("## Error Values"));
    assert!(!types.contains("constructed by `Error(...)`"));
    assert!(!types.contains("optional standard-library functions"));
    assert!(!control.contains("optional standard-library result"));
    assert!(!source.contains("try catch throw delete"));

    assert!(source.contains(
        "`catch` and `throw` are not keywords; statement-head forms from the removed exception channel report `parse.syntax`."
    ));
    assert!(readme.contains("`Result` propagation"));
    assert!(functions.contains("A handled failure is an ordinary `Result<T, E>` value"));
    assert!(types.contains("`Result<T, E>` models a recoverable failure"));
}

// --- The checked-arithmetic form -------------------------------------------

/// The checked-arithmetic form: the success path binds the result; each fault
/// runs its diverging arm.
#[test]
fn checked_arithmetic_success_and_each_arm() {
    const SOURCE: &str = r#"pub fn safeMul(a: int, b: int): int {
    const p: int = checked a * b
        on out_of_range return -1
    return p
}

pub fn safeDiv(a: int, b: int): int {
    return checked a / b
        on out_of_range {
            return -1
        } on zero_divisor return 0
}
"#;
    let mut session = Project::single(SOURCE).session();
    let call = |session: &mut common::Session, export: &str, a: i64, b: i64| {
        session.call(export, vec![Value::Int(a), Value::Int(b)])
    };
    // Success paths.
    assert_eq!(call(&mut session, "safeMul", 6, 7), Some(Value::Int(42)));
    assert_eq!(call(&mut session, "safeDiv", 20, 4), Some(Value::Int(5)));
    // out_of_range arm: 2^62 * 4 overflows.
    assert_eq!(
        call(&mut session, "safeMul", 4_611_686_018_427_387_904, 4),
        Some(Value::Int(-1))
    );
    // zero_divisor arm.
    assert_eq!(call(&mut session, "safeDiv", 1, 0), Some(Value::Int(0)));
    // out_of_range arm of division: i64::MIN / -1.
    assert_eq!(
        call(&mut session, "safeDiv", i64::MIN, -1),
        Some(Value::Int(-1))
    );
}

/// Complex nested procedural code reads clearly with the checked form, without
/// combinator ceremony: a running total that both guards overflow and short-circuits.
#[test]
fn checked_reads_clearly_in_nested_procedural_code() {
    const SOURCE: &str = r#"pub fn boundedFactorial(n: int, cap: int): int {
    var acc: int = 1
    var i: int = 2
    while i <= n {
        const next: int = checked acc * i
            on out_of_range return -1
        if next > cap { return cap }
        acc = next
        i = i + 1
    }
    return acc
}
"#;
    let mut session = Project::single(SOURCE).session();
    assert_eq!(
        session.call(
            "boundedFactorial",
            vec![Value::Int(5), Value::Int(1_000_000)]
        ),
        Some(Value::Int(120))
    );
    // Overflow guard fires before native overflow: with the cap just below
    // i64::MAX, 20! (2.4e18) stays under it but 21! overflows and runs the arm.
    assert_eq!(
        session.call(
            "boundedFactorial",
            vec![Value::Int(100), Value::Int(9_000_000_000_000_000_000)]
        ),
        Some(Value::Int(-1))
    );
    // Cap short-circuit.
    assert_eq!(
        session.call("boundedFactorial", vec![Value::Int(20), Value::Int(100)]),
        Some(Value::Int(100))
    );
}

/// A checked form whose arm does not diverge, or that omits a required arm, is a
/// source diagnostic.
#[test]
fn checked_form_arm_rules_are_diagnostics() {
    // Non-diverging out_of_range arm.
    let non_diverging = refused(Project::single(
        r#"pub fn bad(a: int, b: int): int {
    const p: int = checked a + b
        on out_of_range {
            const x: int = 0
        }
    return p
}
"#,
    ));
    assert!(
        non_diverging.has_code("check.type"),
        "{:?}",
        non_diverging.all()
    );

    // Missing zero_divisor arm on a checked division.
    let missing_arm = refused(Project::single(
        r#"pub fn bad(a: int, b: int): int {
    return checked a / b
        on out_of_range return -1
}
"#,
    ));
    assert!(
        missing_arm.has_code("check.type"),
        "{:?}",
        missing_arm.all()
    );
}

/// A checked `/`/`%` whose divisor is a provably-nonzero integer literal takes no
/// `on zero_divisor` arm (the fault is dead), runs correctly, and still arms the
/// live `on out_of_range` overflow. A supplied dead arm is rejected; a non-literal
/// or literal-zero divisor still requires the arm.
#[test]
fn checked_division_by_a_nonzero_literal_drops_the_dead_zero_arm() {
    // No zero_divisor arm needed; runs.
    assert_eq!(
        value(
            r#"pub fn half(x: int): int {
    const q: int = checked x / 100
        on out_of_range return -1
    return q
}
"#,
            "half",
            vec![Value::Int(500)],
        ),
        Some(Value::Int(5))
    );

    // The out_of_range arm stays live: i64::MIN / -1 overflows into it.
    assert_eq!(
        value(
            r#"pub fn neg(x: int): int {
    const q: int = checked x / -1
        on out_of_range return 777
    return q
}
"#,
            "neg",
            vec![Value::Int(i64::MIN)],
        ),
        Some(Value::Int(777))
    );

    // A supplied `on zero_divisor` arm on a literal-nonzero divisor is a dead arm.
    let dead = refused(Project::single(
        r#"pub fn dead(x: int): int {
    const q: int = checked x / 100
        on out_of_range return -1
        on zero_divisor return 0
    return q
}
"#,
    ));
    assert!(dead.has_code("check.type"), "{:?}", dead.all());

    // A non-literal divisor and a literal-zero divisor still require the arm.
    for body in [
        "pub fn f(x: int, d: int): int {\n    return checked x / d\n        on out_of_range return -1\n}\n",
        "pub fn f(x: int): int {\n    return checked x / 0\n        on out_of_range return -1\n}\n",
    ] {
        let diagnostics = refused(Project::single(body));
        assert!(
            diagnostics.has_code("check.type"),
            "{body}: {:?}",
            diagnostics.all()
        );
    }
}

// --- The text floor, interpolation, and the renderable-hole boundary --------

/// The closed pure text floor: isEmpty / contains / trim.
#[test]
fn text_floor_builtins_travel_the_full_path() {
    const SOURCE: &str = r#"pub fn empty(s: string): bool {
    return isEmpty(trim(s))
}

pub fn has(h: string, n: string): bool {
    return contains(h, n)
}
"#;
    let mut session = Project::single(SOURCE).session();
    assert_eq!(
        session.call("empty", vec![text("   ")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("empty", vec![text(" x ")]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        session.call("has", vec![text("hello"), text("ell")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("has", vec![text("hello"), text("xyz")]),
        Some(Value::Bool(false))
    );
}

/// Interpolated strings carry their decoded literal segments and doubled-brace
/// escapes, and scalar holes use the same canonical rendering as `string(value)`.
#[test]
fn interpolation_renders_holes_and_escapes() {
    const SOURCE: &str = r#"pub fn greet(id: int, on: bool): string {
    return $"id: {id} ok={on}!"
}

pub fn braces(): string {
    return $"a {{ b }}\tc"
}

pub fn empty(): string {
    return $""
}
"#;
    let mut session = Project::single(SOURCE).session();
    assert_eq!(
        session.call("greet", vec![Value::Int(7), Value::Bool(true)]),
        Some(text("id: 7 ok=true!"))
    );
    assert_eq!(session.call("braces", vec![]), Some(text("a { b }\tc")));
    assert_eq!(session.call("empty", vec![]), Some(text("")));
}

/// A hole whose value has no current canonical rendering is a typed
/// `check.unsupported`, matching the `string(value)` boundary.
#[test]
fn interpolation_rejects_an_unrenderable_hole() {
    let diagnostics = refused(Project::single(
        "pub fn bad(d: decimal): string {\n    return $\"v: {d}\"\n}\n",
    ));
    assert!(
        diagnostics.has_code("check.unsupported"),
        "{:?}",
        diagnostics.all()
    );
}

/// Every canonically renderable value is an interpolation hole and rides
/// `string(...)`: temporals, enums (with payloads), and bytes. A record, list, map,
/// or optional hole is not renderable and is refused.
#[test]
fn interpolation_and_string_render_every_scalar_and_enum() {
    const SOURCE: &str = r#"enum Shape {
    dot
    circle(radius: int)
}

pub fn temporal(): string {
    const due = date("2026-08-01")
    const at = instant("2026-07-15T17:00:00Z")
    return $"{due} {at} in {3 days}"
}

pub fn shape(): string {
    return $"{Shape::circle(radius: 5)} and {Shape::dot}"
}

pub fn asText(): string {
    return string(date("2026-08-01"))
}

pub fn bytesHole(s: string): string {
    return $"{bytes(s)}"
}
"#;
    let mut session = Project::single(SOURCE).session();
    assert_eq!(
        session.call("temporal", vec![]),
        Some(text("2026-08-01 2026-07-15T17:00:00Z in PT259200S"))
    );
    assert_eq!(
        session.call("shape", vec![]),
        Some(text("Shape::circle(5) and Shape::dot"))
    );
    assert_eq!(session.call("asText", vec![]), Some(text("2026-08-01")));
    // "hi" is 0x6869; a bytes hole renders as canonical hex.
    assert_eq!(
        session.call("bytesHole", vec![text("hi")]),
        Some(text("0x6869"))
    );

    // A list hole is not a renderable value.
    let list = refused(modules(&[(
        "main.mw",
        "module main\n\npub fn f(): string {\n    var xs: List<int> = List(1, 2)\n    return $\"{xs}\"\n}\n",
    )]));
    assert!(list.has_code("check.unsupported"), "{:?}", list.all());
}

/// Interpolation renders an aggregate enum payload through the canonical owner.
#[test]
fn interpolation_renders_an_aggregate_enum_payload() {
    assert_eq!(
        value(
            r#"struct Point {
    x: int
    y: int
}

pub fn interp(): string {
    const p: Option<Point> = some(Point(x: 1, y: 2))
    return $"p is {p}"
}
"#,
            "interp",
            vec![],
        ),
        Some(text("p is Option::some({x: 1, y: 2})"))
    );
}

/// A bare presence-optional (`T?`) hole stays refused at check — only a scalar, enum,
/// or identity is a renderable hole; the `Option<T>` enum renders, the `T?` does not.
#[test]
fn a_bare_presence_optional_hole_is_still_refused() {
    let diagnostics = refused(modules(&[(
        "main.mw",
        "module main\n\nfn maybe(n: int): int? {\n    if n > 0 { return n }\n    return absent\n}\n\npub fn f(n: int): string {\n    return $\"{maybe(n)}\"\n}\n",
    )]));
    assert!(
        diagnostics.has_code("check.unsupported"),
        "{:?}",
        diagnostics.all()
    );
}

// --- Presence forms: `if const`, let-else, divergence -----------------------

/// A chained `if const` proves each subject present left to right (short-circuit),
/// scopes each binding rightward and into the then block, and takes the else tail
/// when any subject is absent or the trailing condition is false.
#[test]
fn if_const_chain_short_circuits_and_scopes_rightward() {
    const SOURCE: &str = r#"fn maybe(n: int): int? {
    if n > 0 { return n }
    return absent
}

pub fn f(a: int): int {
    if const x = maybe(a) and const y = maybe(x + 1) and x > 2 {
        return x * 100 + y
    } else {
        return -1
    }
}
"#;
    let mut session = Project::single(SOURCE).session();
    // present: x=5, y=maybe(6)=6, 5>2 true.
    assert_eq!(
        session.call("f", vec![Value::Int(5)]),
        Some(Value::Int(506))
    );
    // trailing condition false: x=2, 2>2 false.
    assert_eq!(session.call("f", vec![Value::Int(2)]), Some(Value::Int(-1)));
    // first subject absent short-circuits.
    assert_eq!(session.call("f", vec![Value::Int(0)]), Some(Value::Int(-1)));
}

/// A let-else binding runs its diverging `else` when the subject is absent and
/// otherwise binds the present value for the rest of the block; a `var` let-else
/// binds mutably; a non-diverging `else` is a typed `check.type`.
#[test]
fn let_else_binds_present_and_requires_a_diverging_else() {
    const SOURCE: &str = r#"fn maybe(n: int): int? {
    if n > 0 { return n }
    return absent
}

pub fn f(a: int): int {
    var x = maybe(a) else {
        return -1
    }
    x += 100
    return x
}
"#;
    let mut session = Project::single(SOURCE).session();
    assert_eq!(
        session.call("f", vec![Value::Int(5)]),
        Some(Value::Int(105))
    );
    assert_eq!(session.call("f", vec![Value::Int(0)]), Some(Value::Int(-1)));

    let diagnostics = refused(Project::single(
        r#"fn maybe(n: int): int? {
    if n > 0 { return n }
    return absent
}

pub fn f(a: int): int {
    const x = maybe(a) else {
        const y = 1
    }
    return x
}
"#,
    ));
    assert!(
        diagnostics.has_code("check.type"),
        "{:?}",
        diagnostics.all()
    );
}

/// A let-else binding is out of scope inside its own `else` — the absent edge,
/// where the binding is never established. A reference to it there is a scoped
/// unknown-name `check.type`, never an uninitialized-slot image rejection, and it
/// does not shadow an outer binding of the same name that the `else` should see.
#[test]
fn let_else_binding_is_out_of_scope_in_its_own_else() {
    // A checker rejection, not an image.function artifact rejection.
    let scoped = refused(Project::single(
        r#"fn maybe(n: int): int? {
    if n > 0 { return n }
    return absent
}

pub fn f(a: int): int {
    const x = maybe(a) else {
        return x
    }
    return x
}
"#,
    ));
    let codes = scoped.codes();
    assert!(codes.contains(&"check.type"), "{codes:?}");
    assert!(
        !codes.iter().any(|code| code.starts_with("image.")),
        "{codes:?}"
    );

    // The else sees the outer binding, not the not-yet-established inner one.
    let mut session = Project::single(
        r#"fn maybe(n: int): int? {
    if n > 0 { return n }
    return absent
}

pub fn f(a: int): int {
    const n = 7
    const n = maybe(a) else {
        return n
    }
    return n
}
"#,
    )
    .session();
    // absent: the else returns the outer n (7).
    assert_eq!(session.call("f", vec![Value::Int(0)]), Some(Value::Int(7)));
    // present: the inner binding is in scope for the continuation (5).
    assert_eq!(session.call("f", vec![Value::Int(5)]), Some(Value::Int(5)));
}

/// `unreachable(...)` diverges, so it stands as the final statement of a
/// value-returning function whose earlier branches cover every real case, and it
/// runs the returning path normally.
#[test]
fn unreachable_satisfies_exhaustive_return_and_runs_the_real_path() {
    assert_eq!(
        value(
            r#"pub fn sign(n: int): int {
    if n > 0 { return 1 }
    if n < 0 { return -1 }
    if n == 0 { return 0 }
    unreachable("n is int, so one branch always returns")
}
"#,
            "sign",
            vec![Value::Int(-5)],
        ),
        Some(Value::Int(-1))
    );
}

/// `unreachable` requires a static string literal, so a computed argument is a
/// source diagnostic, not a runtime value.
#[test]
fn unreachable_rejects_a_computed_argument() {
    let diagnostics = refused(Project::single(
        "pub fn bad(s: string): int {\n    unreachable(s)\n}\n",
    ));
    assert!(
        diagnostics.has_code("check.type"),
        "{:?}",
        diagnostics.all()
    );
}

/// `todo("...")` mirrors `unreachable`: it diverges (so it satisfies exhaustive
/// return), it requires a static string literal, and reaching it faults with the
/// distinct `run.todo` code.
#[test]
fn todo_diverges_and_faults_run_todo() {
    const SOURCE: &str = r#"pub fn classify(n: int): int {
    if n > 0 { return 1 }
    todo("handle non-positive inputs")
}
"#;
    // Divergence satisfies the "all paths return" check and the real path runs.
    assert_eq!(
        value(SOURCE, "classify", vec![Value::Int(7)]),
        Some(Value::Int(1))
    );
    assert_eq!(
        fault(SOURCE, "classify", vec![Value::Int(-1)]),
        Code::RunTodo
    );

    // A computed argument is rejected, like `unreachable`.
    let diagnostics = refused(Project::single(
        "pub fn bad(s: string): int {\n    todo(s)\n}\n",
    ));
    assert!(
        diagnostics.has_code("check.type"),
        "{:?}",
        diagnostics.all()
    );
}

// --- Records: constructors, field reads, optional coalescing ----------------

/// A project whose resource, constructor, field reads, optional coalescing, and
/// `if const` guard travel the full path. One source file drives several exports.
const RECORDS_SOURCE: &str = r#"resource Note {
    required title: string
    body: string
}

pub fn titleOf(): string {
    const n = Note(title: "hello")
    return n.title
}

pub fn bodyOrDefault(): string {
    const n = Note(title: "hi", body: "there")
    return n.body ?? "none"
}

pub fn missingBody(): string {
    const n = Note(title: "hi")
    return n.body ?? "none"
}

pub fn guardedBody(): string {
    const n = Note(title: "hi", body: "yo")
    if const b = n.body {
        return b
    }
    return "none"
}

pub fn maybe(): string? {
    return absent
}
"#;

#[test]
fn record_field_reads_and_optional_coalescing_compute() {
    let mut session = Project::single(RECORDS_SOURCE).session();
    assert_eq!(session.call("titleOf", vec![]), Some(text("hello")));
    assert_eq!(session.call("bodyOrDefault", vec![]), Some(text("there")));
    assert_eq!(session.call("missingBody", vec![]), Some(text("none")));
    assert_eq!(session.call("guardedBody", vec![]), Some(text("yo")));
    assert_eq!(session.call("maybe", vec![]), Some(Value::Optional(None)));
}

// --- Module constants -------------------------------------------------------

#[test]
fn a_module_constant_folds_into_a_function() {
    assert_eq!(
        modules(&[(
            "main.mw",
            "module main\n\nconst MAX: int = 100\n\npub fn cap(): int {\n    return MAX + 1\n}\n",
        )])
        .session()
        .call("cap", vec![]),
        Some(Value::Int(101))
    );
}

#[test]
fn a_negated_integer_constant_is_allowed() {
    assert_eq!(
        modules(&[(
            "main.mw",
            "module main\n\nconst MIN = -5\n\npub fn floor(): int {\n    return MIN\n}\n",
        )])
        .session()
        .call("floor", vec![]),
        Some(Value::Int(-5))
    );
}

#[test]
fn a_constant_type_annotation_must_match_its_value() {
    assert!(
        refused(modules(&[(
            "main.mw",
            "module main\n\nconst FLAG: bool = 1\n\npub fn run(): bool {\n    return FLAG\n}\n",
        )]))
        .has_code("check.type")
    );
}

#[test]
fn a_non_literal_constant_is_unsupported() {
    assert!(
        refused(modules(&[(
            "main.mw",
            "module main\n\nconst SUM = 1 + 2\n\npub fn run(): int {\n    return SUM\n}\n",
        )]))
        .has_code("check.unsupported")
    );
}

#[test]
fn a_module_constant_is_private_to_its_module() {
    // `SECRET` is declared in `lib`; referencing it unqualified from `main` is not
    // in scope, and a qualified constant reference is not a supported form.
    assert!(
        refused(modules(&[
            ("lib.mw", "module lib\n\nconst SECRET = 7\n"),
            (
                "main.mw",
                "module main\n\npub fn run(): int {\n    return SECRET\n}\n",
            ),
        ]))
        .has_code("check.type")
    );
}

#[test]
fn a_duplicate_constant_in_one_module_conflicts() {
    assert!(
        refused(modules(&[(
            "main.mw",
            "module main\n\nconst K = 1\n\nconst K = 2\n\npub fn run(): int {\n    return K\n}\n",
        )]))
        .has_code("check.name_conflict")
    );
}

// --- Module-scoped call resolution and `use` imports ------------------------

/// A `std::` path is ordinary project module resolution, not an ambient library:
/// an absent module is a `check.type` at the call, a project-declared one resolves
/// and runs, and a private target is a `check.visibility`.
#[test]
fn std_paths_use_ordinary_project_module_resolution() {
    const CALLER: &str = r#"module main

pub fn run(): string {
    return std::text::decorate("Marrow")
}
"#;
    assert!(
        refused(modules(&[("main.mw", CALLER)])).has_code("check.type"),
        "an absent std module is unresolved at the call"
    );

    assert_eq!(
        modules(&[
            ("main.mw", CALLER),
            (
                "std/text.mw",
                "module std::text\n\npub fn decorate(value: string): string {\n    return $\"[{value}]\"\n}\n",
            ),
        ])
        .session()
        .call("run", vec![]),
        Some(text("[Marrow]"))
    );

    assert!(
        refused(modules(&[
            ("main.mw", CALLER),
            (
                "std/text.mw",
                "module std::text\n\nfn decorate(value: string): string {\n    return $\"[{value}]\"\n}\n",
            ),
        ]))
        .has_code("check.visibility")
    );

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let standard = normalize(include_str!("../../../docs/language/builtins.md"));
    let source = normalize(include_str!("../../../docs/language/source-and-syntax.md"));
    let functions = normalize(include_str!(
        "../../../docs/language/modules-and-functions.md"
    ));
    let future = normalize(include_str!(
        "../../../docs/future/general-purpose-language.md"
    ));

    assert!(standard.contains("The current toolchain supplies no `std::` modules."));
    assert!(
        standard
            .contains("A project-declared `std::` path is project code, not an ambient library.")
    );
    assert!(!standard.contains("std::text::trim"));
    assert!(!source.contains("declared library names"));
    assert!(!source.contains("std::text::contains"));
    assert!(!functions.contains("`std::` operations"));
    assert!(!functions.contains("host-provided standard-library function"));
    assert!(!future.contains("current standard library is implemented"));
}

/// Project and generic functions take positional arguments; a labelled argument is
/// a `check.type`.
#[test]
fn project_and_generic_function_arguments_are_positional() {
    assert_eq!(
        modules(&[(
            "main.mw",
            r#"module main

fn decorate(value: string): string {
    return $"[{value}]"
}

pub fn run(): string {
    return decorate("Marrow")
}
"#,
        )])
        .session()
        .call("run", vec![]),
        Some(text("[Marrow]"))
    );

    for (name, source) in [
        (
            "named-project-call",
            r#"module main

fn decorate(value: string): string {
    return $"[{value}]"
}

pub fn run(): string {
    return decorate(value: "Marrow")
}
"#,
        ),
        (
            "named-generic-call",
            r#"module main

fn identity<T>(value: T): T {
    return value
}

pub fn run(): int {
    return identity(value: 7)
}
"#,
        ),
    ] {
        let diagnostics = refused(modules(&[("main.mw", source)]));
        assert!(
            diagnostics.has_code("check.type"),
            "{name}: {:?}",
            diagnostics.all()
        );
    }

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let source = normalize(include_str!("../../../docs/language/source-and-syntax.md"));
    let functions = normalize(include_str!(
        "../../../docs/language/modules-and-functions.md"
    ));

    assert!(source.contains("Project and generic functions take positional arguments."));
    assert!(functions.contains("Project and generic functions take positional arguments."));
    assert!(!source.contains("project functions match argument labels"));
    assert!(!source.contains("does not yet reject labels consistently"));
    assert!(!functions.contains("Project-function arguments may be positional"));
    assert!(!functions.contains("matched to project-function parameter names"));
}

#[test]
fn a_use_import_resolves_a_cross_module_call() {
    assert_eq!(
        modules(&[
            (
                "mathlib/ops.mw",
                "module mathlib::ops\n\npub fn double(n: int): int {\n    return n + n\n}\n",
            ),
            (
                "main.mw",
                "module main\n\nuse mathlib::ops\n\npub fn run(): int {\n    return ops::double(21)\n}\n",
            ),
        ])
        .session()
        .call("run", vec![]),
        Some(Value::Int(42))
    );
}

#[test]
fn a_fully_qualified_call_resolves_without_a_use() {
    assert_eq!(
        modules(&[
            (
                "mathlib/ops.mw",
                "module mathlib::ops\n\npub fn triple(n: int): int {\n    return n + n + n\n}\n",
            ),
            (
                "main.mw",
                "module main\n\npub fn run(): int {\n    return mathlib::ops::triple(4)\n}\n",
            ),
        ])
        .session()
        .call("run", vec![]),
        Some(Value::Int(12))
    );
}

#[test]
fn a_same_name_function_in_another_module_does_not_conflict() {
    // Two modules each define `helper`; an unqualified call binds the caller's own.
    assert_eq!(
        modules(&[
            ("a.mw", "module a\n\npub fn helper(): int {\n    return 1\n}\n"),
            (
                "b.mw",
                "module b\n\nfn helper(): int {\n    return 2\n}\n\npub fn run(): int {\n    return helper()\n}\n",
            ),
        ])
        .session()
        .call("run", vec![]),
        Some(Value::Int(2))
    );
}

#[test]
fn a_bare_call_does_not_reach_a_function_in_another_module() {
    // `greet` exists only in `other`; an unqualified call from `main` resolves in
    // `main` alone and is unresolved, not silently bound across the boundary.
    assert!(
        refused(modules(&[
            (
                "other.mw",
                "module other\n\npub fn greet(): int {\n    return 1\n}\n",
            ),
            (
                "main.mw",
                "module main\n\npub fn run(): int {\n    return greet()\n}\n",
            ),
        ]))
        .has_code("check.type")
    );
}

#[test]
fn a_qualified_call_to_an_own_module_private_function_resolves() {
    // Qualifying a call with the caller's own module reaches a private function
    // there; visibility only gates crossing a module boundary.
    assert_eq!(
        modules(&[(
            "main.mw",
            "module main\n\nfn secret(): int {\n    return 7\n}\n\npub fn run(): int {\n    return main::secret()\n}\n",
        )])
        .session()
        .call("run", vec![]),
        Some(Value::Int(7))
    );
}

#[test]
fn calling_a_private_function_across_modules_is_a_visibility_error() {
    assert!(
        refused(modules(&[
            (
                "lib.mw",
                "module lib\n\nfn secret(): int {\n    return 1\n}\n"
            ),
            (
                "main.mw",
                "module main\n\npub fn run(): int {\n    return lib::secret()\n}\n",
            ),
        ]))
        .has_code("check.visibility")
    );
}

#[test]
fn a_use_of_an_unknown_module_is_an_import_error() {
    assert!(
        refused(modules(&[(
            "main.mw",
            "module main\n\nuse nope::missing\n\npub fn run(): int {\n    return 1\n}\n",
        )]))
        .has_code("check.import")
    );
}

#[test]
fn a_headerless_script_export_runs_by_its_path_derived_name() {
    let outcome = modules(&[("tools/math.mw", "pub fn two(): int {\n    return 2\n}\n")])
        .run_cli("headerless-script-export", &["run", "tools.math.two"]);
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(outcome.stdout_text(), "2\n");
}

#[test]
fn a_headerless_script_is_not_importable_by_module_path() {
    assert!(
        refused(modules(&[
            ("lib.mw", "pub fn helper(): int {\n    return 1\n}\n"),
            (
                "main.mw",
                "module main\n\nuse lib\n\npub fn run(): int {\n    return lib::helper()\n}\n",
            ),
        ]))
        .has_code("check.import")
    );
}

#[test]
fn a_module_header_that_disagrees_with_its_path_is_rejected() {
    assert!(
        refused(modules(&[(
            "main.mw",
            "module wrong\n\npub fn run(): int {\n    return 1\n}\n",
        )]))
        .has_code("check.module_path")
    );
}

#[test]
fn a_duplicate_function_name_in_one_module_conflicts() {
    assert!(
        refused(modules(&[(
            "main.mw",
            "module main\n\nfn helper(): int {\n    return 1\n}\n\nfn helper(): int {\n    return 2\n}\n\npub fn run(): int {\n    return helper()\n}\n",
        )]))
        .has_code("check.name_conflict")
    );
}

#[test]
fn direct_calls_resolve_forward_and_compute() {
    // `quad` is declared before `double`, exercising forward resolution.
    assert_eq!(
        value(
            "pub fn quad(): int {\n    return double(double(5))\n}\n\nfn double(n: int): int {\n    return n + n\n}\n",
            "quad",
            vec![],
        ),
        Some(Value::Int(20))
    );
}

#[test]
fn mutual_recursion_is_a_check_time_diagnostic() {
    // Recursion is caught at check time, before an image is produced.
    assert!(
        refused(Project::single(
            "pub fn ping(): int {\n    return pong()\n}\n\nfn pong(): int {\n    return ping()\n}\n",
        ))
        .has_code("check.recursion")
    );
}

#[test]
fn direct_self_recursion_is_a_check_time_diagnostic() {
    assert!(
        refused(Project::single(
            "pub fn loops(): int {\n    return loops()\n}\n",
        ))
        .has_code("check.recursion")
    );
}

// --- The durable trough: the command compiles, verifies, mints, and parks ----
//
// Without `--store` the CLI compiles, verifies, and completes the identity of a
// durable program but opens no store, so a storeless `run` of a durable export
// reports the typed `cli.durable_unsupported` outcome. Reaching that outcome is
// positive evidence the durable image is well-formed and identity-complete.

const COUNTER_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[name: string]: Counter

pub fn set(name: string, v: int) {
    transaction {
        ^counters[name] = Counter(value: v)
    }
}

pub fn get(name: string): int? {
    return ^counters[name].value
}
"#;

/// A checked entry reference captures one durable entry address.
/// A durable export using references travels the whole pipeline and
/// parks in the trough exactly like the inline address forms.
const REFERENCE_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[name: string]: Counter

pub fn bump(name: string, v: int) {
    transaction {
        ^counters[name] = Counter(value: v)
        ref p = ^counters[name] else { unreachable("created entry missing") }
        p.label = "tag"
    }
}

pub fn get(name: string): int? {
    return ^counters[name].value
}
"#;

/// A read-only durable export: `run` mints the fresh identities, then parks. A
/// mutating durable export parks the same way.
#[test]
fn a_durable_export_parks_in_the_trough() {
    for (label, source) in [
        ("counter-trough", COUNTER_SOURCE),
        ("reference-trough", REFERENCE_SOURCE),
    ] {
        let workspace = Project::single(source).materialize(label);

        let get = workspace.marrow(&["run", "get", "--format", "jsonl", "--", "hits"]);
        assert!(!get.success(), "{label}: a durable run parks: {get:?}");
        let out = get.stdout_text();
        assert!(out.contains(r#""outcome":"error""#), "{label}: {out}");
        assert!(out.contains("cli.durable_unsupported"), "{label}: {out}");
        assert!(
            workspace.path(".marrow/ids").exists(),
            "{label}: the mint pre-pass published .marrow/ids before parking"
        );

        let mutating = if source == COUNTER_SOURCE {
            "set"
        } else {
            "bump"
        };
        let written = workspace.marrow(&["run", mutating, "--", "hits", "5"]);
        assert!(!written.success(), "{label}: {written:?}");
        assert!(
            written.stdout_text().contains("cli.durable_unsupported"),
            "{label}: {written:?}"
        );
    }
}

/// `--store` is a recognized flag that fails precisely, not a usage error (exit 2).
/// It also closes the run-mint window — with a persistent store a missing durable
/// identity is a precise `check.durable_identity` failure, never the additive
/// auto-mint the storeless path performs.
#[test]
fn the_store_flag_is_recognized_and_closes_the_run_mint_window() {
    let workspace = Project::single(COUNTER_SOURCE).materialize("counter-store-flag");
    let outcome = workspace.marrow(&["run", "get", "--store", "s", "--", "hits"]);
    assert_eq!(
        outcome.code(),
        Some(1),
        "--store is a recognized flag that fails precisely, not a usage error (2): {outcome:?}"
    );
    assert!(
        outcome.stdout_text().contains("check.durable_identity"),
        "with --store a missing identity is a precise failure, not an auto-mint: {outcome:?}"
    );
    // The refusal wrote no ledger: the run-mint window is closed for a persistent store.
    assert!(
        !workspace.path(".marrow/ids").exists(),
        "no .marrow/ids may be minted on the persistent path: {outcome:?}"
    );
}

/// The checked-in tracer and reference fixtures each ship a complete `.marrow/ids`, so
/// a durable export travels the full pipeline and parks in the trough.
#[test]
fn the_checked_in_durable_fixtures_compile_verify_and_park() {
    for name in ["tracer_counter", "reference_counter"] {
        let outcome = marrow_in(
            &conformance_dir(name),
            &["run", "get", "--format", "jsonl", "--", "hits"],
        );
        assert!(!outcome.success(), "{name}: {outcome:?}");
        let out = outcome.stdout_text();
        assert!(out.contains(r#""outcome":"error""#), "{name}: {out}");
        assert!(out.contains("cli.durable_unsupported"), "{name}: {out}");
    }
}

/// `duration` is a span, not an identity, so it is not in the durable-key set: a
/// duration-keyed store is a source diagnostic, not a runnable graph.
#[test]
fn a_duration_keyed_store_is_a_source_diagnostic() {
    assert!(
        refused(Project::single(
            r#"resource Span {
    required n: int
}

store ^spans[d: duration]: Span

pub fn get(d: duration): int? {
    return ^spans[d].n
}
"#,
        ))
        .has_code("check.type")
    );
}
