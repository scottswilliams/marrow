//! The std::text and std::math builtins and their argument-type rejections.

#[macro_use]
mod support;

use support::*;

use marrow_run::{RUN_DIVIDE_BY_ZERO, Value};

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
