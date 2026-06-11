//! Uninitialized typed vars starting at their zero, and `inout` parameter
//! read/write-back across calls, including the no-write-back-on-throw rule.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;

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
fn an_inout_parameter_reads_then_writes_a_local() {
    // `inout` seeds the parameter from the caller's value, then writes back.
    let program = checked_program(
        "pub fn bump(inout n: int)\n    n = n + 1\npub fn main(): int\n    var n: int = 41\n    bump(inout n)\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn an_inout_parameter_mutates_a_local_resource() {
    // Mutating a field of a local resource passed `inout` is visible to the
    // caller.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\npub fn setTitle(inout book: Book)\n    book.title = \"Small Gods\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    setTitle(inout book)\n    return book.title\n",
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
        "module app\nuse library\npub fn main(): string\n    var book: library::Book\n    book.title = \"draft\"\n    return book.title\n",
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
fn an_inout_parameter_writes_back_to_a_local_resource_field() {
    // A field of a local resource, `book.title`, is an assignable place; passing it
    // `inout` reads it to seed the parameter and writes the result back.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\npub fn upper(inout s: string)\n    s = \"UPPER\"\n\npub fn main(): string\n    var book: Book\n    book.title = \"draft\"\n    upper(inout book.title)\n    return book.title\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Str("UPPER".into())))
    );
}

#[test]
fn write_back_is_skipped_when_the_callee_throws() {
    // A callee that mutates an `inout` parameter then throws must not write back:
    // the caller's local keeps its pre-call value.
    let program = checked_program(
        "pub fn bad(inout n: int)\n    n = 99\n    throw Error(code: \"test.error\", message: \"boom\")\npub fn main(): int\n    var n: int = 1\n    try\n        bad(inout n)\n    catch err: Error\n        write(\"caught\")\n    return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn an_argument_mode_must_match_the_parameter_mode() {
    checker_rejects(
        "pub fn plain(n: int): int\n    return n\npub fn main(): int\n    var n: int = 1\n    return plain(inout n)\n",
        "check.call_argument",
    );
}
