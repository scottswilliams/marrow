mod support;

use marrow_check::check_project;

use support::{config, temp_project, write};

// --- `Error` in a scalar position ---------------------------------------------
//
// `MarrowType::Error` is a concrete type with no storage form: it is *not* an
// untyped value. A `catch e: Error` clause binds `e` as `Error`, so using `e`
// where a scalar is required must report the same diagnostic a wrong scalar
// would, never `check.untyped_value` and never nothing. The dual is preserved:
// `Error` must still satisfy an `Error`-typed slot (`std::log::error`).

/// Check a one-module project whose `src/shelf/t.mw` holds `module_src` and return
/// its diagnostic codes. `slot` names the project directory: each caller passes a
/// distinct `slot` so that two tests running concurrently under workspace parallelism
/// never share a temp project (and so cannot delete each other's files mid-run).
fn module_diagnostic_codes(slot: &str, module_src: &str) -> Vec<String> {
    let root = temp_project(slot, |root| write(root, "src/shelf/t.mw", module_src));
    let (report, _) = check_project(&root, &config()).expect("check");
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// Build a one-module project whose single function wraps `body` in a
/// `try`/`catch e: Error`, so `e` is in scope as an `Error` value, and return its
/// diagnostic codes. `signature` is the function header (e.g. `fn f()`).
fn error_value_diagnostic_codes(slot: &str, signature: &str, body: &str) -> Vec<String> {
    module_diagnostic_codes(
        &format!("program-error-scalar-{slot}"),
        &format!(
            "module shelf::t\n\
             {signature}\n\
             \x20   try\n\
             \x20       var x = 1\n\
             \x20   catch e: Error\n\
             {body}\n"
        ),
    )
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
/// and return the diagnostic codes. An `Error`-typed parameter is a concrete user
/// type, so the argument loop checks it like any other typed slot.
fn error_param_call_diagnostic_codes(slot: &str, arg: &str) -> Vec<String> {
    module_diagnostic_codes(
        &format!("program-error-param-{slot}"),
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
    )
}

/// Passing a `string` literal to a `takes(e: Error)` parameter reports
/// `check.call_argument`: the scalar does not satisfy the concrete `Error` slot.
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

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
}
