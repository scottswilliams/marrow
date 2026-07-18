//! End-to-end `marrow run` tests: source travels the real production path
//! (capture → compile → encode → verify → VM) through the built binary.

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
            std::env::temp_dir().join(format!("marrow-t01-{name}-{}-{nanos}", std::process::id()));
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

/// Create a project rooted at `dir` with one source file at `src/main.mw`.
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

#[test]
fn return_const_travels_the_full_production_path() {
    let temp = TempDir::new("return-const");
    project(
        &temp,
        r#"pub fn answer(): int {
    return 42
}
"#,
    );

    let output = run_in(&temp, &["run", "answer"]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

#[test]
fn return_const_jsonl_is_canonical() {
    let temp = TempDir::new("return-const-jsonl");
    project(
        &temp,
        r#"pub fn answer(): int {
    return 42
}
"#,
    );

    let output = run_in(&temp, &["run", "answer", "--format", "jsonl"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"data\":42,\"kind\":\"run\",\"outcome\":\"value\"}\n"
    );
}

#[test]
fn a_type_mismatch_is_a_source_diagnostic() {
    let temp = TempDir::new("type-mismatch");
    project(
        &temp,
        r#"pub fn answer(): int {
    return true
}
"#,
    );

    let output = run_in(&temp, &["run", "answer", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(r#""outcome":"diagnostic""#),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("check.type"),
        "{output:?}"
    );
}

#[test]
fn a_missing_export_is_a_usage_error() {
    let temp = TempDir::new("missing-export");
    project(
        &temp,
        r#"pub fn answer(): int {
    return 42
}
"#,
    );

    let output = run_in(&temp, &["run", "nope"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn locals_arithmetic_and_control_flow_compute_a_value() {
    let temp = TempDir::new("compute");
    project(
        &temp,
        r#"pub fn compute(): int {
    const a = 3
    var b = 4
    b = b * a
    if b > 10 { return b + 1 }
    return b
}
"#,
    );

    let output = run_in(&temp, &["run", "compute"]);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // b = 12, 12 > 10, so returns 13.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "13\n");
}

#[test]
fn a_while_loop_sums() {
    let temp = TempDir::new("sum-loop");
    project(
        &temp,
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
    );

    let output = run_in(&temp, &["run", "total"]);
    assert!(output.status.success(), "{output:?}");
    // 0 + 1 + 2 + 3 + 4 = 10.
    assert_eq!(String::from_utf8_lossy(&output.stdout), "10\n");
}

#[test]
fn short_circuit_boolean_logic() {
    let temp = TempDir::new("andor");
    project(
        &temp,
        r#"pub fn andor(): bool {
    const t = true
    const f = false
    return t and (f or t)
}
"#,
    );

    let output = run_in(&temp, &["run", "andor"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "true\n");
}

#[test]
fn runtime_overflow_is_a_source_mapped_fault() {
    let temp = TempDir::new("overflow");
    project(
        &temp,
        r#"pub fn over(): int {
    const big = 9223372036854775807
    return big + 1
}
"#,
    );

    let output = run_in(&temp, &["run", "over", "--format", "jsonl"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"fault""#), "{output:?}");
    assert!(stdout.contains("run.overflow"), "{output:?}");
}

/// Integer `/` truncates toward zero through the full production path.
#[test]
fn integer_division_truncates_toward_zero() {
    let temp = TempDir::new("div");
    project(
        &temp,
        r#"pub fn q(a: int, b: int): int {
    return a / b
}
"#,
    );
    let output = run_in(&temp, &["run", "q", "--", "-7", "2"]);
    assert!(output.status.success(), "run failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-3\n");
}

/// The implemented `string` and `bytes` conversions travel the full path and
/// render canonically.
#[test]
fn implemented_scalar_conversions_travel_the_full_path() {
    let temp = TempDir::new("conv");
    project(
        &temp,
        r#"pub fn asString(n: int): string {
    return string(n)
}

pub fn flag(b: bool): string {
    return string(b)
}

pub fn asBytes(s: string): bytes {
    return bytes(s)
}
"#,
    );
    let n = run_in(&temp, &["run", "asString", "--", "-7"]);
    assert!(n.status.success(), "{n:?}");
    assert_eq!(String::from_utf8_lossy(&n.stdout), "-7\n");

    let b = run_in(&temp, &["run", "flag", "--", "true"]);
    assert_eq!(String::from_utf8_lossy(&b.stdout), "true\n");

    // "hi" is 0x6869; bytes render as 0x-prefixed lowercase hex.
    let by = run_in(&temp, &["run", "asBytes", "--", "hi"]);
    assert!(by.status.success(), "{by:?}");
    assert_eq!(String::from_utf8_lossy(&by.stdout), "0x6869\n");
}

/// Direct byte-literal spelling is parser-recognized but not executable; the
/// current bytes constructor remains available through the production path.
#[test]
fn byte_literal_boundary_and_reference_are_exact() {
    let temp = TempDir::new("byte-literal-boundary");
    project(
        &temp,
        r#"pub fn value(): bytes {
    return b"key"
}
"#,
    );

    let literal = run_in(&temp, &["run", "value", "--format", "jsonl"]);
    assert!(!literal.status.success(), "{literal:?}");
    let stdout = String::from_utf8_lossy(&literal.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{literal:?}");
    assert!(
        stdout.contains(r#""code":"check.unsupported""#),
        "{literal:?}"
    );

    project(
        &temp,
        r#"pub fn value(): bytes {
    return bytes("key")
}
"#,
    );

    let constructor = run_in(&temp, &["run", "value"]);
    assert!(constructor.status.success(), "{constructor:?}");
    assert_eq!(String::from_utf8_lossy(&constructor.stdout), "0x6b6579\n");

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

#[test]
fn std_paths_use_ordinary_project_module_resolution() {
    let missing = source_project(
        "std-module-missing",
        &[(
            "main.mw",
            r#"module main

pub fn run(): string {
    return std::text::decorate("Marrow")
}
"#,
        )],
    );
    let diagnostic = run_diagnostic_code(&missing, "main.run");
    assert!(
        diagnostic.contains(r#""code":"check.type""#),
        "{diagnostic}"
    );

    let declared = source_project(
        "std-module-declared",
        &[
            (
                "main.mw",
                r#"module main

pub fn run(): string {
    return std::text::decorate("Marrow")
}
"#,
            ),
            (
                "std/text.mw",
                r#"module std::text

pub fn decorate(value: string): string {
    return $"[{value}]"
}
"#,
            ),
        ],
    );
    let output = run_in(&declared, &["run", "main.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[Marrow]\n");

    let private = source_project(
        "std-module-private",
        &[
            (
                "main.mw",
                r#"module main

pub fn run(): string {
    return std::text::decorate("Marrow")
}
"#,
            ),
            (
                "std/text.mw",
                r#"module std::text

fn decorate(value: string): string {
    return $"[{value}]"
}
"#,
            ),
        ],
    );
    let diagnostic = run_diagnostic_code(&private, "main.run");
    assert!(
        diagnostic.contains(r#""code":"check.visibility""#),
        "{diagnostic}"
    );

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let standard = normalize(include_str!("../../../docs/language/standard-library.md"));
    let source = normalize(include_str!("../../../docs/language/source-and-syntax.md"));
    let functions = normalize(include_str!(
        "../../../docs/language/modules-and-functions.md"
    ));
    let future = normalize(include_str!(
        "../../../docs/future/source-standard-library.md"
    ));

    assert!(standard.contains("The current toolchain supplies no `std::` modules."));
    assert!(standard.contains(
        "An absent module or function reports `check.type`; a cross-module call to a non-public function reports `check.visibility`."
    ));
    assert!(
        standard
            .contains("A project-declared `std::` path is project code, not an ambient library.")
    );
    assert!(!standard.contains("std::text::trim"));
    assert!(!source.contains("declared library names"));
    assert!(source.contains(
        "An absent module or function reports `check.type`; a cross-module call to a non-public function reports `check.visibility`."
    ));
    assert!(!source.contains("std::text::contains"));
    assert!(!functions.contains("`std::` operations"));
    assert!(!functions.contains("host-provided standard-library function"));
    assert!(!future.contains("current standard library is implemented"));
}

#[test]
fn project_and_generic_function_arguments_are_positional() {
    let positional = source_project(
        "positional-project-call",
        &[(
            "main.mw",
            r#"module main

fn decorate(value: string): string {
    return $"[{value}]"
}

pub fn run(): string {
    return decorate("Marrow")
}
"#,
        )],
    );
    let output = run_in(&positional, &["run", "main.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[Marrow]\n");

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
        let temp = source_project(name, &[("main.mw", source)]);
        let diagnostic = run_diagnostic_code(&temp, "main.run");
        assert!(
            diagnostic.contains(r#""code":"check.type""#),
            "{name}: {diagnostic}"
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
fn export_output_and_scalar_conversion_rendering_are_distinct() {
    let resource = source_project(
        "resource-export-rendering",
        &[(
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
        )],
    );
    let output = run_in(
        &resource,
        &["run", "main.draft", "--", "Small Gods", "Pratchett"],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{title: Small Gods, author: Pratchett}\n"
    );

    let unsupported = source_project(
        "resource-string-rejection",
        &[(
            "main.mw",
            r#"module main

resource Book {
    required title: string
}

pub fn text(): string {
    return string(Book(title: "Marrow"))
}
"#,
        )],
    );
    let diagnostic = run_diagnostic_code(&unsupported, "main.text");
    assert!(
        diagnostic.contains(r#""code":"check.unsupported""#),
        "{diagnostic}"
    );

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let builtins = normalize(include_str!("../../../docs/language/builtins.md"));
    assert!(builtins.contains(
        "`marrow run` renders every admitted export result through the canonical value renderer."
    ));
    assert!(builtins.contains(
        "`string(...)` and interpolation use the same scalar, enum, and identity renderings but reject bare aggregates and presence optionals."
    ));
    assert!(!builtins.contains("Resources and local or durable trees have no direct rendering"));
}

#[test]
fn rejected_conversion_and_decimal_literal_codes_are_exact() {
    for (name, source, code) in [
        (
            "int-from-text",
            r#"module main

pub fn value(): int {
    return int("1")
}
"#,
            "check.unsupported",
        ),
        (
            "bool-from-int",
            r#"module main

pub fn value(): bool {
    return bool(1)
}
"#,
            "check.unsupported",
        ),
        (
            "decimal-call",
            r#"module main

pub fn value(): int {
    const converted = decimal(1)
    return 0
}
"#,
            "check.type",
        ),
        (
            "error-code-call",
            r#"module main

pub fn value(): int {
    const converted = ErrorCode("run.example")
    return 0
}
"#,
            "check.type",
        ),
        (
            "decimal-literal",
            r#"module main

pub fn value(): int {
    return 1.5
}
"#,
            "check.unsupported",
        ),
    ] {
        let temp = source_project(name, &[("main.mw", source)]);
        let diagnostic = run_diagnostic_code(&temp, "main.value");
        let expected = format!(r#""code":"{code}""#);
        assert!(diagnostic.contains(&expected), "{name}: {diagnostic}");
    }

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let types = normalize(include_str!("../../../docs/language/types-and-values.md"));
    let syntax = normalize(include_str!("../../../docs/language/source-and-syntax.md"));
    let builtins = normalize(include_str!("../../../docs/language/builtins.md"));

    assert!(types.contains(
        "`int(\"1\")` and `bool(1)` are examples. `decimal` and `ErrorCode` have no current callable scalar owner, so `decimal(1)` and `ErrorCode(\"run.example\")` report `check.type`."
    ));
    assert!(syntax.contains("| Decimal (**future**) |"));
    assert!(!syntax.contains("| Decimal |"));
    assert!(!builtins.contains("std::bytes::toText"));
    assert!(!types.contains("| `bool` | `bool`, or `int` equal to `0` or `1` |"));
}

#[test]
fn reference_excludes_removed_throwable_channel_and_keywords() {
    let identifiers = source_project(
        "removed-channel-identifiers",
        &[(
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
        )],
    );
    let output = run_in(&identifiers, &["run", "main.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "4\n");

    for (name, statement) in [
        ("removed-throw-statement", "throw \"failure\""),
        ("removed-catch-clause", "catch { return 0 }"),
    ] {
        let source =
            format!("module main\n\npub fn run(): int {{\n    {statement}\n    return 1\n}}\n");
        let temp = source_project(name, &[("main.mw", &source)]);
        let diagnostic = run_diagnostic_code(&temp, "main.run");
        assert!(
            diagnostic.contains(r#""code":"parse.syntax""#),
            "{name}: {diagnostic}"
        );
    }

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let readme = normalize(include_str!("../../../docs/language/README.md"));
    let standard = normalize(include_str!("../../../docs/language/standard-library.md"));
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
    assert!(!functions.contains("may read or write durable places, throw"));
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

/// A terminal value literal must be in canonical form: the `bytes` decoder admits
/// only a `0x`-prefixed even-length lowercase-hex string, and the `bool` decoder
/// only `true`/`false`. A noncanonical spelling — uppercase hex, a missing `0x`
/// prefix, an odd hex length, or `1` for a bool — is a usage error (exit 2), never
/// a silent coercion.
#[test]
fn a_noncanonical_terminal_value_literal_is_a_usage_error() {
    let temp = TempDir::new("noncanonical");
    project(
        &temp,
        r#"pub fn firstByte(b: bytes): int {
    return 0
}

pub fn flag(b: bool): bool {
    return b
}
"#,
    );
    for (export, arg) in [
        ("firstByte", "0xAB"),  // uppercase hex
        ("firstByte", "abcd"),  // missing 0x prefix
        ("firstByte", "0xabc"), // odd length
        ("flag", "1"),          // bool spelled as an int
        ("flag", "True"),       // bool wrong case
    ] {
        let output = run_in(&temp, &["run", export, "--", arg]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{export} {arg:?} must be a usage error: {output:?}"
        );
    }
    // The canonical forms are accepted, so the rejection is of the spelling, not
    // the type.
    assert!(
        run_in(&temp, &["run", "firstByte", "--", "0xabcd"])
            .status
            .success()
    );
    assert!(
        run_in(&temp, &["run", "flag", "--", "false"])
            .status
            .success()
    );
}

/// A non-terminating loop exhausts the per-invocation instruction budget and
/// faults with `run.budget` — the VM's dynamic-limit backstop — rather than running
/// forever. There is no runner or environment override.
#[test]
fn nonterminating_loop_faults_on_the_instruction_budget() {
    let temp = TempDir::new("budget");
    project(
        &temp,
        r#"pub fn spin() {
    var n: int = 0
    while true {
        n = n + 1
    }
}
"#,
    );
    let output = run_in(&temp, &["run", "spin", "--format", "jsonl"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"fault""#), "{output:?}");
    assert!(stdout.contains("run.budget"), "{output:?}");
}

/// The checked-arithmetic form: the success path binds the result; each fault
/// runs its diverging arm.
#[test]
fn checked_arithmetic_success_and_each_arm() {
    let temp = TempDir::new("checked");
    project(
        &temp,
        r#"pub fn safeMul(a: int, b: int): int {
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
"#,
    );
    // Success paths.
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "safeMul", "--", "6", "7"]).stdout),
        "42\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "safeDiv", "--", "20", "4"]).stdout),
        "5\n"
    );
    // out_of_range arm: 2^62 * 4 overflows.
    assert_eq!(
        String::from_utf8_lossy(
            &run_in(&temp, &["run", "safeMul", "--", "4611686018427387904", "4"]).stdout
        ),
        "-1\n"
    );
    // zero_divisor arm.
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "safeDiv", "--", "1", "0"]).stdout),
        "0\n"
    );
    // out_of_range arm of division: i64::MIN / -1.
    assert_eq!(
        String::from_utf8_lossy(
            &run_in(
                &temp,
                &["run", "safeDiv", "--", "-9223372036854775808", "-1"]
            )
            .stdout
        ),
        "-1\n"
    );
}

/// Complex nested procedural code reads clearly with the checked form, without
/// combinator ceremony: a running total that both guards overflow and short-circuits.
#[test]
fn checked_reads_clearly_in_nested_procedural_code() {
    let temp = TempDir::new("checked-nested");
    project(
        &temp,
        r#"pub fn boundedFactorial(n: int, cap: int): int {
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
"#,
    );
    assert_eq!(
        String::from_utf8_lossy(
            &run_in(&temp, &["run", "boundedFactorial", "--", "5", "1000000"]).stdout
        ),
        "120\n"
    );
    // Overflow guard fires before native overflow: with the cap just below
    // i64::MAX, 20! (2.4e18) stays under it but 21! overflows and runs the arm.
    assert_eq!(
        String::from_utf8_lossy(
            &run_in(
                &temp,
                &[
                    "run",
                    "boundedFactorial",
                    "--",
                    "100",
                    "9000000000000000000"
                ]
            )
            .stdout
        ),
        "-1\n"
    );
    // Cap short-circuit.
    assert_eq!(
        String::from_utf8_lossy(
            &run_in(&temp, &["run", "boundedFactorial", "--", "20", "100"]).stdout
        ),
        "100\n"
    );
}

/// A checked form whose arm does not diverge, or that omits a required arm, is a
/// source diagnostic.
#[test]
fn checked_form_arm_rules_are_diagnostics() {
    let temp = TempDir::new("checked-bad");
    // Non-diverging out_of_range arm.
    project(
        &temp,
        r#"pub fn bad(a: int, b: int): int {
    const p: int = checked a + b
        on out_of_range {
            const x: int = 0
        }
    return p
}
"#,
    );
    let out = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "1", "2"]);
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(r#""outcome":"diagnostic""#), "{out:?}");
    assert!(s.contains("check.type"), "{out:?}");

    // Missing zero_divisor arm on a checked division.
    project(
        &temp,
        r#"pub fn bad(a: int, b: int): int {
    return checked a / b
        on out_of_range return -1
}
"#,
    );
    let out = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "1", "2"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("check.type"),
        "{out:?}"
    );
}

/// A checked `/`/`%` whose divisor is a provably-nonzero integer literal takes no
/// `on zero_divisor` arm (the fault is dead), runs correctly, and still arms the
/// live `on out_of_range` overflow. A supplied dead arm is rejected; a non-literal
/// or literal-zero divisor still requires the arm.
#[test]
fn checked_division_by_a_nonzero_literal_drops_the_dead_zero_arm() {
    let temp = TempDir::new("checked-litdiv");

    // No zero_divisor arm needed; runs.
    project(
        &temp,
        r#"pub fn half(x: int): int {
    const q: int = checked x / 100
        on out_of_range return -1
    return q
}
"#,
    );
    let out = run_in(&temp, &["run", "half", "--format", "jsonl", "--", "500"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(r#""data":5"#),
        "{out:?}"
    );

    // The out_of_range arm stays live: i64::MIN / -1 overflows into it.
    project(
        &temp,
        r#"pub fn neg(x: int): int {
    const q: int = checked x / -1
        on out_of_range return 777
    return q
}
"#,
    );
    let out = run_in(
        &temp,
        &[
            "run",
            "neg",
            "--format",
            "jsonl",
            "--",
            "-9223372036854775808",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(r#""data":777"#),
        "{out:?}"
    );

    // A supplied `on zero_divisor` arm on a literal-nonzero divisor is a dead arm.
    project(
        &temp,
        r#"pub fn dead(x: int): int {
    const q: int = checked x / 100
        on out_of_range return -1
        on zero_divisor return 0
    return q
}
"#,
    );
    let out = run_in(&temp, &["run", "dead", "--format", "jsonl", "--", "1"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("check.type"),
        "{out:?}"
    );

    // A non-literal divisor and a literal-zero divisor still require the arm.
    for body in [
        "pub fn f(x: int, d: int): int {\n    return checked x / d\n        on out_of_range return -1\n}\n",
        "pub fn f(x: int): int {\n    return checked x / 0\n        on out_of_range return -1\n}\n",
    ] {
        project(&temp, body);
        let out = run_in(&temp, &["run", "f", "--format", "jsonl", "--", "1", "2"]);
        assert!(!out.status.success(), "{body}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("check.type"),
            "{body}"
        );
    }
}

/// The closed pure text floor: isEmpty / contains / trim.
#[test]
fn text_floor_builtins_travel_the_full_path() {
    let temp = TempDir::new("textfloor");
    project(
        &temp,
        r#"pub fn empty(s: string): bool {
    return isEmpty(trim(s))
}

pub fn has(h: string, n: string): bool {
    return contains(h, n)
}
"#,
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "empty", "--", "   "]).stdout),
        "true\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "empty", "--", " x "]).stdout),
        "false\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "has", "--", "hello", "ell"]).stdout),
        "true\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "has", "--", "hello", "xyz"]).stdout),
        "false\n"
    );
}

/// Interpolated strings travel the full path: literal segments carry their
/// decoded text and doubled-brace escapes, and scalar holes use the same
/// canonical rendering as `string(value)`.
#[test]
fn interpolation_renders_holes_and_escapes() {
    let temp = TempDir::new("interp");
    project(
        &temp,
        r#"pub fn greet(id: int, on: bool): string {
    return $"id: {id} ok={on}!"
}

pub fn braces(): string {
    return $"a {{ b }}\tc"
}

pub fn empty(): string {
    return $""
}
"#,
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "greet", "--", "7", "true"]).stdout),
        "id: 7 ok=true!\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "braces"]).stdout),
        "a { b }\tc\n"
    );
    // The text renderer suppresses an empty line, so an empty interpolation
    // compiles and runs but prints nothing.
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "empty"]).stdout),
        ""
    );
}

