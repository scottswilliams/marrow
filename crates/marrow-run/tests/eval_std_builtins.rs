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
fn std_text_gate16_builtins_use_unicode_scalar_indexes() {
    let program = checked_program(
        "pub fn sliced(): string\n    return std::text::slice(\"aé𝄞z\", 1, 3)\n\n\
         pub fn found(): int\n    return std::text::indexOf(\"aé𝄞z\", \"𝄞\") ?? -1\n\n\
         pub fn missing(): int\n    return std::text::indexOf(\"aé𝄞z\", \"x\") ?? -1\n\n\
         pub fn joined(): string\n    return std::text::join(std::text::split(\"a,b,c\", \",\"), \"|\")\n\n\
         pub fn replaced(): string\n    return std::text::replace(\"café café\", \"fé\", \"FE\")\n\n\
         pub fn checks(): bool\n    return std::text::startsWith(\"café\", \"ca\") and std::text::endsWith(\"café\", \"fé\")\n\n\
         pub fn upper(): string\n    return std::text::toUpper(\"hé\")\n\n\
         pub fn lower(): string\n    return std::text::toLower(\"HÉ\")\n\n\
         pub fn upper_simple(): string\n    return std::text::toUpper(\"ß\")\n\n\
         pub fn upper_simple_length(): int\n    return std::text::length(std::text::toUpper(\"ß\"))\n\n\
         pub fn upper_simple_greek(): string\n    return std::text::toUpper(\"ᾀ\")\n\n\
         pub fn upper_simple_greek_length(): int\n    return std::text::length(std::text::toUpper(\"ᾀ\"))\n\n\
         pub fn lower_simple(): string\n    return std::text::toLower(\"İ\")\n\n\
         pub fn lower_simple_length(): int\n    return std::text::length(std::text::toLower(\"İ\"))\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::sliced")).unwrap(),
        Some(Value::Str("é𝄞".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::found")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::missing")).unwrap(),
        Some(Value::Int(-1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::joined")).unwrap(),
        Some(Value::Str("a|b|c".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::replaced")).unwrap(),
        Some(Value::Str("caFE caFE".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::checks")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::upper")).unwrap(),
        Some(Value::Str("HÉ".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lower")).unwrap(),
        Some(Value::Str("hé".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::upper_simple")).unwrap(),
        Some(Value::Str("ß".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::upper_simple_length")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::upper_simple_greek")).unwrap(),
        Some(Value::Str("ᾈ".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::upper_simple_greek_length")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lower_simple")).unwrap(),
        Some(Value::Str("i".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lower_simple_length")).unwrap(),
        Some(Value::Int(1))
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
fn std_math_gate16_builtins_round_and_bound_values() {
    let program = checked_program(
        "pub fn min_int(): int\n    return std::math::minInt(7, -2)\n\n\
         pub fn max_int(): int\n    return std::math::maxInt(7, -2)\n\n\
         pub fn min_decimal(): string\n    return string(std::math::minDecimal(1.5, -2.25))\n\n\
         pub fn max_decimal(): string\n    return string(std::math::maxDecimal(1.5, -2.25))\n\n\
         pub fn round_down_even(): int\n    return std::math::round(2.5)\n\n\
         pub fn round_up_even(): int\n    return std::math::round(3.5)\n\n\
         pub fn round_negative_even(): int\n    return std::math::round(-2.5)\n\n\
         pub fn ceiling_negative(): int\n    return std::math::ceiling(-2.1)\n\n\
         pub fn pow_ok(): int\n    return std::math::powInt(3, 4)\n\n\
         pub fn pow_overflow(): int\n    return std::math::powInt(3037000500, 2)\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::min_int")).unwrap(),
        Some(Value::Int(-2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::max_int")).unwrap(),
        Some(Value::Int(7))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::min_decimal")).unwrap(),
        Some(Value::Str("-2.25".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::max_decimal")).unwrap(),
        Some(Value::Str("1.5".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::round_down_even")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::round_up_even")).unwrap(),
        Some(Value::Int(4))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::round_negative_even")).unwrap(),
        Some(Value::Int(-2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::ceiling_negative")).unwrap(),
        Some(Value::Int(-2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::pow_ok")).unwrap(),
        Some(Value::Int(81))
    );
    assert_run_error(
        run(checked_entry!(&program, "test::pow_overflow")),
        marrow_run::RUN_OVERFLOW,
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
