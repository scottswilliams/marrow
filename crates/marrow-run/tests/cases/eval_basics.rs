//! Locals and reassignment, boolean and equality operators, integer overflow and
//! division faults, bindings, while and for-range loops, and break/continue.

use crate::support;
use support::*;

use marrow_run::{RUN_DECIMAL_OVERFLOW, RUN_DIVIDE_BY_ZERO, RUN_OVERFLOW, RUN_TYPE, Value};
use marrow_store::Decimal;

#[test]
fn evaluates_locals_and_reassignment() {
    assert_eq!(
        eval_source(
            "pub fn f(n: int): int\n    var total = n\n    total = total + 1\n    return total\n",
            "f",
            vec![Value::Int(41)]
        ),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn formatted_nested_trailing_comma_call_still_evaluates() {
    // A trailing comma makes the inner call format multiline inside single-line
    // enclosing calls. Formatting must preserve meaning: the formatted program
    // still checks and evaluates to the same value.
    let source = "module test\n\nfn g(a: int, b: int): int\n    return a + b\n\nfn h(x: int): int\n    return x\n\npub fn run(): int\n    return h(g(a: 1, b: 2,))\n";
    let formatted = marrow_syntax::format_source(source);
    assert_eq!(
        eval_source(&formatted, "run", vec![]),
        Ok(Some(Value::Int(3)))
    );
}

#[test]
fn formatted_trailing_comma_call_inside_interpolation_still_evaluates() {
    // A string interpolation cannot host newlines, so the trailing-comma call
    // inside it must format inline. Formatting must preserve meaning: the
    // formatted program still checks and evaluates to the same string.
    let source = "module test\n\nfn g(a: int, b: int): int\n    return a + b\n\npub fn run(): string\n    return $\"x{g(a: 1, b: 2,)}y\"\n";
    let formatted = marrow_syntax::format_source(source);
    assert_eq!(
        eval_source(&formatted, "run", vec![]),
        Ok(Some(Value::Str("x3y".to_string())))
    );
}

#[test]
fn evaluates_boolean_logic() {
    let source = "pub fn f(a: bool, b: bool): bool\n    return a and not b\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Bool(true), Value::Bool(false)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        eval_source(source, "f", vec![Value::Bool(true), Value::Bool(true)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn equality_compares_values() {
    // Marrow spells equality `==` (and inequality `!=`); assignment is the
    // single `=`, so equality in expression position uses `==`.
    let source = "pub fn f(a: int, b: int): bool\n    return a == b\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(5), Value::Int(5)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(5), Value::Int(6)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn a_function_that_returns_nothing_yields_none() {
    // Falls off the end with no `return`.
    assert_eq!(
        eval_source(
            "pub fn f(a: int)\n    const x = a + 1\n",
            "f",
            vec![Value::Int(1)]
        ),
        Ok(None)
    );
}

#[test]
fn rejects_division_by_zero() {
    let result = eval_source(
        "pub fn f(a: int): int\n    const boom = a / 0\n    return 0\n",
        "f",
        vec![Value::Int(10)],
    );
    assert_run_error(result, RUN_DIVIDE_BY_ZERO);
}

#[test]
fn division_half_even_rounds_an_inexact_quotient_into_the_envelope() {
    // `/` always yields a decimal. An inexact quotient is rounded half-to-even to
    // the 34-significant-digit / 34-fractional-place envelope rather than faulting.
    let div = "pub fn f(a: decimal, b: decimal): decimal\n    return a / b\n";
    let dec = |raw: &str| Value::Decimal(Decimal::parse(raw).expect("canonical decimal"));

    assert_eq!(
        eval_source(div, "f", vec![dec("1"), dec("3")]),
        Ok(Some(dec("0.3333333333333333333333333333333333")))
    );
    assert_eq!(
        eval_source(div, "f", vec![dec("2"), dec("3")]),
        Ok(Some(dec("0.6666666666666666666666666666666667")))
    );

    // An exact quotient is exact, with no rounding artifact.
    assert_eq!(
        eval_source(div, "f", vec![dec("6"), dec("2")]),
        Ok(Some(dec("3")))
    );

    // Only a quotient whose magnitude leaves the envelope faults.
    let huge = "9999999999999999999999999999999999";
    assert_run_error(
        eval_source(div, "f", vec![dec(huge), dec("0.1")]),
        RUN_DECIMAL_OVERFLOW,
    );
}

#[test]
fn integer_remainder_by_zero_reports_one_consistent_message() {
    // The `%` operator and `std::math::remainder`/`modulo` are the same integer
    // remainder, so a zero divisor must report the same divide-by-zero message.
    let result = eval_source(
        "pub fn f(a: int): int\n    return a % 0\n",
        "f",
        vec![Value::Int(10)],
    );
    let Err(error) = result else {
        panic!("expected an error, got {result:?}");
    };
    assert_eq!(error.code(), RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.message, "integer remainder by zero");

    // std::math::modulo routes through the same integer-remainder path.
    let program = checked_program("pub fn g(): int\n    return std::math::modulo(7, 0)\n");
    assert_eq!(
        run(checked_entry!(&program, "test::g"))
            .unwrap_err()
            .message,
        "integer remainder by zero"
    );
}

#[test]
fn detects_integer_overflow() {
    let result = eval_source(
        "pub fn f(a: int): int\n    return a * a\n",
        "f",
        vec![Value::Int(i64::MAX)],
    );
    assert_run_error(result, RUN_OVERFLOW);
}

#[test]
fn detects_an_over_range_integer_literal() {
    checker_rejects(
        "pub fn f(): int\n    return 99999999999999999999999999\n",
        "check.literal_range",
    );
}

#[test]
fn evaluates_the_i64_min_literal() {
    // `i64::MIN` is written as `-9223372036854775808`; its bare magnitude is
    // `i64::MAX + 1` and out of range, so the negated literal must reach the runtime
    // as a single folded value rather than overflowing before the minus applies.
    assert_eq!(
        eval_source(
            "pub fn f(): int\n    return -9223372036854775808\n",
            "f",
            vec![],
        ),
        Ok(Some(Value::Int(i64::MIN)))
    );
}

#[test]
fn detects_an_over_envelope_decimal_literal() {
    checker_rejects(
        "pub fn f(): decimal\n    return 9.9999999999999999999999999999999999\n",
        "check.literal_range",
    );
}

#[test]
fn rejects_an_unbound_name() {
    checker_rejects("pub fn f(): int\n    return x\n", "check.unresolved_name");
}

#[test]
fn rejects_assignment_to_an_immutable_binding() {
    // Reassigning a `const` is rejected at check, since immutability is statically
    // known — the runtime never sees the program.
    checker_rejects(
        "pub fn f(): int\n    const x = 1\n    x = 2\n    return x\n",
        "check.invalid_assign_target",
    );
}

#[test]
fn a_local_const_binds_a_runtime_computed_value() {
    // `const` is the immutable local binding. Unlike a module constant, its
    // initializer may be any expression — here a call resolved at run time.
    let program = checked_program(
        "pub fn double(n: int): int\n    return n * 2\npub fn f(): int\n    const x = double(5)\n    return x\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")),
        Ok(Some(Value::Int(10)))
    );
}