/// A hole whose value has no current canonical rendering is a typed
/// `check.unsupported`, matching the `string(value)` boundary.
#[test]
fn interpolation_rejects_an_unrenderable_hole() {
    let temp = TempDir::new("interp-bad");
    project(
        &temp,
        r#"pub fn bad(d: decimal): string {
    return $"v: {d}"
}
"#,
    );
    let output = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "1.5"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{output:?}");
    assert!(stdout.contains("check.unsupported"), "{output:?}");
}

/// A chained `if const` proves each subject present left to right (short-circuit),
/// scopes each binding rightward and into the then block, and takes the else tail
/// when any subject is absent or the trailing condition is false.
#[test]
fn if_const_chain_short_circuits_and_scopes_rightward() {
    let temp = TempDir::new("ifconst-chain");
    project(
        &temp,
        r#"fn maybe(n: int): int? {
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
"#,
    );
    // present: x=5, y=maybe(6)=6, 5>2 true.
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "f", "--", "5"]).stdout),
        "506\n"
    );
    // trailing condition false: x=2, 2>2 false.
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "f", "--", "2"]).stdout),
        "-1\n"
    );
    // first subject absent short-circuits.
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "f", "--", "0"]).stdout),
        "-1\n"
    );
}

/// A let-else binding runs its diverging `else` when the subject is absent and
/// otherwise binds the present value for the rest of the block; a `var` let-else
/// binds mutably; a non-diverging `else` is a typed `check.type`.
#[test]
fn let_else_binds_present_and_requires_a_diverging_else() {
    let temp = TempDir::new("let-else");
    project(
        &temp,
        r#"fn maybe(n: int): int? {
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
"#,
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "f", "--", "5"]).stdout),
        "105\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_in(&temp, &["run", "f", "--", "0"]).stdout),
        "-1\n"
    );

    let bad = TempDir::new("let-else-bad");
    project(
        &bad,
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
    );
    let output = run_in(&bad, &["run", "f", "--format", "jsonl", "--", "5"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("check.type"), "{stdout}");
}

