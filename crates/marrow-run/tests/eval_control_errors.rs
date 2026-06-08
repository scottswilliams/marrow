//! Conditionals and asserts, group-entry writes, std text/math builtins, and the
//! throw / catch / try / finally error model including inout parameters and
//! host-effect transaction guards.

#[macro_use]
mod support;

use support::*;

use marrow_run::{
    Host, RUN_ASSERT, RUN_CAPABILITY, RUN_DECIMAL_OVERFLOW, RUN_DIVIDE_BY_ZERO, RUN_OVERFLOW,
    RUN_TYPE, RUN_UNCAUGHT_THROW, Value,
};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn evaluates_conditionals() {
    let source = "pub fn max(a: int, b: int): int\n    if a > b\n        return a\n    return b\n";
    assert_eq!(
        eval_source(source, "max", vec![Value::Int(7), Value::Int(3)]),
        Ok(Some(Value::Int(7)))
    );
    assert_eq!(
        eval_source(source, "max", vec![Value::Int(3), Value::Int(7)]),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn std_assert_is_true_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isTrue(1 == 1)\n");
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isTrue(1 == 2)\n");
    assert_run_error(run(checked_entry!(&program, "test::bad")), RUN_ASSERT);
}

#[test]
fn std_assert_is_false_passes_and_fails() {
    let program = checked_program("pub fn ok()\n    std::assert::isFalse(1 == 2)\n");
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));

    let program = checked_program("pub fn bad()\n    std::assert::isFalse(1 == 1)\n");
    assert_run_error(run(checked_entry!(&program, "test::bad")), RUN_ASSERT);
}

#[test]
fn std_assert_fail_raises_with_its_message() {
    let program = checked_program("pub fn bad()\n    std::assert::fail(\"boom\")\n");
    let error = run_expecting_error(checked_entry!(&program, "test::bad"));
    assert_eq!(error.code, RUN_ASSERT);
    let (code, message) = error_throw_fields(&error);
    assert_eq!(code, RUN_ASSERT);
    assert_eq!(message, "boom");
}

#[test]
fn std_assert_absent_passes_when_nothing_is_saved() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\
         \n\
         pub fn ok()\n    std::assert::absent(^books(1))\n",
    );
    assert_eq!(run(checked_entry!(&program, "test::ok")), Ok(None));
}

#[test]
fn std_assert_absent_fails_when_a_value_is_present() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    title: string\n\
         \n\
         pub fn bad()\n    std::assert::absent(^books(1))\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("present".into()),
    );
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::bad")),
        RUN_ASSERT,
    );
}

#[test]
fn std_assert_rejects_misused_arguments() {
    checker_rejects(
        "pub fn bad()\n    std::assert::isTrue(1)\n",
        "check.call_argument",
    );
    checker_rejects(
        "pub fn bad()\n    std::assert::fail(42)\n",
        "check.call_argument",
    );
}

#[test]
fn a_passing_assert_lets_execution_continue() {
    // A passing assertion produces no value and falls through to later statements.
    let program =
        checked_program("pub fn ok(): int\n    std::assert::isTrue(1 == 1)\n    return 7\n");
    assert_eq!(
        run(checked_entry!(&program, "test::ok")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_whole_group_entry_write_creates_the_entry() {
    // `^books(1).versions(2) = b` writes the whole group entry from a resource
    // value; the runtime matches its fields against the group's members by name.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v2\"\n\
         \x20\x20\x20\x20^books(1).versions(2) = b\n\
         \n\
         pub fn version_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::version_title"))
            .unwrap()
            .value,
        Some(Value::Str("v2".into()))
    );
}

#[test]
fn a_nested_group_field_round_trips() {
    // `versions(version)` entries hold a nested `comments(pos)` group; writing and
    // reading `^books(1).versions(2).comments(3).text` exercises a saved-tree path
    // deeper than one keyed layer.
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20comments(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20required text: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20^books(1).title = \"root\"\n\
         \x20\x20\x20\x20^books(1).versions(2).title = \"version\"\n\
         \x20\x20\x20\x20^books(1).versions(2).comments(3).text = \"deep\"\n\
         \n\
         pub fn comment(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).comments(3).text ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::comment"))
            .unwrap()
            .value,
        Some(Value::Str("deep".into()))
    );
}

