//! Uninitialized typed vars start at their zero, and a function return value
//! updates a local, resource, or resource field through assignment;
//! parameter-mode syntax is rejected before runtime.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_syntax::{DiagnosticReason, ParseDiagnosticReason, UnsupportedSyntax, parse_source};

#[test]
fn an_uninitialized_scalar_var_starts_at_its_zero() {
    // A typed `var` without an initializer is a writable place that starts at its
    // type's default, so plain declaration-then-use works.
    let program = checked_program("pub fn main(): int\n    var n: int\n    return n\n");
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(0)))
    );
}

#[test]
fn a_return_value_updates_a_local() {
    let program = checked_program(
        "pub fn bump(n: int): int\n    return n + 1\npub fn main(): int\n    var n: int = 41\n    n = bump(n)\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn a_returned_resource_updates_a_local_resource() {
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\npub fn withTitle(book: Book): Book\n    var updated: Book = book\n    updated.title = \"Small Gods\"\n    return updated\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    book = withTitle(book)\n    return book.title ?? \"\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Str("Small Gods".into())))
    );
}

#[test]
fn an_uninitialized_qualified_resource_var_starts_empty() {
    let program = checked_program_modules(&[
        "module library\nresource Book\n    title: string\n",
        "module app\nuse library\npub fn main(): string\n    var book: library::Book\n    book.title = \"draft\"\n    return book.title ?? \"\"\n",
    ]);
    assert_eq!(
        run(checked_entry!(&program, "app::main")),
        Ok(Some(Value::Str("draft".into())))
    );
}

#[test]
fn uninitialized_bare_foreign_resource_var_is_not_project_wide() {
    checker_rejects_sources(
        &[
            "module library\nresource Book\n    title: string\n",
            "module app\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    return book.title\n",
        ],
        "check.unknown_type",
    );
}

#[test]
fn a_return_value_updates_a_local_resource_field() {
    let program = checked_program(
        "resource Book\n    title: string\nstore ^books(id: int): Book\n\npub fn upper(s: string): string\n    return \"UPPER\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    book.title = upper(book.title ?? \"\")\n    return book.title ?? \"\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Str("UPPER".into())))
    );
}

#[test]
fn maybe_returning_function_returns_present_value() {
    let program = checked_program(
        "pub fn maybe_value(flag: bool): int?\n\
         \x20   if flag\n\
         \x20       return 42\n\
         \x20   return absent\n\n\
         pub fn main(flag: bool): int\n\
         \x20   return maybe_value(flag) ?? -1\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::main", Value::Bool(true))),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn return_absent_resolves_through_coalesce_and_if_const() {
    let program = checked_program(
        "pub fn missing(): int?\n\
         \x20   return absent\n\n\
         pub fn coalesced(): int\n\
         \x20   return missing() ?? -1\n\n\
         pub fn guarded(): int\n\
         \x20   if const n = missing()\n\
         \x20       return n\n\
         \x20   return -2\n\n\
         pub fn missing_exists(): bool\n\
         \x20   return exists(missing())\n\n\
         pub fn present_exists(): bool\n\
         \x20   return exists(present())\n\n\
         pub fn present(): int?\n\
         \x20   return 7\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::coalesced")),
        Ok(Some(Value::Int(-1)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::guarded")),
        Ok(Some(Value::Int(-2)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::missing_exists")),
        Ok(Some(Value::Bool(false)))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::present_exists")),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn maybe_entry_return_absent_has_no_run_value() {
    let program = checked_program("pub fn main(): int?\n    return absent\n");

    assert_eq!(run(checked_entry!(&program, "test::main")), Ok(None));
}

#[test]
fn maybe_function_propagates_absent_saved_read_without_option_value() {
    let program = checked_program(
        "resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\n\
         pub fn subtitle(id: int): string?\n\
         \x20   return ^books(id).subtitle\n\n\
         pub fn subtitle_or_missing(id: int): string\n\
         \x20   return subtitle(id) ?? \"missing\"\n",
    );

    assert_eq!(
        run(checked_entry!(
            &program,
            "test::subtitle_or_missing",
            Value::Int(1)
        )),
        Ok(Some(Value::Str("missing".into())))
    );
}

#[test]
fn a_throw_before_assignment_leaves_the_local_unchanged() {
    let program = checked_program(
        "pub fn bad(n: int): int\n    throw Error(code: \"test.error\", message: \"boom\")\npub fn main(): int\n    var n: int = 1\n    try\n        n = bad(n)\n    catch err: Error\n        print(\"caught\")\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn inout_parameter_and_argument_syntax_is_rejected_before_runtime() {
    for source in [
        "pub fn bump(inout n: int)\n    return\n",
        "pub fn plain(n: int): int\n    return n\npub fn main(): int\n    var n: int = 1\n    return plain(inout n)\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            parsed.has_errors(),
            "expected parse rejection for:\n{source}"
        );
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == DiagnosticReason::Parser(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::ParameterModes,
                ))),
            "{:#?}",
            parsed.diagnostics
        );
    }
}