/// A let-else binding is out of scope inside its own `else` — the absent edge,
/// where the binding is never established. A reference to it there is a scoped
/// unknown-name `check.type`, never an uninitialized-slot image rejection, and it
/// does not shadow an outer binding of the same name that the `else` should see.
#[test]
fn let_else_binding_is_out_of_scope_in_its_own_else() {
    // (a) referencing the binding in its own else is a clean check.type unknown
    // name (a checker rejection), not an image.function artifact rejection.
    let scoped = TempDir::new("let-else-scope");
    project(
        &scoped,
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
    );
    let output = run_in(&scoped, &["run", "f", "--format", "jsonl", "--", "5"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("check.type"), "{stdout}");
    assert!(!stdout.contains("image.function"), "{stdout}");
    assert!(!stdout.contains("artifact_rejected"), "{stdout}");

    // (b) the else sees the outer binding, not the not-yet-established inner one of
    // the same name.
    let shadow = TempDir::new("let-else-shadow");
    project(
        &shadow,
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
    );
    // absent: the else returns the outer n (7).
    assert_eq!(
        String::from_utf8_lossy(&run_in(&shadow, &["run", "f", "--", "0"]).stdout),
        "7\n"
    );
    // present: the inner binding is in scope for the continuation (5).
    assert_eq!(
        String::from_utf8_lossy(&run_in(&shadow, &["run", "f", "--", "5"]).stdout),
        "5\n"
    );
}

/// `string` comparisons order lexicographically through the full path.
#[test]
fn string_comparison_orders_lexicographically() {
    let temp = TempDir::new("strcmp");
    project(
        &temp,
        r#"pub fn before(a: string, b: string): bool {
    return a < b
}
"#,
    );
    let yes = run_in(&temp, &["run", "before", "--", "apple", "banana"]);
    assert!(yes.status.success(), "run failed: {yes:?}");
    assert_eq!(String::from_utf8_lossy(&yes.stdout), "true\n");
    let no = run_in(&temp, &["run", "before", "--", "banana", "apple"]);
    assert_eq!(String::from_utf8_lossy(&no.stdout), "false\n");
}