#[test]
fn a_whole_group_entry_can_be_read_and_copied() {
    // `^books(1).versions(2) = ^books(1).versions(1)` reads the whole entry as a
    // value (RHS) and writes it to another key (LHS).
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20\x20\x20\x20\x20note: string\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20var b: Book\n\
         \x20\x20\x20\x20b.title = \"v1\"\n\
         \x20\x20\x20\x20^books(1).versions(1) = b\n\
         \x20\x20\x20\x20if exists(^books(1).versions(1))\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).versions(2) = ^books(1).versions(1)\n\
         \n\
         pub fn copied_title(): string\n\
         \x20\x20\x20\x20return ^books(1).versions(2).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::copied_title"))
            .unwrap()
            .value,
        Some(Value::Str("v1".into()))
    );
}

#[test]
fn std_text_builtins_operate_on_strings() {
    // `length` counts Unicode scalar values, not bytes ("café" is 4 scalars).
    let program = checked_program("pub fn f(): int\n    return std::text::length(\"café\")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(4)))
    );

    let program = checked_program("pub fn f(): string\n    return std::text::trim(\"  hi  \")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Str("hi".into())))
    );

    let program =
        checked_program("pub fn f(): bool\n    return std::text::contains(\"hello\", \"ell\")\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Bool(true)))
    );
}

#[test]
fn std_math_builtins_compute_over_integers() {
    let program = checked_program("pub fn f(): int\n    return std::math::absInt(0 - 7)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(7)))
    );

    // remainder is truncated (sign of the dividend): -7 rem 3 = -1.
    let program = checked_program("pub fn f(): int\n    return std::math::remainder(0 - 7, 3)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(-1)))
    );

    // modulo is floored (sign of the divisor): -7 mod 3 = 2.
    let program = checked_program("pub fn f(): int\n    return std::math::modulo(0 - 7, 3)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn std_math_modulo_by_zero_is_a_runtime_error() {
    let program = checked_program("pub fn f(): int\n    return std::math::modulo(7, 0)\n");
    assert_run_error(run(checked_entry!(&program, "test::f")), RUN_DIVIDE_BY_ZERO);
}

#[test]
fn std_builtins_reject_wrong_argument_types() {
    checker_rejects(
        "pub fn f(): int\n    return std::text::length(42)\n",
        "check.call_argument",
    );
    checker_rejects(
        "pub fn f(): int\n    return std::math::absInt(\"x\")\n",
        "check.call_argument",
    );
}

#[test]
fn throw_surfaces_as_an_uncaught_error() {
    let program = checked_program(
        "pub fn bad()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
    );
    let error = run_expecting_error(checked_entry!(&program, "test::bad"));
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    // The rendered message is byte-identical to the `uncaught error [code]: msg`
    // formula, pinning the format the CLI surfaces for an uncaught throw.
    assert_eq!(error.message, "uncaught error [book.absent]: no book");
}

#[test]
fn error_constructor_requires_code_and_message() {
    // The required subset {code, message} of the `Error` shape is owned by the
    // schema descriptor; the checker rejects a constructor that omits a required
    // field, so the program never reaches the runtime.
    checker_rejects(
        "pub fn bad()\n    throw Error(code: \"x.y\")\n",
        "check.call_argument",
    );
}

#[test]
fn error_constructor_rejects_an_unknown_field() {
    // A field outside the descriptor's set is a constructor error the checker
    // catches before run.
    checker_rejects(
        "pub fn bad()\n    throw Error(code: \"x\", message: \"m\", oops: \"!\")\n",
        "check.call_argument",
    );
}

#[test]
fn error_constructor_rejects_non_string_builtin_fields() {
    // Each builtin `Error` field carries a declared type; a value of the wrong
    // type is a constructor error the checker rejects.
    for source in [
        "pub fn bad()\n    throw Error(code: true, message: \"m\")\n",
        "pub fn bad()\n    throw Error(code: \"x\", message: true)\n",
        "pub fn bad()\n    throw Error(code: \"x\", message: \"m\", help: true)\n",
    ] {
        checker_rejects(source, "check.call_argument");
    }
}

#[test]
fn error_constructor_accepts_open_data_payload() {
    let program = checked_program(
        "pub fn safe(): string\n    try\n        throw Error(code: \"x.y\", message: \"m\", data: true)\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("x.y".into())))
    );
}

#[test]
fn error_fields_keep_their_declared_types() {
    // `help` is a string scalar and `data` is the open `unknown` payload; reading
    // each off a caught error must type and run as the descriptor declares.
    let program = checked_program(
        "pub fn pick(): string\n    try\n        throw Error(code: \"x.y\", message: \"m\", help: \"try again\", data: \"raw\")\n    catch err: Error\n        return err.help\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::pick")),
        Ok(Some(Value::Str("try again".into())))
    );
}

