//! End-to-end tests for the built-in `Option[T]` and `Result[T, E]` value types:
//! construction, exhaustive `match`, nested distinctness, `==`/`!=` equality, and
//! prefix `try` propagation travel the real production path (capture -> compile ->
//! encode -> verify -> VM) through the built binary, via the `option_result`
//! conformance fixture and inline invalid-source projects asserting typed
//! diagnostics. Option/Result ride the same ENUMS section and enum opcodes as a
//! user `enum`, so no new image section or opcode is introduced by this vertical.

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
            "marrow-c02-optres-{name}-{}-{nanos}",
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
        .join("fixtures/v01/conformance/option_result")
}

/// The Option/Result conformance fixture passes end to end: construction and
/// exhaustive `match`, nested `Option[Option[int]]` distinctness, exact equality,
/// and prefix `try` success and error propagation all report `passed`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn option_result_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "option/result fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":6"#), "{summary}");
}

/// A returned `Option`/`Result` value renders through the VM via the sealed enum
/// names, for `some`, `none`, and `err`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn option_and_result_values_render_through_the_vm() {
    let temp = TempDir::new("render");
    project(
        &temp,
        "pub fn opt(n: int): Option[int]\n\
         \x20   if n == 0\n\
         \x20       return none\n\
         \x20   return some(n)\n\
         \n\
         pub fn res(n: int): Result[int, string]\n\
         \x20   if n < 0\n\
         \x20       return err(\"neg\")\n\
         \x20   return ok(n)\n",
    );
    let some = run_in(&temp, &["run", "opt", "--format", "jsonl", "--", "7"]);
    let stdout = String::from_utf8_lossy(&some.stdout);
    assert!(some.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""data":{"enum":"Option","member":"some","payload":[7]}"#),
        "{stdout}"
    );
    let none = run_in(&temp, &["run", "opt", "--format", "jsonl", "--", "0"]);
    let stdout = String::from_utf8_lossy(&none.stdout);
    assert!(
        stdout.contains(r#""data":{"enum":"Option","member":"none","payload":[]}"#),
        "{stdout}"
    );
    let err = run_in(&temp, &["run", "res", "--format", "jsonl", "--", "-1"]);
    let stdout = String::from_utf8_lossy(&err.stdout);
    assert!(
        stdout.contains(r#""data":{"enum":"Result","member":"err","payload":["neg"]}"#),
        "{stdout}"
    );
}

/// A `try` whose error type does not match the function's `Result` error type is a
/// typed `check.type` (same `E`, no implicit conversion).
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_try_with_a_mismatched_error_type_is_reported() {
    let temp = TempDir::new("mismatchE");
    project(
        &temp,
        "pub fn g(n: int): Result[int, string]\n\
         \x20   return ok(n)\n\
         \n\
         pub fn f(): Result[int, int]\n\
         \x20   const x = try g(1)\n\
         \x20   return ok(x)\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// A `try` on a non-`Result` value, and a `try` outside a `Result`-returning
/// function, are typed `check.type` diagnostics.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_try_in_the_wrong_context_is_reported() {
    for source in [
        "pub fn f(): Result[int, string]\n    const x = try 5\n    return ok(x)\n",
        "pub fn f(): int\n    const x = try 5\n    return x\n",
    ] {
        let temp = TempDir::new("trycontext");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source}\n{stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{source}\n{stdout}"
        );
    }
}

/// `Option` and `Result` are reserved built-in type names; redeclaring either as a
/// user type is a `check.name_conflict`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn redeclaring_a_builtin_generic_name_is_reported() {
    for decl in ["enum Option\n    a\n", "struct Result\n    x: int\n"] {
        let temp = TempDir::new("reserved");
        project(&temp, &format!("{decl}\npub fn f(): int\n    return 0\n"));
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{decl}\n{stdout}");
        assert!(
            stdout.contains(r#""code":"check.name_conflict""#),
            "{decl}\n{stdout}"
        );
    }
}

/// The removed throw/catch channel is a typed parse diagnostic: a `throw`
/// statement, a block-form `try`/`catch`, and a stray `catch` all report
/// `parse.syntax` and keep the parse total.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn the_removed_throw_catch_channel_is_reported() {
    for source in [
        "pub fn f(): int\n    throw 5\n",
        "pub fn f(): int\n    try\n        return 1\n    catch e\n        return 2\n",
        "pub fn f(): int\n    catch e\n        return 2\n",
    ] {
        let temp = TempDir::new("throwcatch");
        project(&temp, source);
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{source}\n{stdout}");
        assert!(
            stdout.contains(r#""code":"parse.syntax""#),
            "{source}\n{stdout}"
        );
    }
}

/// A bare constructor whose full type argument set cannot be inferred — `none`,
/// `ok(v)`, or `err(e)` with no expected type — is a typed `check.type`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_uninferable_bare_constructor_is_reported() {
    for value in ["none", "ok(5)", "err(\"x\")"] {
        let temp = TempDir::new("infer");
        project(
            &temp,
            &format!("pub fn f(): int\n    const x = {value}\n    return 0\n"),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{value}\n{stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{value}\n{stdout}"
        );
    }
}

/// Every value-level built-in the compiler intercepts before user resolution —
/// the `Option`/`Result` constructors (`none`/`some`/`ok`/`err`), the presence
/// test (`exists`), the divergence marker (`unreachable`), and the pure text
/// floor (`isEmpty`/`contains`/`trim`) — is reserved at every value-binding
/// declaration site. A `fn`, module `const`, parameter, local `const`/`var`, or
/// `if const` binding that reuses one is a `check.name_conflict` at the
/// declaration, not a declaration that is admitted and then silently shadowed at
/// its use site (surfacing later as a confusing `check.type`).
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn redeclaring_a_reserved_builtin_value_name_is_reported() {
    const NAMES: [&str; 11] = [
        "none",
        "some",
        "ok",
        "err",
        "exists",
        "unreachable",
        "isEmpty",
        "date_add_days",
        "date_days_between",
        "contains",
        "trim",
    ];
    for name in NAMES {
        let sources = [
            // module function
            format!("pub fn {name}(): int\n    return 0\n\npub fn f(): int\n    return 0\n"),
            // module constant
            format!("const {name}: int = 1\n\npub fn f(): int\n    return 0\n"),
            // parameter
            format!("pub fn g({name}: int): int\n    return 0\n\npub fn f(): int\n    return 0\n"),
            // local constant
            format!("pub fn f(): int\n    const {name} = 1\n    return 0\n"),
            // local variable
            format!("pub fn f(): int\n    var {name} = 1\n    return 0\n"),
            // if-const binding
            format!(
                "pub fn maybe(): Option[int]\n    return some(1)\n\n\
                 pub fn f(): int\n    if const {name} = maybe()\n        return 0\n    return 0\n"
            ),
        ];
        for source in sources {
            let temp = TempDir::new("reserved-value");
            project(&temp, &source);
            let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(!output.status.success(), "{source}\n{stdout}");
            assert!(
                stdout.contains(r#""code":"check.name_conflict""#),
                "{source}\n{stdout}"
            );
        }
    }
}

/// The reserved-built-in family covers only bare and unqualified-call use sites,
/// so a struct field or an enum variant that spells a built-in name does not
/// collide: both are reached solely through member syntax (`s.none`, `E::ok`),
/// which no built-in ever occupies. Such a program checks and runs.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_struct_field_or_enum_variant_may_spell_a_builtin_name() {
    let temp = TempDir::new("member-name-ok");
    project(
        &temp,
        "struct S\n\
         \x20   none: int\n\
         \x20   trim: int\n\
         \n\
         enum E\n\
         \x20   ok\n\
         \x20   err\n\
         \n\
         pub fn pick(b: bool): E\n\
         \x20   if b\n\
         \x20       return E::ok\n\
         \x20   return E::err\n\
         \n\
         pub fn f(): int\n\
         \x20   const s = S(none: 3, trim: 4)\n\
         \x20   return s.none + s.trim\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":7"#), "{stdout}");
    assert!(!stdout.contains("name_conflict"), "{stdout}");
}