#[test]
fn integer_division_by_zero_is_a_source_mapped_fault() {
    let temp = TempDir::new("divzero");
    project(
        &temp,
        r#"pub fn q(a: int, b: int): int {
    return a / b
}
"#,
    );
    let output = run_in(&temp, &["run", "q", "--format", "jsonl", "--", "1", "0"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"fault""#), "{output:?}");
    assert!(stdout.contains("run.divide_by_zero"), "{output:?}");
}

/// `unreachable(...)` diverges, so it stands as the final statement of a
/// value-returning function whose earlier branches cover every real case, and it
/// runs the returning path normally.
#[test]
fn unreachable_satisfies_exhaustive_return_and_runs_the_real_path() {
    let temp = TempDir::new("unreach-ok");
    project(
        &temp,
        r#"pub fn sign(n: int): int {
    if n > 0 { return 1 }
    if n < 0 { return -1 }
    if n == 0 { return 0 }
    unreachable("n is int, so one branch always returns")
}
"#,
    );
    let output = run_in(&temp, &["run", "sign", "--", "-5"]);
    assert!(output.status.success(), "run failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-1\n");
}

/// Reaching an `unreachable` faults with `run.unreachable`; the text output carries
/// the static author text, while the typed JSONL surface stays code and span.
#[test]
fn unreachable_faults_and_carries_static_text() {
    let temp = TempDir::new("unreach-fault");
    let source = r#"pub fn boom(hit: bool): int {
    if hit {
        unreachable("the invariant broke")
    }
    return 0
}
"#;
    project(&temp, source);

    let jsonl = run_in(&temp, &["run", "boom", "--format", "jsonl", "--", "true"]);
    assert!(!jsonl.status.success());
    let jsonl_out = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl_out.contains(r#""outcome":"fault""#), "{jsonl:?}");
    assert!(jsonl_out.contains("run.unreachable"), "{jsonl:?}");
    assert!(
        !jsonl_out.contains("the invariant broke"),
        "static text stays out of the typed JSONL grammar: {jsonl:?}"
    );

    let text = run_in(&temp, &["run", "boom", "--", "true"]);
    assert!(!text.status.success());
    let text_out = String::from_utf8_lossy(&text.stdout);
    assert!(text_out.contains("run.unreachable"), "{text:?}");
    assert!(text_out.contains("the invariant broke"), "{text:?}");
}