#[test]
fn throw_is_an_error_value() {
    checker_rejects("pub fn bad()\n    throw 7\n", "check.throw_type");
}

#[test]
fn catch_binds_the_thrown_error_and_recovers() {
    let program = checked_program(
        "pub fn safe(): string\n    try\n        throw Error(code: \"x.y\", message: \"boom\")\n    catch err: Error\n        return err.message\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("boom".into())))
    );
}

#[test]
fn a_try_that_succeeds_skips_catch() {
    let program = checked_program(
        "pub fn ok(): int\n    try\n        return 1\n    catch err: Error\n        return 2\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::ok")),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn finally_runs_on_success_and_on_throw() {
    let program = checked_program(
        "pub fn run_it(do_throw: bool)\n    try\n        if do_throw\n            throw Error(code: \"x.y\", message: \"b\")\n    catch err: Error\n        write(\"caught \")\n    finally\n        write(\"cleanup\")\n",
    );
    let out = |b| {
        run_full(checked_entry!(&program, "test::run_it", Value::Bool(b)))
            .unwrap()
            .output
    };
    assert_eq!(out(false), "cleanup");
    assert_eq!(out(true), "caught cleanup");
}

#[test]
fn a_runtime_fault_in_try_is_caught() {
    let program = checked_program(
        "pub fn f(): int\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 2\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn numeric_parse_and_range_faults_are_catchable_with_specific_codes() {
    let program = checked_program(
        "pub fn overflow_code(): string\n\
         \x20   try\n\
         \x20       var x: int = 9223372036854775807\n\
         \x20       x = x + 1\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn remainder_code(): string\n\
         \x20   try\n\
         \x20       var z: int = 0\n\
         \x20       var y: int = 5 % z\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn parse_date_code(): string\n\
         \x20   try\n\
         \x20       var d: date = std::clock::parseDate(\"2023-02-29\")\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn parse_duration_code(): string\n\
         \x20   try\n\
         \x20       var d: duration = std::clock::parseDuration(\"nonsense\")\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn duration_conversion_code(): string\n\
         \x20   try\n\
         \x20       var raw: string = \"nonsense\"\n\
         \x20       var d: duration = duration(raw)\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn instant_range_code(): string\n\
         \x20   try\n\
         \x20       var text: string = std::clock::formatInstant(std::clock::add(std::clock::parseInstant(\"9999-12-31T23:59:59Z\"), 1.day))\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n\
         \n\
         pub fn decimal_overflow_code(): string\n\
         \x20   try\n\
         \x20       var x: decimal = 9999999999999999999999999999999999.0 * 9999999999999999999999999999999999.0\n\
         \x20   catch err: Error\n\
         \x20       return err.code\n\
         \x20   return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::overflow_code")),
        Ok(Some(Value::Str(RUN_OVERFLOW.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::remainder_code")),
        Ok(Some(Value::Str(RUN_DIVIDE_BY_ZERO.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::parse_date_code")),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::parse_duration_code")),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::duration_conversion_code")),
        Ok(Some(Value::Str(RUN_TYPE.into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::instant_range_code")),
        Ok(Some(Value::Str("value.range".into())))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::decimal_overflow_code")),
        Ok(Some(Value::Str(RUN_DECIMAL_OVERFLOW.into())))
    );
}

#[test]
fn a_throw_from_a_callee_is_caught_by_the_caller() {
    // An Error thrown inside a called function unwinds through the call and is
    // caught by the caller.
    let program = checked_program(
        "pub fn boom()\n    throw Error(code: \"x.y\", message: \"deep\")\npub fn safe(): string\n    try\n        boom()\n    catch err: Error\n        return err.message\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("deep".into())))
    );
}

#[test]
fn an_expression_position_call_throw_is_caught_like_a_statement_throw() {
    // The throw rides one channel regardless of position: a throw from a call used
    // mid-expression (`var x = boom() + 1`) unwinds on the same `Err` channel a
    // bare `throw` statement does, so the same `catch` binds it.
    let program = checked_program(
        "pub fn boom(): int\n    throw Error(code: \"x.y\", message: \"mid\")\npub fn safe(): string\n    try\n        var total: int = boom() + 1\n    catch err: Error\n        return err.message\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::safe")),
        Ok(Some(Value::Str("mid".into())))
    );
}

#[test]
fn a_throw_propagates_through_intermediate_calls() {
    // a -> b -> c; c throws, a catches. The Error crosses two call boundaries.
    let program = checked_program(
        "pub fn c()\n    throw Error(code: \"deep.fail\", message: \"from c\")\npub fn b()\n    c()\npub fn a(): string\n    try\n        b()\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::a")),
        Ok(Some(Value::Str("deep.fail".into())))
    );
}

#[test]
fn a_callee_throw_rolls_back_the_enclosing_transaction() {
    // A transaction writes, then a called function throws. The throw escapes the
    // transaction, so it rolls back and the write never lands.
    let program = checked_program(
        "resource Account at ^accts(id: int)\n    balance: int\n\npub fn fail()\n    throw Error(code: \"x\", message: \"boom\")\n\npub fn run_it()\n    transaction\n        ^accts(1).balance = 5\n        fail()\n\npub fn read(): int\n    return ^accts(1).balance ?? -1\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::run_it"));
    assert_run_error(result, RUN_UNCAUGHT_THROW);
    let after = run_entry(&store, checked_entry!(&program, "test::read"))
        .expect("read")
        .value;
    assert_eq!(after, Some(Value::Int(-1)));
}

#[test]
fn a_caught_callee_throw_does_not_leak_into_a_later_fault() {
    // After a caller catches a callee's throw, the pending throw is cleared, so a
    // later fault is caught with its own Error value rather than the stale throw.
    let program = checked_program(
        "pub fn callee()\n    throw Error(code: \"e1\", message: \"boom\")\npub fn check(): int\n    try\n        callee()\n    catch err: Error\n        write(\"caught\")\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::check")),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn a_throwing_finally_does_not_leak_a_pending_throw() {
    // A `finally` throwing over a call-propagated throw must not leave that throw
    // stashed: after an outer `catch` swallows the finally throw, a later fault is
    // caught with its own Error value rather than the stale throw.
    let program = checked_program(
        "pub fn callee()\n    throw Error(code: \"e1\", message: \"from call\")\npub fn leak(): int\n    try\n        try\n            callee()\n        finally\n            throw Error(code: \"e2\", message: \"from finally\")\n    catch err: Error\n        write(\"swallowed\")\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::leak")),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn a_throw_from_a_call_in_finally_propagates() {
    // A `finally` whose own called function throws: that throw replaces the
    // outcome and is caught by an outer handler.
    let program = checked_program(
        "pub fn boom()\n    throw Error(code: \"deep\", message: \"x\")\npub fn run_it(): string\n    try\n        try\n            write(\"body\")\n        finally\n            boom()\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::run_it")),
        Ok(Some(Value::Str("deep".into())))
    );
}