#[test]
fn rejects_an_argument_count_mismatch() {
    let program = checked_program("pub fn add(a: int, b: int): int\n    return a + b\n");
    let error = rejected_entry_call(&program, "test::add", vec![Value::Int(1)]);
    assert_eq!(error.code(), RUN_TYPE);
}

#[test]
fn reports_an_unsupported_construct() {
    checker_rejects("pub fn f(): int\n    return 1..3\n", "check.range_value");
}

#[test]
fn an_if_condition_must_be_boolean() {
    checker_rejects(
        "pub fn f(a: int): int\n    if a\n        return 1\n    return 0\n",
        "check.condition_type",
    );
}

#[test]
fn an_inner_scope_shadows_then_restores_an_outer_binding() {
    // `const x = 1` inside the if-block shadows only within that block; after it,
    // the outer `x` (99) is what `return x` sees.
    assert_eq!(
        eval_source(
            "pub fn f(): int\n    const x = 99\n    if true\n        const x = 1\n    return x\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Int(99)))
    );
}

#[test]
fn an_else_if_chain_selects_the_matching_branch() {
    let source = "pub fn grade(n: int): int\n    if n > 90\n        return 1\n    else if n > 80\n        return 2\n    else\n        return 3\n";
    assert_eq!(
        eval_source(source, "grade", vec![Value::Int(95)]),
        Ok(Some(Value::Int(1)))
    );
    assert_eq!(
        eval_source(source, "grade", vec![Value::Int(85)]),
        Ok(Some(Value::Int(2)))
    );
    assert_eq!(
        eval_source(source, "grade", vec![Value::Int(50)]),
        Ok(Some(Value::Int(3)))
    );
}

#[test]
fn remainder_by_negative_one_is_zero_for_every_dividend() {
    // `x % -1 == 0` mathematically for every `x`: the remainder always fits even
    // though the quotient `i64::MIN / -1` would overflow. The `%` operator and the
    // `std::math::remainder`/`modulo` functions share one integer-remainder path,
    // so all three must yield 0 at `i64::MIN` rather than faulting `run.overflow`.
    let op = checked_program("pub fn f(a: int, b: int): int\n    return a % b\n");
    let remainder =
        checked_program("pub fn f(a: int, b: int): int\n    return std::math::remainder(a, b)\n");
    let modulo =
        checked_program("pub fn f(a: int, b: int): int\n    return std::math::modulo(a, b)\n");
    for program in [&op, &remainder, &modulo] {
        assert_eq!(
            run(checked_entry!(
                program,
                "test::f",
                Value::Int(i64::MIN),
                Value::Int(-1)
            )),
            Ok(Some(Value::Int(0)))
        );
        assert_eq!(
            run(checked_entry!(
                program,
                "test::f",
                Value::Int(-7),
                Value::Int(-1)
            )),
            Ok(Some(Value::Int(0)))
        );
    }
}

#[test]
fn evaluates_a_while_loop() {
    let source = "pub fn sum(n: int): int\n    var total = 0\n    var i = 1\n    while i <= n\n        total = total + i\n        i = i + 1\n    return total\n";
    assert_eq!(
        eval_source(source, "sum", vec![Value::Int(5)]),
        Ok(Some(Value::Int(15)))
    );
}

#[test]
fn evaluates_an_inclusive_for_range() {
    let source = "pub fn sum(n: int): int\n    var total = 0\n    for i in 1..=n\n        total = total + i\n    return total\n";
    assert_eq!(
        eval_source(source, "sum", vec![Value::Int(5)]),
        Ok(Some(Value::Int(15)))
    );
}

#[test]
fn an_exclusive_for_range_stops_before_the_end() {
    let source = "pub fn range_count(n: int): int\n    var c = 0\n    for i in 0..n\n        c = c + 1\n    return c\n";
    assert_eq!(
        eval_source(source, "range_count", vec![Value::Int(5)]),
        Ok(Some(Value::Int(5)))
    );
}

#[test]
fn an_int_range_steps_by_a_positive_by_value() {
    // `1..10 by 2` yields 1, 3, 5, 7, 9 (exclusive end), summing to 25.
    assert_eq!(
        eval_source(
            "pub fn f(): int\n    var total = 0\n    for i in 1..10 by 2\n        total = total + i\n    return total\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Int(25)))
    );
}

#[test]
fn an_int_range_steps_down_with_a_negative_by_value() {
    // `10..1 by -1` counts down 10..2 (exclusive end) — nine iterations from 10 to 2.
    let source = "pub fn f(): int\n    var last = 0\n    var count = 0\n    for i in 10..1 by -1\n        last = i\n        count = count + 1\n    return count * 100 + last\n";
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(902)))
    );
}

