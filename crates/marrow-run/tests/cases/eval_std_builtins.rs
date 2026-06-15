//! The std::text and std::math builtins and their argument-type rejections.

use crate::support;
use support::*;

use marrow_run::{RUN_DIVIDE_BY_ZERO, RUN_TYPE, Value};

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
fn std_math_pow_int_large_exponents_do_not_truncate() {
    let program = checked_program(
        "pub fn wrap_to_zero(): int\n    return std::math::powInt(2, 4294967296)\n\n\
         pub fn wrap_to_one(): int\n    return std::math::powInt(2, 4294967297)\n",
    );

    assert_run_error(
        run(checked_entry!(&program, "test::wrap_to_zero")),
        marrow_run::RUN_OVERFLOW,
    );
    assert_run_error(
        run(checked_entry!(&program, "test::wrap_to_one")),
        marrow_run::RUN_OVERFLOW,
    );
}

#[test]
fn std_math_pow_int_large_bounded_results_are_exact() {
    let program = checked_program(
        "pub fn one(): int\n    return std::math::powInt(1, 4294967296)\n\n\
         pub fn zero(): int\n    return std::math::powInt(0, 4294967296)\n\n\
         pub fn negative_one_even(): int\n    return std::math::powInt(-1, 4294967296)\n\n\
         pub fn negative_one_odd(): int\n    return std::math::powInt(-1, 4294967297)\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::one")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::zero")).unwrap(),
        Some(Value::Int(0))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::negative_one_even")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::negative_one_odd")).unwrap(),
        Some(Value::Int(-1))
    );
}