#[test]
fn a_clean_finally_preserves_a_propagated_call_throw() {
    // A clean `finally` (no throw of its own) over a call-propagated throw must
    // restore the pending throw so an outer `catch` still sees it.
    let program = checked_program(
        "pub fn boom()\n    throw Error(code: \"deep\", message: \"x\")\npub fn run_it(): string\n    try\n        try\n            boom()\n        finally\n            write(\"cleanup\")\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    let outcome = run_full(checked_entry!(&program, "test::run_it")).expect("caught");
    assert_eq!(outcome.value, Some(Value::Str("deep".into())));
    assert_eq!(outcome.output, "cleanup");
}

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
        "pub fn bad(inout n: int)\n    n = 99\n    throw Error(code: \"x\", message: \"boom\")\npub fn main(): int\n    var n: int = 1\n    try\n        bad(inout n)\n    catch err: Error\n        write(\"caught\")\n    return n\n",
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

/// A program exercising the four `std::io` file builtins.
const IO_SAMPLE: &str = "\
pub fn saveText(path: string, text: string)
    std::io::writeText(path, text)

pub fn loadText(path: string): string
    return std::io::readText(path)

pub fn saveBytes(path: string, data: bytes)
    std::io::writeBytes(path, data)

pub fn loadBytes(path: string): bytes
    return std::io::readBytes(path)

pub fn loadOrCode(path: string): string
    try
        return std::io::readText(path)
    catch err: Error
        return err.code
";

#[test]
fn io_round_trips_text_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("note.txt").to_string_lossy().into_owned();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::saveText",
            Value::Str(path.clone()),
            Value::Str("hello".into())
        ),
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadText", Value::Str(path)),
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(Value::Str("hello".into())));
}

#[test]
fn irreversible_host_effects_inside_a_transaction_are_rejected_before_the_effect() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("effect.txt");
    let program = checked_program(
        "pub fn write_in_txn(path: string)\n    transaction\n        std::io::writeText(path, \"leaked\")\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    assert_run_error(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(
                &program,
                "test::write_in_txn",
                Value::Str(path.to_string_lossy().into_owned())
            ),
        ),
        RUN_CAPABILITY,
    );
    assert!(
        !path.exists(),
        "host write must be rejected before creating the file"
    );
}