#[test]
fn an_inclusive_descending_range_reaches_its_end() {
    // `10..=1 by -1` includes 1, so the final bound is reached.
    let source = "pub fn f(): int\n    var last = 99\n    for i in 10..=1 by -1\n        last = i\n    return last\n";
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn a_wrong_direction_variable_step_is_an_empty_loop() {
    // A runtime wrong-direction step never loops forever: it iterates zero times.
    // `1..10 by step` with step = -1 runs the body never.
    let source = "pub fn f(step: int): int\n    var count = 0\n    for i in 1..10 by step\n        count = count + 1\n    return count\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(-1)]),
        Ok(Some(Value::Int(0)))
    );
}

#[test]
fn a_default_wrong_direction_range_is_an_empty_loop() {
    // `lo..hi` with lo > hi and the default +1 step iterates zero times.
    let source = "pub fn f(lo: int, hi: int): int\n    var count = 0\n    for i in lo..hi\n        count = count + 1\n    return count\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(10), Value::Int(1)]),
        Ok(Some(Value::Int(0)))
    );
}

#[test]
fn a_runtime_zero_step_faults() {
    // A zero step would never progress; a non-literal zero faults rather than hangs.
    let source =
        "pub fn f(step: int): int\n    for i in 1..10 by step\n        return i\n    return 0\n";
    let result = eval_source(source, "f", vec![Value::Int(0)]);
    assert_run_error(result, RUN_TYPE);
}

