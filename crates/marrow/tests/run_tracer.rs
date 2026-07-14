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

/// Create a project rooted at `dir` with one module `src/main.mw`.
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
    project(&temp, "pub fn answer(): int\n    return 42\n");

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
    project(&temp, "pub fn answer(): int\n    return 42\n");

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
    project(&temp, "pub fn answer(): int\n    return true\n");

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
    project(&temp, "pub fn answer(): int\n    return 42\n");

    let output = run_in(&temp, &["run", "nope"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn locals_arithmetic_and_control_flow_compute_a_value() {
    let temp = TempDir::new("compute");
    project(
        &temp,
        "pub fn compute(): int\n\
         \x20   const a = 3\n\
         \x20   var b = 4\n\
         \x20   b = b * a\n\
         \x20   if b > 10\n\
         \x20       return b + 1\n\
         \x20   return b\n",
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
        "pub fn total(): int\n\
         \x20   var sum = 0\n\
         \x20   var i = 0\n\
         \x20   while i < 5\n\
         \x20       sum = sum + i\n\
         \x20       i = i + 1\n\
         \x20   return sum\n",
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
        "pub fn ok(): bool\n\
         \x20   const t = true\n\
         \x20   const f = false\n\
         \x20   return t and (f or t)\n",
    );

    let output = run_in(&temp, &["run", "ok"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "true\n");
}

#[test]
fn runtime_overflow_is_a_source_mapped_fault() {
    let temp = TempDir::new("overflow");
    project(
        &temp,
        "pub fn over(): int\n\
         \x20   const big = 9223372036854775807\n\
         \x20   return big + 1\n",
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
        "pub fn q(a: int, b: int): int\n\
         \x20   return a / b\n",
    );
    let output = run_in(&temp, &["run", "q", "--", "-7", "2"]);
    assert!(output.status.success(), "run failed: {output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-3\n");
}

/// The closed scalar conversions travel the full path and render canonically.
#[test]
fn scalar_conversions_travel_the_full_path() {
    let temp = TempDir::new("conv");
    project(
        &temp,
        "pub fn asString(n: int): string\n\
         \x20   return string(n)\n\
         \n\
         pub fn flag(b: bool): string\n\
         \x20   return string(b)\n\
         \n\
         pub fn asBytes(s: string): bytes\n\
         \x20   return bytes(s)\n",
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

/// A non-terminating loop exhausts the per-invocation instruction budget and
/// faults with `run.budget` — the VM's dynamic-limit backstop — rather than running
/// forever. There is no runner or environment override.
#[test]
fn nonterminating_loop_faults_on_the_instruction_budget() {
    let temp = TempDir::new("budget");
    project(
        &temp,
        "pub fn spin()\n\
         \x20   var n: int = 0\n\
         \x20   while true\n\
         \x20       n = n + 1\n",
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
        "pub fn safeMul(a: int, b: int): int\n\
         \x20   const p: int = checked a * b\n\
         \x20       on out_of_range\n\
         \x20           return -1\n\
         \x20   return p\n\
         \n\
         pub fn safeDiv(a: int, b: int): int\n\
         \x20   return checked a / b\n\
         \x20       on out_of_range\n\
         \x20           return -1\n\
         \x20       on zero_divisor\n\
         \x20           return 0\n",
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
        "pub fn boundedFactorial(n: int, cap: int): int\n\
         \x20   var acc: int = 1\n\
         \x20   var i: int = 2\n\
         \x20   while i <= n\n\
         \x20       const next: int = checked acc * i\n\
         \x20           on out_of_range\n\
         \x20               return -1\n\
         \x20       if next > cap\n\
         \x20           return cap\n\
         \x20       acc = next\n\
         \x20       i = i + 1\n\
         \x20   return acc\n",
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
        "pub fn bad(a: int, b: int): int\n\
         \x20   const p: int = checked a + b\n\
         \x20       on out_of_range\n\
         \x20           const x: int = 0\n\
         \x20   return p\n",
    );
    let out = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "1", "2"]);
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(r#""outcome":"diagnostic""#), "{out:?}");
    assert!(s.contains("check.type"), "{out:?}");

    // Missing zero_divisor arm on a checked division.
    project(
        &temp,
        "pub fn bad(a: int, b: int): int\n\
         \x20   return checked a / b\n\
         \x20       on out_of_range\n\
         \x20           return -1\n",
    );
    let out = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "1", "2"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("check.type"),
        "{out:?}"
    );
}

/// The closed pure text floor: isEmpty / contains / trim.
#[test]
fn text_floor_builtins_travel_the_full_path() {
    let temp = TempDir::new("textfloor");
    project(
        &temp,
        "pub fn empty(s: string): bool\n\
         \x20   return isEmpty(trim(s))\n\
         \n\
         pub fn has(h: string, n: string): bool\n\
         \x20   return contains(h, n)\n",
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

/// `string` comparisons order lexicographically through the full path.
#[test]
fn string_comparison_orders_lexicographically() {
    let temp = TempDir::new("strcmp");
    project(
        &temp,
        "pub fn before(a: string, b: string): bool\n\
         \x20   return a < b\n",
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
        "pub fn q(a: int, b: int): int\n\
         \x20   return a / b\n",
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
        "pub fn sign(n: int): int\n\
         \x20   if n > 0\n\
         \x20       return 1\n\
         \x20   if n < 0\n\
         \x20       return -1\n\
         \x20   if n == 0\n\
         \x20       return 0\n\
         \x20   unreachable(\"n is int, so one branch always returns\")\n",
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
    let source = "pub fn boom(hit: bool): int\n\
         \x20   if hit\n\
         \x20       unreachable(\"the invariant broke\")\n\
         \x20   return 0\n";
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
        "pub fn bad(s: string): int\n\
         \x20   unreachable(s)\n",
    );
    let output = run_in(&temp, &["run", "bad", "--format", "jsonl", "--", "x"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""outcome":"diagnostic""#), "{output:?}");
    assert!(stdout.contains("check.type"), "{output:?}");
}

/// A project whose resource, constructor, field reads, optional coalescing, and
/// `if const` guard travel the full path. One source file drives several exports.
const RECORDS_SOURCE: &str = "resource Note\n\
     \x20   required title: string\n\
     \x20   body: string\n\
     \n\
     pub fn titleOf(): string\n\
     \x20   const n = Note(title: \"hello\")\n\
     \x20   return n.title\n\
     \n\
     pub fn bodyOrDefault(): string\n\
     \x20   const n = Note(title: \"hi\", body: \"there\")\n\
     \x20   return n.body ?? \"none\"\n\
     \n\
     pub fn missingBody(): string\n\
     \x20   const n = Note(title: \"hi\")\n\
     \x20   return n.body ?? \"none\"\n\
     \n\
     pub fn guardedBody(): string\n\
     \x20   const n = Note(title: \"hi\", body: \"yo\")\n\
     \x20   if const b = n.body\n\
     \x20       return b\n\
     \x20   return \"none\"\n\
     \n\
     pub fn maybe(): string?\n\
     \x20   return absent\n";

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

#[test]
fn a_qualified_export_in_a_second_module_runs() {
    // Two modules, each with its own public export. The export in `src/math.mw`
    // (module `math`) is invoked by its qualified `module.item` path through the
    // export directory, resolved to its `ExportId`, and looked up in the image by
    // that verified id — never by a source-string dispatch on the name.
    let temp = TempDir::new("qualified-export");
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &temp.join("src").join("main.mw"),
        "pub fn start(): int\n    return 1\n",
    );
    write(
        &temp.join("src").join("math.mw"),
        "pub fn two(): int\n    return 2\n",
    );

    let output = run_in(&temp, &["run", "math.two"]);
    assert!(
        output.status.success(),
        "qualified run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "2\n");

    // The other module's export resolves by its own qualified path too.
    let start = run_in(&temp, &["run", "main.start"]);
    assert!(start.status.success(), "{start:?}");
    assert_eq!(String::from_utf8_lossy(&start.stdout), "1\n");
}

// --- Module constants (C00). ---

#[test]
fn a_module_constant_folds_into_a_function() {
    let temp = multi_module(
        "module-const",
        &[(
            "main.mw",
            "module main\n\nconst MAX: int = 100\n\npub fn cap(): int\n    return MAX + 1\n",
        )],
    );
    let output = run_in(&temp, &["run", "main.cap"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "101\n");
}

#[test]
fn a_negated_integer_constant_is_allowed() {
    let temp = multi_module(
        "neg-const",
        &[(
            "main.mw",
            "module main\n\nconst MIN = -5\n\npub fn floor(): int\n    return MIN\n",
        )],
    );
    let output = run_in(&temp, &["run", "main.floor"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "-5\n");
}

#[test]
fn a_constant_type_annotation_must_match_its_value() {
    let temp = multi_module(
        "const-type",
        &[(
            "main.mw",
            "module main\n\nconst FLAG: bool = 1\n\npub fn run(): bool\n    return FLAG\n",
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.type"));
}

#[test]
fn a_non_literal_constant_is_unsupported() {
    let temp = multi_module(
        "const-nonliteral",
        &[(
            "main.mw",
            "module main\n\nconst SUM = 1 + 2\n\npub fn run(): int\n    return SUM\n",
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.unsupported"));
}

#[test]
fn a_module_constant_is_private_to_its_module() {
    // `SECRET` is declared in `lib`; referencing it unqualified from `main` is not
    // in scope, and a qualified constant reference is not a supported form.
    let temp = multi_module(
        "const-private",
        &[
            ("lib.mw", "module lib\n\nconst SECRET = 7\n"),
            (
                "main.mw",
                "module main\n\npub fn run(): int\n    return SECRET\n",
            ),
        ],
    );
    let stdout = run_diagnostic_code(&temp, "main.run");
    assert!(stdout.contains("check.type"), "{stdout}");
}

#[test]
fn a_duplicate_constant_in_one_module_conflicts() {
    let temp = multi_module(
        "dup-const",
        &[(
            "main.mw",
            "module main\n\nconst K = 1\n\nconst K = 2\n\npub fn run(): int\n    return K\n",
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.name_conflict"));
}

// --- Module-scoped call resolution and `use` imports (C00). ---

/// Write a `marrow.toml` and the named `(relative path, source)` modules under a
/// fresh temp project, returning it.
fn multi_module(name: &str, modules: &[(&str, &str)]) -> TempDir {
    let temp = TempDir::new(name);
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    for (path, source) in modules {
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
    let temp = multi_module(
        "use-import",
        &[
            (
                "mathlib/ops.mw",
                "module mathlib::ops\n\npub fn double(n: int): int\n    return n + n\n",
            ),
            (
                "main.mw",
                "module main\n\nuse mathlib::ops\n\npub fn run(): int\n    return ops::double(21)\n",
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
    let temp = multi_module(
        "fully-qualified",
        &[
            (
                "mathlib/ops.mw",
                "module mathlib::ops\n\npub fn triple(n: int): int\n    return n + n + n\n",
            ),
            (
                "main.mw",
                "module main\n\npub fn run(): int\n    return mathlib::ops::triple(4)\n",
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
    let temp = multi_module(
        "same-name",
        &[
            ("a.mw", "module a\n\npub fn helper(): int\n    return 1\n"),
            (
                "b.mw",
                "module b\n\nfn helper(): int\n    return 2\n\npub fn run(): int\n    return helper()\n",
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
    let temp = multi_module(
        "bare-foreign",
        &[
            (
                "other.mw",
                "module other\n\npub fn greet(): int\n    return 1\n",
            ),
            (
                "main.mw",
                "module main\n\npub fn run(): int\n    return greet()\n",
            ),
        ],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.type"));
}

#[test]
fn a_qualified_call_to_an_own_module_private_function_resolves() {
    // Qualifying a call with the caller's own module reaches a private function
    // there; visibility only gates crossing a module boundary.
    let temp = multi_module(
        "own-qualified-private",
        &[(
            "main.mw",
            "module main\n\nfn secret(): int\n    return 7\n\npub fn run(): int\n    return main::secret()\n",
        )],
    );
    let output = run_in(&temp, &["run", "main.run"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\n");
}

#[test]
fn calling_a_private_function_across_modules_is_a_visibility_error() {
    let temp = multi_module(
        "visibility",
        &[
            ("lib.mw", "module lib\n\nfn secret(): int\n    return 1\n"),
            (
                "main.mw",
                "module main\n\npub fn run(): int\n    return lib::secret()\n",
            ),
        ],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.visibility"));
}

#[test]
fn a_use_of_an_unknown_module_is_an_import_error() {
    let temp = multi_module(
        "unknown-import",
        &[(
            "main.mw",
            "module main\n\nuse nope::missing\n\npub fn run(): int\n    return 1\n",
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.import"));
}

#[test]
fn a_headerless_script_is_not_importable_by_module_path() {
    // `lib.mw` has no `module` header, so it is a single-file script, not an
    // importable module; a `use` of it does not resolve.
    let temp = multi_module(
        "script-not-importable",
        &[
            ("lib.mw", "pub fn helper(): int\n    return 1\n"),
            (
                "main.mw",
                "module main\n\nuse lib\n\npub fn run(): int\n    return 1\n",
            ),
        ],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.import"));
}

#[test]
fn a_module_header_that_disagrees_with_its_path_is_rejected() {
    let temp = multi_module(
        "module-path",
        &[(
            "main.mw",
            "module wrong\n\npub fn run(): int\n    return 1\n",
        )],
    );
    assert!(run_diagnostic_code(&temp, "main.run").contains("check.module_path"));
}

#[test]
fn a_duplicate_function_name_in_one_module_conflicts() {
    let temp = multi_module(
        "dup-in-module",
        &[(
            "main.mw",
            "module main\n\nfn helper(): int\n    return 1\n\nfn helper(): int\n    return 2\n\npub fn run(): int\n    return helper()\n",
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
        "pub fn quad(): int\n\
         \x20   return double(double(5))\n\
         \n\
         fn double(n: int): int\n\
         \x20   return n + n\n",
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
        "pub fn ping(): int\n\
         \x20   return pong()\n\
         \n\
         fn pong(): int\n\
         \x20   return ping()\n",
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
        "pub fn loops(): int\n\
         \x20   return loops()\n",
    );
    let output = run_in(&temp, &["run", "loops", "--format", "jsonl"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("check.recursion"),
        "{output:?}"
    );
}

// --- Durable tracer (slices K.6/K.7): the counter CLI travels the full path
// through redb, and a store written by one process is read by the next. ---

const COUNTER_SOURCE: &str = "resource Counter\n\
     \x20   required value: int\n\
     \x20   label: string\n\
     \n\
     store ^counters(name: string): Counter\n\
     \n\
     pub fn set(name: string, v: int)\n\
     \x20   transaction\n\
     \x20       ^counters(name) = Counter(value: v)\n\
     \n\
     pub fn get(name: string): int?\n\
     \x20   return ^counters(name).value\n\
     \n\
     pub fn bump(name: string)\n\
     \x20   transaction\n\
     \x20       const current = ^counters(name).value ?? 0\n\
     \x20       ^counters(name).value = current + 1\n\
     \n\
     pub fn label(name: string, text: string)\n\
     \x20   transaction\n\
     \x20       ^counters(name).label = text\n\
     \n\
     pub fn remove(name: string)\n\
     \x20   transaction\n\
     \x20       delete ^counters(name)\n\
     \n\
     pub fn total(): int\n\
     \x20   var sum = 0\n\
     \x20   for k in ^counters\n\
     \x20       sum = sum + (^counters(k).value ?? 0)\n\
     \x20   return sum\n";

fn run_counter(dir: &Path, store: &Path, export: &str, call: &[&str]) -> Output {
    let mut args = vec!["run", export, "--store"];
    let store = store.to_str().expect("utf-8 store path");
    args.push(store);
    args.push("--");
    args.extend_from_slice(call);
    run_in(dir, &args)
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn durable_set_then_get_round_trips_on_redb() {
    let temp = TempDir::new("counter-set-get");
    project(&temp, COUNTER_SOURCE);
    let store = temp.join("store");

    stdout_of(&run_counter(&temp, &store, "set", &["hits", "5"]));
    assert_eq!(
        stdout_of(&run_counter(&temp, &store, "get", &["hits"])),
        "5\n"
    );
}

#[test]
fn durable_full_algebra_travels_the_path() {
    let temp = TempDir::new("counter-algebra");
    project(&temp, COUNTER_SOURCE);
    let store = temp.join("store");

    // A read of an absent entry is absent.
    assert_eq!(
        stdout_of(&run_counter(&temp, &store, "get", &["hits"])),
        "absent\n"
    );
    // Bump creates the entry (field-by-field creation at commit).
    stdout_of(&run_counter(&temp, &store, "bump", &["hits"]));
    assert_eq!(
        stdout_of(&run_counter(&temp, &store, "get", &["hits"])),
        "1\n"
    );
    // A sparse field write leaves the required value intact.
    stdout_of(&run_counter(&temp, &store, "label", &["hits", "primary"]));
    assert_eq!(
        stdout_of(&run_counter(&temp, &store, "get", &["hits"])),
        "1\n"
    );
    // Erase removes the whole entry.
    stdout_of(&run_counter(&temp, &store, "remove", &["hits"]));
    assert_eq!(
        stdout_of(&run_counter(&temp, &store, "get", &["hits"])),
        "absent\n"
    );
}

/// The exit gate: one process writes and exits; a fresh process reads the same
/// redb file back. Each `run_counter` spawns the built binary anew.
#[test]
fn a_store_survives_a_process_restart() {
    let temp = TempDir::new("counter-restart");
    project(&temp, COUNTER_SOURCE);
    let store = temp.join("store");

    // First process: write and exit.
    stdout_of(&run_counter(&temp, &store, "set", &["visits", "41"]));
    stdout_of(&run_counter(&temp, &store, "bump", &["visits"]));

    // Second process: read it back.
    assert_eq!(
        stdout_of(&run_counter(&temp, &store, "get", &["visits"])),
        "42\n"
    );
}

#[test]
fn a_durable_export_without_a_store_is_a_usage_error() {
    let temp = TempDir::new("counter-nostore");
    project(&temp, COUNTER_SOURCE);
    let output = run_in(&temp, &["run", "get", "--", "hits"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn durable_iteration_totals_entries() {
    let temp = TempDir::new("counter-total");
    project(&temp, COUNTER_SOURCE);
    let store = temp.join("store");
    stdout_of(&run_counter(&temp, &store, "set", &["a", "10"]));
    stdout_of(&run_counter(&temp, &store, "set", &["b", "20"]));
    stdout_of(&run_counter(&temp, &store, "set", &["c", "30"]));
    let output = run_in(&temp, &["run", "total", "--store", store.to_str().unwrap()]);
    assert_eq!(stdout_of(&output), "60\n");
}

/// The checked-in tracer fixture app compiles and runs through the built binary.
#[test]
fn tracer_fixture_app_runs() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/v01/conformance/tracer_counter");
    let store = TempDir::new("fixture-store");
    let store_path = store.join("s");
    let store_arg = store_path.to_str().unwrap();
    let set = run_in(
        &fixture,
        &["run", "set", "--store", store_arg, "--", "hits", "9"],
    );
    assert!(set.status.success(), "{set:?}");
    let bump = run_in(
        &fixture,
        &["run", "bump", "--store", store_arg, "--", "hits"],
    );
    assert!(bump.status.success(), "{bump:?}");
    let total = run_in(&fixture, &["run", "total", "--store", store_arg]);
    assert_eq!(stdout_of(&total), "10\n");
}