#[test]
fn output_inside_a_transaction_is_rejected_before_the_effect() {
    let program =
        checked_program("pub fn print_in_txn()\n    transaction\n        print(\"leaked\")\n");
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::print_in_txn")),
        RUN_CAPABILITY,
    );
}

#[test]
fn log_inside_a_transaction_is_rejected_before_the_effect() {
    let program = checked_program(
        "pub fn log_in_txn()\n    transaction\n        std::log::info(\"leaked\")\n",
    );
    let store = TreeStore::memory();
    let log = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&log));
    assert_run_error(
        run_entry_with_host(&store, &host, checked_entry!(&program, "test::log_in_txn")),
        RUN_CAPABILITY,
    );
    assert_eq!(log.borrow().as_str(), "");
}

#[test]
fn io_round_trips_bytes_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("blob.bin").to_string_lossy().into_owned();
    let data = Value::Bytes(vec![0, 1, 2, 255, 128]);
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::saveBytes",
            Value::Str(path.clone()),
            data.clone()
        ),
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadBytes", Value::Str(path)),
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(data));
}

#[test]
fn io_without_a_filesystem_capability_is_a_capability_error() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::loadText", Value::Str("x".into())),
    );
    assert_run_error(result, RUN_CAPABILITY);
}

#[test]
fn an_io_error_raises_a_catchable_error() {
    // Reading a missing file (with the capability present) raises a typed Error
    // the program can `catch`, not a runtime fault.
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("absent.txt").to_string_lossy().into_owned();
    let code = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadOrCode", Value::Str(missing)),
    )
    .expect("caught")
    .value;
    assert_eq!(code, Some(Value::Str("io.read".into())));
}

#[test]
fn finally_runs_after_a_fault_and_can_replace_it() {
    // The try body faults (not catchable); finally still runs and its throw
    // replaces the fault, proving finally ran.
    let program = checked_program(
        "pub fn f(): int\n    try\n        const boom = 1 / 0\n    finally\n        throw Error(code: \"cleanup.failed\", message: \"x\")\n    return 0\n",
    );
    let error = run_expecting_error(checked_entry!(&program, "test::f"));
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    let (code, message) = error_throw_fields(&error);
    assert_eq!(code, "cleanup.failed");
    assert_eq!(message, "x");
}

#[test]
fn an_uncaught_throw_without_a_catch_propagates_through_finally() {
    let program = checked_program(
        "pub fn f()\n    try\n        throw Error(code: \"x.y\", message: \"boom\")\n    finally\n        write(\"cleanup\")\n",
    );
    assert_run_error(run(checked_entry!(&program, "test::f")), RUN_UNCAUGHT_THROW);
}

#[test]
fn a_throw_in_finally_replaces_the_outcome() {
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 1\n    finally\n        throw Error(code: \"from.finally\", message: \"x\")\n",
    );
    let error = run_expecting_error(checked_entry!(&program, "test::f"));
    assert_eq!(error.code, RUN_UNCAUGHT_THROW);
    let (code, message) = error_throw_fields(&error);
    assert_eq!(code, "from.finally");
    assert_eq!(message, "x");
}

#[test]
fn a_clean_finally_preserves_a_return() {
    // A finally that completes normally lets the try's `return` through.
    let program = checked_program(
        "pub fn f(): int\n    try\n        return 7\n    finally\n        write(\"cleanup\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn a_throw_caught_inside_a_transaction_commits() {
    // The throw is handled within the transaction, so the body completes normally
    // and the catch's write commits.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn safe(id: int)\n    transaction\n        try\n            throw Error(code: \"x.y\", message: \"b\")\n        catch err: Error\n            ^books(id).title = \"recovered\"\n\n\
         pub fn title(id: int): string\n    return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::safe", Value::Int(1)),
    )
    .expect("safe runs");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title", Value::Int(1))
        )
        .unwrap()
        .value,
        Some(Value::Str("recovered".into()))
    );
}

#[test]
fn throw_inside_a_transaction_rolls_back() {
    // An escaping throw rolls the transaction back, like any other escape.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n\n\
         pub fn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        throw Error(code: \"x.y\", message: \"boom\")\n\n\
         pub fn has_book(id: int): bool\n    return exists(^books(id))\n",
    );
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::risky", Value::Int(1)),
        ),
        RUN_UNCAUGHT_THROW,
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .unwrap()
        .value,
        Some(Value::Bool(false))
    );
}
