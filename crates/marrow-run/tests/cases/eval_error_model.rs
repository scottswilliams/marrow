//! The throw / catch / try error model: the Error constructor shape, catchable
//! faults, propagation across calls, pending-throw hygiene, and transaction
//! commit and rollback under throws.

use crate::support;
use support::*;

use marrow_run::{
    RUN_DECIMAL_OVERFLOW, RUN_DIVIDE_BY_ZERO, RUN_OVERFLOW, RUN_TYPE, RUN_UNCAUGHT_THROW, Value,
};
use marrow_store::tree::TreeStore;

#[test]
fn throw_surfaces_as_an_uncaught_error() {
    let program = checked_program(
        "pub fn bad()\n    throw Error(code: \"book.absent\", message: \"no book\")\n",
    );
    let error = run_expecting_error(checked_entry!(&program, "test::bad"));
    assert_eq!(error.code(), RUN_UNCAUGHT_THROW);
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
        "pub fn bad()\n    throw Error(code: \"test.error\", message: \"m\", oops: \"!\")\n",
        "check.call_argument",
    );
}

#[test]
fn error_constructor_rejects_non_string_builtin_fields() {
    // Each builtin `Error` field carries a declared type; a value of the wrong
    // type is a constructor error the checker rejects.
    for source in [
        "pub fn bad()\n    throw Error(code: true, message: \"m\")\n",
        "pub fn bad()\n    throw Error(code: \"test.error\", message: true)\n",
        "pub fn bad()\n    throw Error(code: \"test.error\", message: \"m\", help: true)\n",
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
fn catch_cleanup_can_rethrow_to_an_outer_handler() {
    let program = checked_program(
        "pub fn run_it(): string\n    try\n        try\n            throw Error(code: \"x.y\", message: \"b\")\n        catch err: Error\n            print(\"cleanup\")\n            throw err\n    catch err: Error\n        return err.code\n    return \"none\"\n",
    );
    let outcome = run_full(checked_entry!(&program, "test::run_it")).expect("outer catch");
    assert_eq!(outcome.value, Some(Value::Str("x.y".into())));
    assert_eq!(outcome.output, "cleanup\n");
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
         \x20       var text: string = std::clock::formatInstant(std::clock::parseInstant(\"9999-12-31T23:59:59Z\") + 1.day)\n\
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
        Ok(Some(Value::Str("run.temporal_overflow".into())))
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
        "resource Account\n    balance: int\nstore ^accts(id: int): Account\n\npub fn fail()\n    throw Error(code: \"test.fail\", message: \"boom\")\n\npub fn run_it()\n    transaction\n        ^accts(1).balance = 5\n        fail()\n\npub fn read(): int\n    return ^accts(1).balance ?? -1\n",
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
        "pub fn callee()\n    throw Error(code: \"test.e1\", message: \"boom\")\npub fn check(): int\n    try\n        callee()\n    catch err: Error\n        print(\"caught\")\n    try\n        const boom = 1 / 0\n    catch err: Error\n        return 99\n    return 0\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::check")),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn a_throw_caught_inside_a_transaction_commits() {
    // The throw is handled within the transaction, so the body completes normally
    // and the catch's write commits.
    let program = checked_program(
        "resource Book\n    required title: string\nstore ^books(id: int): Book\n\n\
         pub fn safe(id: int)\n    transaction\n        try\n            throw Error(code: \"x.y\", message: \"b\")\n        catch err: Error\n            ^books(id).title = \"recovered\"\n\n\
         pub fn title(id: int): string\n    return ^books(id).title ?? \"\"\n",
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
        "resource Book\n    required title: string\nstore ^books(id: int): Book\n\n\
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