#[test]
fn std_math_round_decimal_returns_canonical_decimal() {
    let program = checked_program(
        "pub fn money_seed(): string\n    return string(std::math::roundDecimal(12.345, 2))\n\n\
         pub fn positive_half_up_to_even(): string\n    return string(std::math::roundDecimal(12.355, 2))\n\n\
         pub fn negative_half_down_to_even(): string\n    return string(std::math::roundDecimal(-2.345, 2))\n\n\
         pub fn negative_half_up_to_even(): string\n    return string(std::math::roundDecimal(-2.355, 2))\n\n\
         pub fn zero_scale_down_to_even(): string\n    return string(std::math::roundDecimal(2.5, 0))\n\n\
         pub fn zero_scale_up_to_even(): string\n    return string(std::math::roundDecimal(3.5, 0))\n\n\
         pub fn no_trailing_zero_promise(): string\n    return string(std::math::roundDecimal(1.2, 2))\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::money_seed")).unwrap(),
        Some(Value::Str("12.34".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::positive_half_up_to_even")).unwrap(),
        Some(Value::Str("12.36".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::negative_half_down_to_even")).unwrap(),
        Some(Value::Str("-2.34".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::negative_half_up_to_even")).unwrap(),
        Some(Value::Str("-2.36".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::zero_scale_down_to_even")).unwrap(),
        Some(Value::Str("2".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::zero_scale_up_to_even")).unwrap(),
        Some(Value::Str("4".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::no_trailing_zero_promise")).unwrap(),
        Some(Value::Str("1.2".into()))
    );
}

#[test]
fn std_math_round_decimal_rejects_invalid_scale() {
    let program = checked_program(
        "pub fn negative_scale(): decimal\n    return std::math::roundDecimal(1.2, -1)\n\n\
         pub fn too_large_scale(): decimal\n    return std::math::roundDecimal(1.2, 35)\n",
    );

    assert_run_error(
        run(checked_entry!(&program, "test::negative_scale")),
        RUN_TYPE,
    );
    assert_run_error(
        run(checked_entry!(&program, "test::too_large_scale")),
        RUN_TYPE,
    );
}

#[test]
fn std_json_scalar_helpers_read_present_absent_and_invalid_values() {
    let program = checked_program(
        r#"pub fn valid(): bool
    return std::json::valid("{\"user\":{\"name\":\"Ada\",\"age\":37,\"score\":12.5,\"active\":true,\"tags\":[\"db\",\"lang\"]}}")

pub fn name(): string
    return std::json::string("{\"user\":{\"name\":\"Ada\",\"age\":37,\"score\":12.5,\"active\":true,\"tags\":[\"db\",\"lang\"]}}", "/user/name") ?? ""

pub fn age(): int
    return std::json::int("{\"user\":{\"name\":\"Ada\",\"age\":37,\"score\":12.5,\"active\":true,\"tags\":[\"db\",\"lang\"]}}", "/user/age") ?? -1

pub fn score(): string
    return string(std::json::decimal("{\"user\":{\"name\":\"Ada\",\"age\":37,\"score\":12.5,\"active\":true,\"tags\":[\"db\",\"lang\"]}}", "/user/score") ?? 0.0)

pub fn active(): bool
    return std::json::bool("{\"user\":{\"name\":\"Ada\",\"age\":37,\"score\":12.5,\"active\":true,\"tags\":[\"db\",\"lang\"]}}", "/user/active") ?? false

pub fn tag_count(): int
    return std::json::count("{\"user\":{\"name\":\"Ada\",\"age\":37,\"score\":12.5,\"active\":true,\"tags\":[\"db\",\"lang\"]}}", "/user/tags") ?? -1

pub fn missing(): string
    return std::json::string("{\"user\":null}", "/user/name") ?? "absent"

pub fn wrong_kind(): string
    return std::json::string("{\"user\":{\"age\":37}}", "/user/age") ?? "wrong"

pub fn bad_pointer(): string
    return std::json::string("{}", "user") ?? ""
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::valid")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::name")).unwrap(),
        Some(Value::Str("Ada".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::age")).unwrap(),
        Some(Value::Int(37))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::score")).unwrap(),
        Some(Value::Str("12.5".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::active")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tag_count")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::missing")).unwrap(),
        Some(Value::Str("absent".into()))
    );
    assert_run_error(run(checked_entry!(&program, "test::wrong_kind")), RUN_TYPE);
    assert_run_error(run(checked_entry!(&program, "test::bad_pointer")), RUN_TYPE);
}

#[test]
fn std_json_rejects_lossy_or_ambiguous_scalar_reads() {
    let program = checked_program(
        r#"pub fn high_precision_decimal(): string
    return string(std::json::decimal("{\"n\":0.1234567890123456789}", "/n") ?? 0.0)

pub fn duplicate_key(): int
    return std::json::int("{\"a\":1,\"a\":2}", "/a") ?? -1

pub fn leading_zero_index(): string
    return std::json::string("[\"a\",\"b\"]", "/01") ?? "absent"

pub fn private_number_key(): string
    return std::json::string("{\"$serde_json::private::Number\":\"kept\"}", "/$serde_json::private::Number") ?? "absent"

pub fn negative_zero_int(): int
    return std::json::int("{\"n\":-0}", "/n") ?? 9

pub fn negative_zero_decimal(): string
    return string(std::json::decimal("{\"n\":-0}", "/n") ?? 9.9)
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::high_precision_decimal")).unwrap(),
        Some(Value::Str("0.1234567890123456789".into()))
    );
    assert_run_error(
        run(checked_entry!(&program, "test::duplicate_key")),
        RUN_TYPE,
    );
    assert_run_error(
        run(checked_entry!(&program, "test::leading_zero_index")),
        RUN_TYPE,
    );
    assert_eq!(
        run(checked_entry!(&program, "test::private_number_key")).unwrap(),
        Some(Value::Str("kept".into()))
    );
    assert_run_error(
        run(checked_entry!(&program, "test::negative_zero_int")),
        RUN_TYPE,
    );
    assert_run_error(
        run(checked_entry!(&program, "test::negative_zero_decimal")),
        RUN_TYPE,
    );
}

#[test]
fn std_csv_scalar_helpers_read_present_absent_and_invalid_values() {
    let program = checked_program(
        r#"pub fn rows(): int
    return std::csv::rowCount("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n")

pub fn has_balance(): bool
    return std::csv::hasColumn("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n", "balance")

pub fn name(): string
    return std::csv::string("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n", 0, "name") ?? ""

pub fn age(): int
    return std::csv::int("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n", 0, "age") ?? -1

pub fn balance(): string
    return string(std::csv::decimal("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n", 0, "balance") ?? 0.0)

pub fn active(): bool
    return std::csv::bool("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n", 1, "active") ?? true

pub fn empty_cell(): string
    return std::csv::string("name,age,balance,active\nAda,37,12.5,true\nBob,,0,false\n", 1, "age") ?? "absent"

pub fn duplicate_header(): int
    return std::csv::rowCount("name,name\nAda,Lovelace\n")
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::rows")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::has_balance")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::name")).unwrap(),
        Some(Value::Str("Ada".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::age")).unwrap(),
        Some(Value::Int(37))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::balance")).unwrap(),
        Some(Value::Str("12.5".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::active")).unwrap(),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::empty_cell")).unwrap(),
        Some(Value::Str("absent".into()))
    );
    assert_run_error(
        run(checked_entry!(&program, "test::duplicate_header")),
        RUN_TYPE,
    );
}

#[test]
fn std_id_helpers_return_plain_strings() {
    let program = checked_program(
        r#"pub fn slug(): string
    return std::id::slug(" Hello, Marrow_ID! ")

pub fn uuid(): string
    return std::id::stableUuid("alpha")
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::slug")).unwrap(),
        Some(Value::Str("hello-marrow-id".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::uuid")).unwrap(),
        Some(Value::Str("8ed3f6ad-685b-459e-ad70-22518e1af76c".into()))
    );
}

#[test]
fn std_random_helpers_are_deterministic_and_bounded() {
    let program = checked_program(
        r#"pub fn random_int_is_stable_and_bounded(): bool
    const first: int = std::random::int("seed", 2, 10, 20)
    const second: int = std::random::int("seed", 2, 10, 20)
    return first == second and first >= 10 and first <= 20

pub fn random_bool_is_stable(): bool
    return std::random::bool("seed", 3) == std::random::bool("seed", 3)

pub fn random_decimal_is_stable(): bool
    return string(std::random::decimal("seed", 4)) == string(std::random::decimal("seed", 4))

pub fn random_cross_zero_range_is_bounded(): bool
    const value: int = std::random::int("seed", 5, -1, 9223372036854775807)
    return value >= -1 and value <= 9223372036854775807

pub fn random_full_int_range_returns(): int
    return std::random::int("seed", 0, (0 - 9223372036854775807) - 1, 9223372036854775807)

pub fn random_bad_step(): int
    return std::random::int("seed", -1, 0, 9)
"#,
    );

    assert_eq!(
        run(checked_entry!(
            &program,
            "test::random_int_is_stable_and_bounded"
        ))
        .unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::random_bool_is_stable")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::random_decimal_is_stable")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::random_cross_zero_range_is_bounded"
        ))
        .unwrap(),
        Some(Value::Bool(true))
    );
    assert!(matches!(
        run(checked_entry!(
            &program,
            "test::random_full_int_range_returns"
        )),
        Ok(Some(Value::Int(_)))
    ));
    assert_run_error(
        run(checked_entry!(&program, "test::random_bad_step")),
        RUN_TYPE,
    );
}

#[test]
fn std_audit_helpers_build_json_strings_without_writes() {
    let program = checked_program(
        r#"pub fn audit_event(): string
    return std::audit::event("create", "ada", "book")

pub fn audit_event_args(action: string, actor: string, subject: string): string
    return std::audit::event(action, actor, subject)

pub fn audit_change(): string
    return std::audit::change("title", "old", "new")

pub fn audit_change_args(field: string, before: string, after: string): string
    return std::audit::change(field, before, after)
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::audit_event")).unwrap(),
        Some(Value::Str(
            "{\"action\":\"create\",\"actor\":\"ada\",\"subject\":\"book\"}".into()
        ))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::audit_change")).unwrap(),
        Some(Value::Str(
            "{\"field\":\"title\",\"before\":\"old\",\"after\":\"new\"}".into()
        ))
    );

    let json = |text: &str| serde_json::Value::String(text.to_owned()).to_string();
    let escaped_action = "create \"quoted\"";
    let escaped_actor = "ada\\lovelace\nops";
    let escaped_subject = "book\u{001f}";
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::audit_event_args",
            Value::Str(escaped_action.into()),
            Value::Str(escaped_actor.into()),
            Value::Str(escaped_subject.into())
        ))
        .unwrap(),
        Some(Value::Str(format!(
            "{{\"action\":{},\"actor\":{},\"subject\":{}}}",
            json(escaped_action),
            json(escaped_actor),
            json(escaped_subject)
        )))
    );

    let escaped_field = "title\"edition";
    let escaped_before = "old\\draft";
    let escaped_after = "new\nfinal";
    assert_eq!(
        run(checked_entry!(
            &program,
            "test::audit_change_args",
            Value::Str(escaped_field.into()),
            Value::Str(escaped_before.into()),
            Value::Str(escaped_after.into())
        ))
        .unwrap(),
        Some(Value::Str(format!(
            "{{\"field\":{},\"before\":{},\"after\":{}}}",
            json(escaped_field),
            json(escaped_before),
            json(escaped_after)
        )))
    );
}

#[test]
fn std_matrix_helpers_use_canonical_text_and_exact_arithmetic() {
    let program = checked_program(
        r#"pub fn matrix_parse(): string
    return std::matrix::parse("[1, 2; 3.5, 4]")

pub fn matrix_shape(): string
    const m: string = std::matrix::parse("[1,2;3,4]")
    return $"{std::matrix::rows(m)}x{std::matrix::cols(m)}"

pub fn matrix_identity(): string
    return std::matrix::identity(3)

pub fn matrix_get(): string
    return string(std::matrix::get(std::matrix::parse("[1,2;3.5,4]"), 1, 0))

pub fn matrix_add(): string
    return std::matrix::add("[1,2;3,4]", "[0.5,1;1.5,2]")

pub fn matrix_multiply(): string
    return std::matrix::multiply("[1,2;3,4]", "[5;6]")

pub fn matrix_transpose(): string
    return std::matrix::transpose("[1,2;3,4]")

pub fn matrix_bad(): string
    return std::matrix::parse("[1,2;3]")
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::matrix_parse")).unwrap(),
        Some(Value::Str("[1,2;3.5,4]".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::matrix_shape")).unwrap(),
        Some(Value::Str("2x2".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::matrix_identity")).unwrap(),
        Some(Value::Str("[1,0,0;0,1,0;0,0,1]".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::matrix_get")).unwrap(),
        Some(Value::Str("3.5".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::matrix_add")).unwrap(),
        Some(Value::Str("[1.5,3;4.5,6]".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::matrix_multiply")).unwrap(),
        Some(Value::Str("[17;39]".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::matrix_transpose")).unwrap(),
        Some(Value::Str("[1,3;2,4]".into()))
    );
    assert_run_error(run(checked_entry!(&program, "test::matrix_bad")), RUN_TYPE);
}

#[test]
fn std_math_clamp_helpers_bound_values() {
    let program = checked_program(
        r#"pub fn clamp_int(): int
    return std::math::clampInt(12, 0, 10)

pub fn clamp_decimal(): string
    return string(std::math::clampDecimal(-1.5, 0.0, 10.0))
"#,
    );

    assert_eq!(
        run(checked_entry!(&program, "test::clamp_int")).unwrap(),
        Some(Value::Int(10))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::clamp_decimal")).unwrap(),
        Some(Value::Str("0".into()))
    );
}

#[test]
fn std_error_helpers_read_error_fields() {
    let program = checked_program(
        "pub fn helpers(): string\n    try\n        throw Error(code: \"app.fail\", message: \"boom\")\n    catch err: Error\n        if std::error::hasCode(err, \"app.fail\")\n            return std::error::code(err) + \":\" + std::error::message(err)\n    return \"\"\n",
    );

    assert_eq!(
        run(checked_entry!(&program, "test::helpers")).unwrap(),
        Some(Value::Str("app.fail:boom".into()))
    );
}

#[test]
fn std_error_has_code_validates_expected_code_text() {
    let program = checked_program(
        "pub fn invalid(): bool\n    try\n        throw Error(code: \"app.fail\", message: \"boom\")\n    catch err: Error\n        return std::error::hasCode(err, \"App.fail\")\n",
    );

    assert_run_error(run(checked_entry!(&program, "test::invalid")), RUN_TYPE);
}

#[test]
fn std_matrix_rejects_oversized_text_before_canonicalizing() {
    let program = checked_program(
        "pub fn parse(text: string): string\n    return std::matrix::parse(text)\n",
    );
    let oversized = format!("[{}1]", " ".repeat(1_048_577));

    assert_run_error(
        run(checked_entry!(
            &program,
            "test::parse",
            Value::Str(oversized)
        )),
        RUN_TYPE,
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