/// `unreachable` requires a static string literal, so a computed argument is a
/// source diagnostic, not a runtime value.
#[test]
fn unreachable_rejects_a_computed_argument() {
    let temp = TempDir::new("unreach-arg");
    project(
        &temp,
        r#"pub fn bad(s: string): int {
    unreachable(s)
}
"#,
    );
    let output = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "x"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{output:?}");
    assert!(stdout.contains("check.type"), "{output:?}");
}

/// Every canonically renderable value is an interpolation hole and rides
/// `string(...)`: temporals, enums (with payloads), and bytes use the same
/// canonical text as `marrow run`. A record, list, map, or optional hole is not
/// renderable and is refused.
#[test]
fn interpolation_and_string_render_every_scalar_and_enum() {
    let temp = TempDir::new("interp-canon");
    project(
        &temp,
        r#"enum Shape {
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
"#,
    );

    let temporal = run_in(&temp, &["run", "temporal"]);
    assert!(temporal.status.success(), "{temporal:?}");
    assert_eq!(
        String::from_utf8_lossy(&temporal.stdout),
        "2026-08-01 2026-07-15T17:00:00Z in PT259200S\n"
    );

    let shape = run_in(&temp, &["run", "shape"]);
    assert_eq!(
        String::from_utf8_lossy(&shape.stdout),
        "Shape::circle(5) and Shape::dot\n"
    );

    let as_text = run_in(&temp, &["run", "asText"]);
    assert_eq!(String::from_utf8_lossy(&as_text.stdout), "2026-08-01\n");

    // "hi" is 0x6869; a bytes hole renders as canonical hex.
    let bytes = run_in(&temp, &["run", "bytesHole", "--", "hi"]);
    assert_eq!(String::from_utf8_lossy(&bytes.stdout), "0x6869\n");

    // A list hole is not a renderable value.
    project(
        &temp,
        "module main\n\npub fn f(): string {\n    var xs: List<int> = List(1, 2)\n    return $\"{xs}\"\n}\n",
    );
    let list = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    assert!(!list.status.success());
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("check.unsupported"),
        "{list:?}"
    );
}