#[test]
fn an_int_range_can_drive_decimal_work() {
    assert_eq!(
        eval_source(
            "pub fn f(): string\n    var total: decimal = 0.0\n    for i in 0..4\n        var x: decimal = decimal(i) * 0.25\n        total = total + x\n    return string(total)\n",
            "f",
            Vec::new()
        ),
        Ok(Some(Value::Str("1.5".into())))
    );
}

#[test]
fn a_date_range_steps_one_calendar_day_across_a_leap_boundary() {
    // 2024-02-27..=2024-03-02 by 1.day lands on Feb 28, 29, Mar 1, 2 in a leap year:
    // calendar arithmetic, not 30-day months.
    let program = checked_program(
        "pub fn f(): string\n    var acc = \"\"\n    for d in std::clock::parseDate(\"2024-02-27\")..=std::clock::parseDate(\"2024-03-02\") by 1.day\n        acc = acc + std::clock::formatDate(d) + \";\"\n    return acc\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str(
            "2024-02-27;2024-02-28;2024-02-29;2024-03-01;2024-03-02;".into()
        ))
    );
}

#[test]
fn a_date_range_rejects_a_sub_day_step() {
    checker_rejects(
        "pub fn f(): int\n    var count = 0\n    for d in std::clock::parseDate(\"2024-01-01\")..std::clock::parseDate(\"2024-01-10\") by 1.hour\n        count = count + 1\n    return count\n",
        "check.range",
    );
}

#[test]
fn an_instant_range_steps_by_a_duration_in_utc() {
    // Stepping an instant range by 1.hour from noon to 3pm UTC yields noon, 1pm, 2pm
    // (exclusive end): three instants.
    let program = checked_program(
        "pub fn f(): string\n    var acc = \"\"\n    for t in std::clock::parseInstant(\"2024-03-10T12:00:00Z\")..std::clock::parseInstant(\"2024-03-10T15:00:00Z\") by 1.hour\n        acc = acc + std::clock::formatInstant(t) + \";\"\n    return acc\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str(
            "2024-03-10T12:00:00Z;2024-03-10T13:00:00Z;2024-03-10T14:00:00Z;".into()
        ))
    );
}

#[test]
fn break_exits_the_loop() {
    let source = "pub fn f(n: int): int\n    var i = 0\n    while true\n        if i > n\n            break\n        i = i + 1\n    return i\n";
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(3)]),
        Ok(Some(Value::Int(4)))
    );
}

#[test]
fn continue_skips_to_the_next_iteration() {
    let source = "pub fn f(n: int): int\n    var c = 0\n    for i in 1..=n\n        if i == 1\n            continue\n        c = c + 1\n    return c\n";
    // The first iteration is skipped; the rest count.
    assert_eq!(
        eval_source(source, "f", vec![Value::Int(3)]),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn function_extraction_replaces_labeled_outer_loop_exit() {
    let source = "pub fn count_until(): int\n    var count = 0\n    for i in 1..=3\n        for j in 1..=3\n            if j == 2\n                return count\n            count = count + 1\n    return count\n\npub fn f(): int\n    return count_until()\n";
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(1)))
    );
}

#[test]
fn an_unlabeled_break_exits_only_the_inner_loop() {
    let source = "pub fn f(): int\n    var count = 0\n    for i in 1..=2\n        for j in 1..=3\n            if j == 2\n                break\n            count = count + 1\n    return count\n";
    // Each outer iteration counts j=1 then breaks the inner loop: 2 total.
    assert_eq!(
        eval_source(source, "f", Vec::new()),
        Ok(Some(Value::Int(2)))
    );
}

#[test]
fn break_outside_a_loop_is_an_error() {
    checker_rejects("pub fn f()\n    break\n", "check.loop_control_flow");
}
