use crate::support;
use marrow_check::check_project;

use support::{config, temp_project, write};

// --- `Error(...)` constructor field typing ------------------------------------
//
// `Error(...)` is checked as a resource constructor against the one `Error`
// field contract (`code` required with ErrorCode grammar, `message` required,
// `help` optional, `data` optional). A wrong field type, an unknown field, a
// missing required field, or a duplicate field is a compile error at the call
// site, not a runtime fault. The field set and code grammar are owned by
// `marrow_schema::error`; the checker must not invent its own.

/// Build a one-module project whose single function constructs `Error(args)` and
/// return its diagnostic codes. `slot` keeps concurrent temp projects disjoint.
fn error_construct_codes(slot: &str, args: &str) -> Vec<String> {
    let root = temp_project(&format!("program-error-construct-{slot}"), |root| {
        write(
            root,
            "src/shelf/t.mw",
            &format!(
                "module shelf::t\n\
                 fn f()\n\
                 \x20   const err = Error({args})\n\
                 \x20   return\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// A valid `Error(code, message)` with both required string fields checks clean.
#[test]
fn required_fields_check_clean() {
    let codes = error_construct_codes("ok-required", "code: \"book.absent\", message: \"gone\"");
    assert!(codes.is_empty(), "{codes:#?}");
}

/// A valid `Error(code, message, help)` adding the optional string help checks
/// clean.
#[test]
fn optional_help_checks_clean() {
    let codes = error_construct_codes(
        "ok-help",
        "code: \"book.absent\", message: \"gone\", help: \"try another id\"",
    );
    assert!(codes.is_empty(), "{codes:#?}");
}

/// A valid `Error(code, message, help, data)` supplying the open `data` payload
/// (here a string, but `data` accepts any type) checks clean.
#[test]
fn optional_data_checks_clean() {
    let codes = error_construct_codes(
        "ok-data",
        "code: \"book.absent\", message: \"gone\", help: \"h\", data: \"payload\"",
    );
    assert!(codes.is_empty(), "{codes:#?}");
}

/// An `int` where `code` must be a string reports `check.call_argument`.
#[test]
fn non_string_code_is_a_call_argument_error() {
    let codes = error_construct_codes("bad-code", "code: 123, message: \"gone\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// An `int` where `message` must be a string reports `check.call_argument`.
#[test]
fn non_string_message_is_a_call_argument_error() {
    let codes = error_construct_codes("bad-message", "code: \"book.absent\", message: 123");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// A `bool` where the optional `help` must be a string reports
/// `check.call_argument`. The optional fields are typed too.
#[test]
fn non_string_help_is_a_call_argument_error() {
    let codes = error_construct_codes(
        "bad-help",
        "code: \"book.absent\", message: \"gone\", help: true",
    );
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// A missing required field reports `check.call_argument`: `Error` without
/// `message` is incomplete.
#[test]
fn missing_required_field_is_a_call_argument_error() {
    let codes = error_construct_codes("missing-message", "code: \"book.absent\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// An unknown field name reports `check.call_argument`: `Error` has no `note`.
#[test]
fn unknown_field_is_a_call_argument_error() {
    let codes = error_construct_codes(
        "unknown-field",
        "code: \"book.absent\", message: \"gone\", note: \"x\"",
    );
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// A field supplied twice reports `check.call_argument`.
#[test]
fn duplicate_field_is_a_call_argument_error() {
    let codes = error_construct_codes("duplicate", "code: \"a\", code: \"b\", message: \"gone\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

/// A positional argument reports `check.call_argument`: `Error(...)` takes named
/// fields, matching the runtime contract.
#[test]
fn positional_argument_is_a_call_argument_error() {
    let codes = error_construct_codes("positional", "\"book.absent\", \"gone\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

#[test]
fn an_invalid_literal_code_is_a_call_argument_error() {
    let codes = error_construct_codes(
        "invalid-code",
        "code: \"Not A Valid Code!!!\", message: \"boom\"",
    );
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

#[test]
fn a_dotless_literal_code_is_a_call_argument_error() {
    let codes = error_construct_codes("dotless-code", "code: \"boom\", message: \"boom\"");
    assert!(
        codes.iter().any(|code| code == "check.call_argument"),
        "{codes:#?}"
    );
}

#[test]
fn a_valid_literal_code_checks_clean() {
    let codes = error_construct_codes("valid-code", "code: \"app.bad_input\", message: \"boom\"");
    assert!(codes.is_empty(), "{codes:#?}");
}