/// `Option`/`Result` are enums whose payload can be an aggregate. Interpolation
/// and `marrow run` text use the canonical enum rendering, including aggregate
/// payloads, without a runtime fault; JSONL preserves the structured value.
#[test]
fn generic_enum_payloads_render_totally() {
    let temp = TempDir::new("generic-enum-render");
    project(
        &temp,
        r#"struct Point {
    x: int
    y: int
}

pub fn interp(): string {
    const p: Option<Point> = some(Point(x: 1, y: 2))
    return $"p is {p}"
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
    );

    // Interpolation renders an aggregate enum payload through the canonical owner.
    let interp = run_in(&temp, &["run", "interp"]);
    assert!(interp.status.success(), "{interp:?}");
    assert_eq!(
        String::from_utf8_lossy(&interp.stdout),
        "p is Option::some({x: 1, y: 2})\n"
    );

    // Returned generic-enum values use the same canonical text through `marrow run`.
    for (export, expected) in [
        ("retOpt", "Option::some({x: 1, y: 2})\n"),
        ("retNone", "Option::none\n"),
        ("retResult", "Result::ok({x: 3, y: 4})\n"),
        ("retNested", "Option::some(Option::some(7))\n"),
    ] {
        let out = run_in(&temp, &["run", export]);
        assert!(out.status.success(), "{export}: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{export}");
    }

    // JSONL keeps the structured enum object.
    let jsonl = run_in(&temp, &["run", "retOpt", "--format", "jsonl"]);
    assert!(
        String::from_utf8_lossy(&jsonl.stdout)
            .contains(r#""data":{"enum":"Option","member":"some","payload":[{"x":1,"y":2}]}"#),
        "{jsonl:?}"
    );
}

/// A bare presence-optional (`T?`) hole stays refused at check — only a scalar, enum,
/// or identity is a renderable hole; the `Option<T>` enum renders, the `T?` does not.
#[test]
fn a_bare_presence_optional_hole_is_still_refused() {
    let temp = TempDir::new("optional-hole");
    project(
        &temp,
        "module main\n\nfn maybe(n: int): int? {\n    if n > 0 { return n }\n    return absent\n}\n\npub fn f(n: int): string {\n    return $\"{maybe(n)}\"\n}\n",
    );
    let out = run_in(&temp, &["run", "f", "--format", "jsonl", "--", "1"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("check.unsupported"),
        "{out:?}"
    );
}

/// `todo("...")` mirrors `unreachable`: it diverges (so it satisfies exhaustive
/// return), it requires a static string literal, and reaching it faults with the
/// distinct `run.todo` code carrying the author text.
#[test]
fn todo_diverges_and_faults_run_todo() {
    let temp = TempDir::new("todo");

    // Divergence satisfies the "all paths return" check and the real path runs.
    project(
        &temp,
        r#"pub fn classify(n: int): int {
    if n > 0 { return 1 }
    todo("handle non-positive inputs")
}
"#,
    );
    let ok = run_in(&temp, &["run", "classify", "--", "7"]);
    assert!(ok.status.success(), "{ok:?}");
    assert_eq!(String::from_utf8_lossy(&ok.stdout), "1\n");

    // Reaching it faults with run.todo; the typed surface stays code + span, the text
    // surface carries the author string.
    let jsonl = run_in(&temp, &["run", "classify", "--format", "jsonl", "--", "-1"]);
    assert!(!jsonl.status.success());
    let jsonl_out = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl_out.contains(r#""outcome":"fault""#), "{jsonl:?}");
    assert!(jsonl_out.contains("run.todo"), "{jsonl:?}");
    assert!(
        !jsonl_out.contains("handle non-positive"),
        "static text stays out of the typed JSONL grammar: {jsonl:?}"
    );
    let text = run_in(&temp, &["run", "classify", "--", "-1"]);
    let text_out = String::from_utf8_lossy(&text.stdout);
    assert!(text_out.contains("run.todo"), "{text:?}");
    assert!(text_out.contains("handle non-positive inputs"), "{text:?}");

    // A computed argument is rejected, like `unreachable`.
    project(
        &temp,
        r#"pub fn bad(s: string): int {
    todo(s)
}
"#,
    );
    let out = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "x"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("check.type"),
        "{out:?}"
    );
}

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

