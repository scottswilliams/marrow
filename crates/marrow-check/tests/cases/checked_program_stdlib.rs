use crate::support;
use marrow_check::{DiagnosticPayload, check_project};

use support::{config, temp_project, with_code, write};

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

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A std helper's argument types are checked: passing an `int` where
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

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

/// A std helper's arity is checked: `std::math::modulo` takes two ints, so a
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

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.call_argument"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn std_text_join_requires_a_string_sequence() {
    let root = temp_project("program-std-join-sequence", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn ok(): string\n\
             \x20   return std::text::join(std::text::split(\"a,b\", \",\"), \"|\")\n\
             pub fn bad(): string\n\
             \x20   return std::text::join(\"a,b\", \"|\")\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn std_text_index_of_is_maybe_present() {
    let root = temp_project("program-std-indexof-maybe", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn unresolved(): int\n\
             \x20   return std::text::indexOf(\"abc\", \"b\")\n\
             pub fn coalesced(): int\n\
             \x20   return std::text::indexOf(\"abc\", \"x\") ?? -1\n\
             pub fn guarded(): int\n\
             \x20   if const pos = std::text::indexOf(\"abc\", \"b\")\n\
             \x20       return pos\n\
             \x20   return -1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn write_is_not_a_language_builtin() {
    let root = temp_project("program-write-removed", |root| {
        let removed = "write";
        let source = format!(
            "module shelf::t\n\
             pub fn bad()\n\
             \x20   {removed}(\"x\")\n"
        );
        write(root, "src/shelf/t.mw", &source);
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.unresolved_call"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn removed_clock_shift_helper_is_not_a_standard_library_operation() {
    let root = temp_project("program-clock-shift-removed", |root| {
        let removed = "add";
        let source = format!(
            "module shelf::t\n\
             pub fn bad()\n\
             \x20   std::clock::{removed}(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), 1.hour)\n"
        );
        write(root, "src/shelf/t.mw", &source);
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let unresolved = with_code(&report, "check.unresolved_call");
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        unresolved[0].payload,
        DiagnosticPayload::UnresolvedCall("std::clock::add".into())
    );
}

#[test]
fn removed_imported_clock_shift_helper_is_not_a_standard_library_operation() {
    let root = temp_project("program-short-clock-shift-removed", |root| {
        let removed = "add";
        let source = format!(
            "module shelf::t\n\
             use std::clock\n\
             pub fn bad()\n\
             \x20   clock::{removed}(std::clock::parseInstant(\"2026-05-28T12:00:00Z\"), 1.hour)\n"
        );
        write(root, "src/shelf/t.mw", &source);
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    let unresolved = with_code(&report, "check.unresolved_call");
    assert_eq!(unresolved.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        unresolved[0].payload,
        DiagnosticPayload::UnresolvedCall("std::clock::add".into())
    );
}

/// A duration literal types to `duration`: returned from a `: duration`
/// function and passed where a `duration` argument is expected it checks clean,
/// and returned from a `: int` function it is a return-type error.
#[test]
fn duration_literal_types_to_duration() {
    let root = temp_project("program-duration-literal", |root| {
        write(
            root,
            "src/shelf/t.mw",
            "module shelf::t\n\
             pub fn span(): duration\n\
             \x20   return 1.day\n\
             pub fn shift(): instant\n\
             \x20   return std::clock::parseInstant(\"2026-05-28T12:00:00Z\") + 1.hour\n\
             pub fn wrong(): int\n\
             \x20   return 1.day\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");

    // The duration operand must not raise an untyped-value error: a duration
    // literal is a known type, not dynamic data.
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.untyped_value"),
        "{:#?}",
        report.diagnostics
    );
    let return_type_errors = with_code(&report, "check.return_type");
    assert_eq!(
        return_type_errors.len(),
        1,
        "only the `: int` return should mismatch: {:#?}",
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

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}
