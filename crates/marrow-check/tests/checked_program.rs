use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{MarrowType, check_project};
use marrow_project::parse_config;
use marrow_store::value::ScalarType;

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

#[test]
fn builds_a_module_for_a_clean_library_file() {
    let root = temp_project("program-clean", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn add(title: string): Book::Id\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert_eq!(program.modules.len(), 1, "{program:#?}");

    let module = &program.modules[0];
    assert_eq!(module.name, "shelf::books");

    assert_eq!(module.resources.len(), 1, "{:#?}", module.resources);
    assert_eq!(module.resources[0].name, "Book");

    let add = module
        .functions
        .iter()
        .find(|function| function.name == "add")
        .expect("add function");
    assert!(add.public, "{add:#?}");
    assert_eq!(add.params.len(), 1, "{:#?}", add.params);
    assert_eq!(add.params[0].name, "title");
    assert_eq!(add.params[0].ty, MarrowType::Primitive(ScalarType::Str));
    assert!(add.return_type.is_some(), "{add:#?}");
    // `add`'s body touches the `^books` saved root (allocating an id with `nextId`).
    assert!(add.touches_saved_data, "{add:#?}");
    // The body is carried into the artifact for the runtime to evaluate.
    assert!(!add.body.statements.is_empty(), "{add:#?}");
}

/// `nextId(^books)` over a single-`int` root types to `Book::Id`, so a function
/// returning it under a declared `Book::Id` return type checks clean. (`nextId`
/// is a saved-data read, so it lives in a function body, not a module const.)
/// Previously `nextId` typed to `Unknown`. The local-const annotation
/// `const id: Book::Id = nextId(^books)` likewise checks clean.
#[test]
fn next_id_types_to_the_resource_identity() {
    let root = temp_project("program-nextid-id", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             pub fn fresh(): Book::Id\n\
             \x20   const id: Book::Id = nextId(^books)\n\
             \x20   return id\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// `nextId` over a composite-identity root is rejected at check time with
/// `check.next_id_requires_single_int`, so the misuse is caught before running.
#[test]
fn next_id_over_a_composite_root_is_flagged() {
    let root = temp_project("program-nextid-composite", |root| {
        write(
            root,
            "src/shelf/enroll.mw",
            "module shelf::enroll\n\
             resource Enrollment at ^enrollments(studentId: string, courseId: string)\n\
             \x20   required grade: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^enrollments)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over a single non-integer (string) root is flagged the same way.
#[test]
fn next_id_over_a_string_keyed_root_is_flagged() {
    let root = temp_project("program-nextid-string", |root| {
        write(
            root,
            "src/shelf/tags.mw",
            "module shelf::tags\n\
             resource Tag at ^tags(slug: string)\n\
             \x20   required name: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^tags)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `nextId` over a keyless singleton root is flagged: a singleton has no
/// generated identity.
#[test]
fn next_id_over_a_singleton_root_is_flagged() {
    let root = temp_project("program-nextid-singleton", |root| {
        write(
            root,
            "src/shelf/settings.mw",
            "module shelf::settings\n\
             resource Settings at ^settings\n\
             \x20   required theme: string\n\
             fn fresh()\n\
             \x20   const id = nextId(^settings)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.next_id_requires_single_int"),
        "{:#?}",
        report.diagnostics
    );
}

/// `use std::clock` lets a short-form `clock::now()` resolve and type to its
/// declared result (`instant`), just as the fully-qualified form does.
#[test]
fn short_form_std_import_resolves() {
    let root = temp_project("program-shortform-clock", |root| {
        write(
            root,
            "src/shelf/times.mw",
            "module shelf::times\n\
             use std::clock\n\
             pub fn stamp(): instant\n\
             \x20   return clock::now()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Without the import, the short-form `clock::now()` does not resolve and reports
/// `check.unresolved_call` — short-form requires the matching `use`.
#[test]
fn short_form_without_import_is_unresolved() {
    let root = temp_project("program-shortform-noimport", |root| {
        write(
            root,
            "src/shelf/times.mw",
            "module shelf::times\n\
             pub fn stamp(): instant\n\
             \x20   return clock::now()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.unresolved_call"),
        "{:#?}",
        report.diagnostics
    );
}

/// Short-form works for project modules too: `use shelf::books` lets `books::add`
/// resolve to the qualified function in that module.
#[test]
fn short_form_project_import_resolves() {
    let root = temp_project("program-shortform-project", |root| {
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\
             pub fn make(): int\n\
             \x20   return 1\n",
        );
        write(
            root,
            "src/shelf/app.mw",
            "module shelf::app\n\
             use shelf::books\n\
             pub fn run(): int\n\
             \x20   return books::make()\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A std helper's argument types are now checked: passing an `int` where
/// `std::text::contains` expects a `string` reports `check.call_argument`.
#[test]
fn std_call_with_wrong_argument_type_is_flagged() {
    let root = temp_project("program-std-argtype", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn bad(): bool\n\
             \x20   return std::text::contains(1, \"x\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// A std helper's arity is now checked: `std::math::modulo` takes two ints, so a
/// one-argument call reports `check.call_argument`.
#[test]
fn std_call_with_wrong_arity_is_flagged() {
    let root = temp_project("program-std-arity", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn bad(): int\n\
             \x20   return std::math::modulo(1)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// A well-typed std call checks clean: `std::clock::add(instant, duration)` with
/// the right argument types reports nothing.
#[test]
fn well_typed_std_call_checks_clean() {
    let root = temp_project("program-std-clean", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn good(): instant\n\
             \x20   return std::clock::add(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), std::clock::parseDuration(\"PT1H\"))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Short-form std calls are arg-checked identically to fully-qualified ones:
/// `clock::add(int, ...)` (wrong first arg) under `use std::clock` is flagged.
#[test]
fn short_form_std_call_is_arg_checked() {
    let root = temp_project("program-std-shortform-arg", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             use std::clock\n\
             pub fn bad(): instant\n\
             \x20   return clock::add(1, clock::parseDuration(\"PT1H\"))\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// Short-form resolves even when the module name is a type keyword: `use std::bytes`
/// lets `bytes::base64Encode(...)` parse (a keyword can lead a `::` path) and check
/// clean, not just the fully-qualified `std::bytes::base64Encode(...)`.
#[test]
fn short_form_keyword_module_resolves() {
    let root = temp_project("program-shortform-bytes", |root| {
        write(
            root,
            "src/shelf/b.mw",
            "module shelf::b\n\
             use std::bytes\n\
             pub fn enc(): string\n\
             \x20   return bytes::base64Encode(b\"hi\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn a_file_with_a_parse_error_contributes_no_module() {
    let root = temp_project("program-parse-error", |root| {
        // A leading tab is a lexical error, so the file parses with errors and
        // is excluded from the artifact.
        write(
            root,
            "src/shelf/books.mw",
            "module shelf::books\n\tconst X = 1\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(report.has_errors(), "{:#?}", report.diagnostics);
    assert!(program.modules.is_empty(), "{program:#?}");
}

// --- `Error` in a scalar position (regression for A08) -------------------------
//
// `MarrowType::Error` is a concrete type with no storage form: it is *not* an
// untyped value. A `catch e: Error` clause binds `e` as `Error`, so using `e`
// where a scalar is required must report the same diagnostic a wrong scalar
// would, never `check.untyped_value` and never nothing. (A08 split `Error` into
// its own arm; before that `Error` was a primitive that simply failed to match,
// which is the behavior these tests pin back in place.) The dual is preserved:
// `Error` must still satisfy an `Error`-typed slot (`std::log::error`).

/// Build a one-module project whose single function wraps `body` in a
/// `try`/`catch e: Error`, so `e` is in scope as an `Error` value, and return its
/// diagnostic codes. `signature` is the function header (e.g. `fn f()`). `slot`
/// names the project directory: each caller passes a distinct `slot` so that two
/// of these tests running concurrently under workspace parallelism never share a
/// temp project (and so cannot delete each other's files mid-run).
fn error_value_diagnostic_codes(slot: &str, signature: &str, body: &str) -> Vec<String> {
    let root = temp_project(&format!("program-error-scalar-{slot}"), |root| {
        write(
            root,
            "src/shelf/t.mw",
            &format!(
                "module shelf::t\n\
                 {signature}\n\
                 \x20   try\n\
                 \x20       var x = 1\n\
                 \x20   catch e: Error\n\
                 {body}\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// `if e` over an `Error` condition reports `check.condition_type` (a condition
/// must be `bool`), not `check.untyped_value`.
#[test]
fn error_condition_is_a_condition_type_error() {
    let codes =
        error_value_diagnostic_codes("condition", "fn f()", "        if e\n            x = 1");
    assert!(
        codes.iter().any(|code| code == "check.condition_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `return e` from a `: string` function reports `check.return_type`, not
/// `check.untyped_value`.
#[test]
fn error_return_is_a_return_type_error() {
    let codes = error_value_diagnostic_codes("return", "fn f(): string", "        return e");
    assert!(
        codes.iter().any(|code| code == "check.return_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `s = e` storing an `Error` into a `string` place reports
/// `check.assignment_type`, not `check.untyped_value`.
#[test]
fn error_assignment_is_an_assignment_type_error() {
    let codes = error_value_diagnostic_codes(
        "assignment",
        "fn f()",
        "        var s: string = \"a\"\n        s = e",
    );
    assert!(
        codes.iter().any(|code| code == "check.assignment_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// Passing `e` to a user function declared `f(s: string)` reports
/// `check.call_argument`, not `check.untyped_value`.
#[test]
fn error_argument_to_user_function_is_a_call_argument_error() {
    let root = temp_project("program-error-userfn-arg", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             fn takes(s: string)\n\
             \x20   return\n\
             fn f()\n\
             \x20   try\n\
             \x20       var x = 1\n\
             \x20   catch e: Error\n\
             \x20       takes(e)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
}

/// Build a project declaring `fn takes(e: Error)` and calling it with `arg` from
/// inside a `try`/`catch e: Error` (so the name `e` is an `Error` value in scope),
/// and return the diagnostic codes. An `Error`-typed parameter is a reachable user
/// type (`names_known_type` accepts "Error"; `from_resolved` maps it to
/// `MarrowType::Error`), so the argument loop must check it like a scalar.
fn error_param_call_diagnostic_codes(slot: &str, arg: &str) -> Vec<String> {
    let root = temp_project(&format!("program-error-param-{slot}"), |root| {
        write(
            root,
            "src/shelf/t.mw",
            &format!(
                "module shelf::t\n\
                 fn takes(e: Error)\n\
                 \x20   return\n\
                 fn f()\n\
                 \x20   try\n\
                 \x20       var x = 1\n\
                 \x20   catch e: Error\n\
                 \x20       takes({arg})\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// Passing a `string` literal to a `takes(e: Error)` parameter reports
/// `check.call_argument`: the scalar does not satisfy the concrete `Error` slot.
/// (Before the fix the `as_primitive(&param.ty).is_some()` gate skipped any
/// `Error`-typed parameter, silently accepting the mismatch.)
#[test]
fn scalar_argument_to_error_param_is_a_call_argument_error() {
    let codes = error_param_call_diagnostic_codes("scalar", "\"oops\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// Passing an unbound name (an `Unknown` value) to a `takes(e: Error)` parameter
/// reports `check.untyped_value`: strict typing still requires a known type for a
/// concrete slot, even an `Error` one.
#[test]
fn untyped_argument_to_error_param_is_an_untyped_value_error() {
    let codes = error_param_call_diagnostic_codes("untyped", "mystery");
    assert!(
        codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// Passing a catch-bound `Error` value to a `takes(e: Error)` parameter checks
/// clean: the concrete `Error` slot is satisfied by an `Error` argument.
#[test]
fn error_argument_to_error_param_checks_clean() {
    let codes = error_param_call_diagnostic_codes("clean", "e");
    assert!(codes.is_empty(), "{codes:#?}");
}

/// Passing `e` to `std::log::info` (which expects a `string`) reports
/// `check.call_argument`, not `check.untyped_value`.
#[test]
fn error_argument_to_std_log_info_is_a_call_argument_error() {
    let codes = error_value_diagnostic_codes("log-info", "fn f()", "        std::log::info(e)");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `-e` negating an `Error` reports `check.operator_type` (no operator applies to
/// an `Error`), not `check.untyped_value`.
#[test]
fn error_unary_negation_is_an_operator_type_error() {
    let codes = error_value_diagnostic_codes("unary", "fn f()", "        y = -e");
    assert!(
        codes.iter().any(|code| code == "check.operator_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `e + 1` with an `Error` operand reports `check.operator_type` (no operator
/// applies to an `Error`), not `check.untyped_value` and never nothing.
#[test]
fn error_arithmetic_operand_is_an_operator_type_error() {
    let codes = error_value_diagnostic_codes("arithmetic", "fn f()", "        y = e + 1");
    assert!(
        codes.iter().any(|code| code == "check.operator_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

/// `e < 1` comparing an `Error` operand reports `check.operator_type`, not
/// `check.untyped_value` and never nothing.
#[test]
fn error_comparison_operand_is_an_operator_type_error() {
    let codes = error_value_diagnostic_codes("comparison", "fn f()", "        y = e < 1");
    assert!(
        codes.iter().any(|code| code == "check.operator_type"),
        "{codes:#?}"
    );
    assert!(
        !codes.iter().any(|code| code == "check.untyped_value"),
        "{codes:#?}"
    );
}

// --- `Error` in the one slot that *expects* it (dual of the above) -------------

/// `std::log::error(e)` accepts an `Error` value: the `Error`-typed slot is
/// satisfied, so the call checks clean.
#[test]
fn error_argument_to_std_log_error_checks_clean() {
    let codes = error_value_diagnostic_codes("log-error", "fn f()", "        std::log::error(e)");
    assert!(codes.is_empty(), "{codes:#?}");
}

/// A scalar passed to `std::log::error` (which expects an `Error`) reports
/// `check.call_argument` — the scalar does not satisfy the `Error` slot.
#[test]
fn scalar_argument_to_std_log_error_is_a_call_argument_error() {
    let root = temp_project("program-logerror-scalar", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             fn f()\n\
             \x20   std::log::error(\"oops\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// An untyped value passed to `std::log::error` reports `check.untyped_value`:
/// `Unknown` is still untyped (unchanged by the `Error` fix). An unbound name
/// (`mystery`) has no known type.
#[test]
fn untyped_argument_to_std_log_error_is_an_untyped_value_error() {
    let root = temp_project("program-logerror-untyped", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             fn f()\n\
             \x20   std::log::error(mystery)\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
}