fn run_records(export: &str) -> String {
    let temp = TempDir::new(&format!("records-{export}"));
    project(&temp, RECORDS_SOURCE);
    let output = run_in(&temp, &["run", export]);
    assert!(
        output.status.success(),
        "run {export} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn required_field_read() {
    assert_eq!(run_records("titleOf"), "hello\n");
}

#[test]
fn present_sparse_field_coalesces_to_itself() {
    assert_eq!(run_records("bodyOrDefault"), "there\n");
}

#[test]
fn vacant_sparse_field_coalesces_to_default() {
    assert_eq!(run_records("missingBody"), "none\n");
}

#[test]
fn if_const_binds_a_present_optional() {
    assert_eq!(run_records("guardedBody"), "yo\n");
}

#[test]
fn an_absent_optional_return_renders_absent() {
    assert_eq!(run_records("maybe"), "absent\n");
}

// --- Module constants (C00). ---

#[test]
fn a_module_constant_folds_into_a_function() {
    let temp = source_project(
        "module-const",
        &[(
            "main.mw",
            r#"module main

const MAX: int = 100

pub fn cap(): int {
    return MAX + 1
}
"#,
        )],
    );
    let output = run_in(&temp, &["run", "main.cap"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "101\n");
}

#[test]
fn a_negated_integer_constant_is_allowed() {
    let temp = source_project(
        "neg-const",
        &[(
            "main.mw",
            r#"module main

const MIN = -5

pub fn floor(): int {
    return MIN
}
"#,
        )],
    );
    let output = run_in(&temp, &["run", "main.floor"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-5\n");
}

#[test]
fn a_constant_type_annotation_must_match_its_value() {
    let temp = source_project(
        "const-type",
        &[(
            "main.mw",
            r#"module main

const FLAG: bool = 1

pub fn run(): bool {
    return FLAG
}
"#,
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.type"));
}

#[test]
fn a_non_literal_constant_is_unsupported() {
    let temp = source_project(
        "const-nonliteral",
        &[(
            "main.mw",
            r#"module main

const SUM = 1 + 2

pub fn run(): int {
    return SUM
}
"#,
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.unsupported"));
}

#[test]
fn a_module_constant_is_private_to_its_module() {
    // `SECRET` is declared in `lib`; referencing it unqualified from `main` is not
    // in scope, and a qualified constant reference is not a supported form.
    let temp = source_project(
        "const-private",
        &[
            ("lib.mw", "module lib\n\nconst SECRET = 7\n"),
            (
                "main.mw",
                r#"module main

pub fn run(): int {
    return SECRET
}
"#,
            ),
        ],
    );
    let stdout = run_diagnostic_code(&temp, "main.run");
    assert!(stdout.contains("check.type"), "{stdout}");
}

#[test]
fn a_duplicate_constant_in_one_module_conflicts() {
    let temp = source_project(
        "dup-const",
        &[(
            "main.mw",
            r#"module main

const K = 1

const K = 2

pub fn run(): int {
    return K
}
"#,
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.name_conflict"));
}

// --- Module-scoped call resolution and `use` imports (C00). ---

/// Write a `marrow.toml` and the named `(relative path, source)` files under a
/// fresh temp project, returning it.
fn source_project(name: &str, files: &[(&str, &str)]) -> TempDir {
    let temp = TempDir::new(name);
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    for (path, source) in files {
        write(&temp.join("src").join(path), source);
    }
    temp
}

/// The first diagnostic code from a failed `marrow run --format jsonl`.
fn run_diagnostic_code(dir: &Path, export: &str) -> String {
    let output = run_in(dir, &["run", export, "--format", "jsonl"]);
    assert!(!output.status.success(), "expected failure: {output:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_use_import_resolves_a_cross_module_call() {
    let temp = source_project(
        "use-import",
        &[
            (
                "mathlib/ops.mw",
                r#"module mathlib::ops

pub fn double(n: int): int {
    return n + n
}
"#,
            ),
            (
                "main.mw",
                r#"module main

use mathlib::ops

pub fn run(): int {
    return ops::double(21)
}
"#,
            ),
        ],
    );
    let output = run_in(&temp, &["run", "main.run"]);
    assert!(
        output.status.success(),
        "cross-module call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

#[test]
fn a_fully_qualified_call_resolves_without_a_use() {
    let temp = source_project(
        "fully-qualified",
        &[
            (
                "mathlib/ops.mw",
                r#"module mathlib::ops

pub fn triple(n: int): int {
    return n + n + n
}
"#,
            ),
            (
                "main.mw",
                r#"module main

pub fn run(): int {
    return mathlib::ops::triple(4)
}
"#,
            ),
        ],
    );
    let output = run_in(&temp, &["run", "main.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "12\n");
}

#[test]
fn a_same_name_function_in_another_module_does_not_conflict() {
    // Two modules each define `helper`; an unqualified call binds the caller's own.
    let temp = source_project(
        "same-name",
        &[
            (
                "a.mw",
                r#"module a

pub fn helper(): int {
    return 1
}
"#,
            ),
            (
                "b.mw",
                r#"module b

fn helper(): int {
    return 2
}

pub fn run(): int {
    return helper()
}
"#,
            ),
        ],
    );
    let output = run_in(&temp, &["run", "b.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "2\n");
}

#[test]
fn a_bare_call_does_not_reach_a_function_in_another_module() {
    // `greet` exists only in `other`; an unqualified call from `main` resolves in
    // `main` alone and is unresolved, not silently bound across the boundary.
    let temp = source_project(
        "bare-foreign",
        &[
            (
                "other.mw",
                r#"module other

pub fn greet(): int {
    return 1
}
"#,
            ),
            (
                "main.mw",
                r#"module main

pub fn run(): int {
    return greet()
}
"#,
            ),
        ],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.type"));
}

#[test]
fn a_qualified_call_to_an_own_module_private_function_resolves() {
    // Qualifying a call with the caller's own module reaches a private function
    // there; visibility only gates crossing a module boundary.
    let temp = source_project(
        "own-qualified-private",
        &[(
            "main.mw",
            r#"module main

fn secret(): int {
    return 7
}

pub fn run(): int {
    return main::secret()
}
"#,
        )],
    );
    let output = run_in(&temp, &["run", "main.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\n");
}

#[test]
fn calling_a_private_function_across_modules_is_a_visibility_error() {
    let temp = source_project(
        "visibility",
        &[
            (
                "lib.mw",
                r#"module lib

fn secret(): int {
    return 1
}
"#,
            ),
            (
                "main.mw",
                r#"module main

pub fn run(): int {
    return lib::secret()
}
"#,
            ),
        ],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.visibility"));
}

#[test]
fn a_use_of_an_unknown_module_is_an_import_error() {
    let temp = source_project(
        "unknown-import",
        &[(
            "main.mw",
            r#"module main

use nope::missing

pub fn run(): int {
    return 1
}
"#,
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.import"));
}

#[test]
fn a_headerless_script_export_runs_by_its_path_derived_name() {
    let temp = source_project(
        "headerless-script-export",
        &[(
            "tools/math.mw",
            r#"pub fn two(): int {
    return 2
}
"#,
        )],
    );
    let output = run_in(&temp, &["run", "tools.math.two"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "2\n");
}

#[test]
fn a_headerless_script_is_not_importable_by_module_path() {
    let temp = source_project(
        "script-not-importable",
        &[
            (
                "lib.mw",
                r#"pub fn helper(): int {
    return 1
}
"#,
            ),
            (
                "main.mw",
                r#"module main

use lib

pub fn run(): int {
    return lib::helper()
}
"#,
            ),
        ],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.import"));
}

#[test]
fn a_module_header_that_disagrees_with_its_path_is_rejected() {
    let temp = source_project(
        "module-path",
        &[(
            "main.mw",
            r#"module wrong

pub fn run(): int {
    return 1
}
"#,
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.module_path"));
}

#[test]
fn a_duplicate_function_name_in_one_module_conflicts() {
    let temp = source_project(
        "dup-in-module",
        &[(
            "main.mw",
            r#"module main

fn helper(): int {
    return 1
}

fn helper(): int {
    return 2
}

pub fn run(): int {
    return helper()
}
"#,
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.name_conflict"));
}

#[test]
fn direct_calls_resolve_forward_and_compute() {
    let temp = TempDir::new("calls");
    // `quad` is declared before `double`, exercising forward resolution.
    project(
        &temp,
        r#"pub fn quad(): int {
    return double(double(5))
}

fn double(n: int): int {
    return n + n
}
"#,
    );
    let output = run_in(&temp, &["run", "quad"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "20\n");
}

#[test]
fn mutual_recursion_is_a_check_time_diagnostic() {
    // Recursion is caught at check time as a source diagnostic, before an image is
    // produced. (The verifier still independently rejects a cyclic image it is
    // handed; that is covered by the verifier's own hostile suite.)
    let temp = TempDir::new("recursion");
    project(
        &temp,
        r#"pub fn ping(): int {
    return pong()
}

fn pong(): int {
    return ping()
}
"#,
    );
    let output = run_in(&temp, &["run", "ping", "--format", "jsonl"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{output:?}");
    assert!(stdout.contains("check.recursion"), "{output:?}");
}

#[test]
fn direct_self_recursion_is_a_check_time_diagnostic() {
    let temp = TempDir::new("self-recursion");
    project(
        &temp,
        r#"pub fn loops(): int {
    return loops()
}
"#,
    );
    let output = run_in(&temp, &["run", "loops", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("check.recursion"),
        "{output:?}"
    );
}

// --- Durable tracer (D00): the durable-run trough. The CLI compiles, verifies,
// and completes the identity of a durable program, but T01's in-process `--store`
// open died at D00, so a durable `run` reports the typed `cli.durable_unsupported`
// trough outcome rather than executing. Durable execution returns as the
// ephemeral-memory preview (E01); the persistent terminal path — one process
// writing a store, a fresh process reading it back — returns at F02b over the
// companion runner, and its end-to-end CLI restart gate returns with it. ---

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

/// A durable export compiles, verifies, mints its identity, and then parks: the
/// CLI reports the typed `cli.durable_unsupported` trough outcome and never opens
/// a store. The reads-or-writes export reaches this only after the whole pipeline
/// (capture → compile → verify → resolve) succeeded, so a park is positive
/// evidence the durable image is well-formed and identity-complete.
#[test]
fn a_durable_export_parks_in_the_trough() {
    let temp = TempDir::new("counter-trough");
    project(&temp, COUNTER_SOURCE);

    // A read-only durable export: `run` mints the fresh identities, then parks.
    let get = run_in(&temp, &["run", "get", "--format", "jsonl", "--", "hits"]);
    assert!(!get.status.success(), "a durable run parks: {get:?}");
    let out = String::from_utf8_lossy(&get.stdout);
    assert!(out.contains(r#""outcome":"error""#), "{get:?}");
    assert!(out.contains("cli.durable_unsupported"), "{get:?}");
    assert!(
        temp.join("marrow.ids").exists(),
        "the mint pre-pass published marrow.ids before parking"
    );

    // A mutating durable export parks the same way.
    let set = run_in(&temp, &["run", "set", "--", "hits", "5"]);
    assert!(!set.status.success(), "{set:?}");
    assert!(
        String::from_utf8_lossy(&set.stdout).contains("cli.durable_unsupported"),
        "{set:?}"
    );
}

/// `--store` no longer names a CLI open path: it died at D00 and returns at F02b.
/// Until then it is an unknown option, a usage error before the command body.
#[test]
fn the_store_flag_is_gone() {
    let temp = TempDir::new("counter-store-flag");
    project(&temp, COUNTER_SOURCE);
    let output = run_in(&temp, &["run", "get", "--store", "s", "--", "hits"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

/// `duration` is a span, not an identity, so it is not in the durable-key set: a
/// duration-keyed store is a source diagnostic, not a runnable graph.
#[test]
fn a_duration_keyed_store_is_a_source_diagnostic() {
    let temp = TempDir::new("dur-key");
    project(
        &temp,
        r#"resource Span {
    required n: int
}

store ^spans[d: duration]: Span

pub fn get(d: duration): int? {
    return ^spans[d].n
}
"#,
    );
    assert!(run_diagnostic_code(&temp, "get").contains("check.type"));
}

/// The checked-in tracer fixture stays a compile/verify/identity fixture: its
/// committed `marrow.ids` is complete, so a durable export travels the full
/// pipeline and parks in the trough (its runtime journey returns at E01/F02b).
#[test]
fn tracer_fixture_compiles_verifies_and_parks() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/v01/conformance/tracer_counter");
    let output = run_in(&fixture, &["run", "get", "--format", "jsonl", "--", "hits"]);
    assert!(!output.status.success(), "{output:?}");
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains(r#""outcome":"error""#), "{output:?}");
    assert!(out.contains("cli.durable_unsupported"), "{output:?}");
}

// --- Named `place` bindings (D02): a source-local binding names one durable entry
// address whose key tuple is evaluated once. A durable export using places travels
// the whole pipeline (parse -> compile -> verify -> resolve) and parks in the
// trough exactly like the inline address forms; execution returns at E01/F02b. ---

const PLACE_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[name: string]: Counter

pub fn bump(name: string, v: int) {
    transaction {
        place p = ^counters[name]
        p = Counter(value: v)
        p.label = "tag"
    }
}

pub fn get(name: string): int? {
    place p = ^counters[name]
    return p.value
}
"#;

/// A durable export written with named `place` bindings compiles, verifies, mints
/// its identities, and parks: the pipeline reaching the trough is positive evidence
/// the place image is well-formed and identity-complete.
#[test]
fn a_place_binding_export_parks_in_the_trough() {
    let temp = TempDir::new("place-trough");
    project(&temp, PLACE_SOURCE);

    let get = run_in(&temp, &["run", "get", "--format", "jsonl", "--", "hits"]);
    assert!(!get.status.success(), "a durable place run parks: {get:?}");
    let out = String::from_utf8_lossy(&get.stdout);
    assert!(out.contains(r#""outcome":"error""#), "{get:?}");
    assert!(out.contains("cli.durable_unsupported"), "{get:?}");
    assert!(
        temp.join("marrow.ids").exists(),
        "the mint pre-pass published marrow.ids before parking"
    );

    let bump = run_in(&temp, &["run", "bump", "--", "hits", "5"]);
    assert!(!bump.status.success(), "{bump:?}");
    assert!(
        String::from_utf8_lossy(&bump.stdout).contains("cli.durable_unsupported"),
        "{bump:?}"
    );
}

/// The checked-in `place_counter` fixture: a complete `marrow.ids`, so a place-based
/// durable export travels the full pipeline and parks in the trough.
#[test]
fn place_fixture_compiles_verifies_and_parks() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/v01/conformance/place_counter");
    let output = run_in(&fixture, &["run", "get", "--format", "jsonl", "--", "hits"]);
    assert!(!output.status.success(), "{output:?}");
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains(r#""outcome":"error""#), "{output:?}");
    assert!(out.contains("cli.durable_unsupported"), "{output:?}");
}
