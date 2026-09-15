//! The integer-bound value built-ins `maxInt` and `minInt` through the production
//! `compile` path: they type as `int` in value position, fold as a constant
//! initializer, are reserved against a colliding declaration, and reject a call form.
//! The owner ruling is that no source spells `9223372036854775807`; the language
//! provides the bound as a named value.

use marrow_codes::Code;
use marrow_compile::SourceDiagnostic;

use super::{compile_err, compile_ok, wrap};

fn has_code(diagnostics: &[SourceDiagnostic], code: Code) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code() == code)
}

#[test]
fn the_int_bounds_type_as_int_in_value_position() {
    compile_ok(&wrap(
        r#"pub fn hi(): int {
    return maxInt
}

pub fn lo(): int {
    return minInt
}

pub fn arith(): int {
    return maxInt - 1
}"#,
    ));
}

#[test]
fn a_bound_is_a_constant_initializer() {
    // The prime use: a `const` capacity bound, which the beta const folder must admit
    // even though it otherwise restricts a constant to a scalar literal.
    compile_ok(&wrap(
        r#"const CAP = maxInt
const FLOOR: int = minInt

pub fn cap(): int {
    return CAP
}"#,
    ));
}

#[test]
fn a_wrong_typed_annotation_on_a_bound_constant_is_rejected() {
    let diagnostics = compile_err(&wrap("const CAP: bool = maxInt\n"));
    assert!(
        has_code(&diagnostics, Code::CheckType),
        "a bool-annotated int bound must be a type error: {diagnostics:#?}"
    );
}

#[test]
fn a_bound_name_cannot_be_redeclared() {
    // Reserved like the rest of the built-in value family: a colliding `const`,
    // parameter, or `fn` is rejected rather than silently shadowing the bound.
    for source in [
        wrap("const maxInt = 5\n"),
        wrap("pub fn f(minInt: int): int {\n    return minInt\n}\n"),
        wrap("pub fn maxInt(): int {\n    return 1\n}\n"),
    ] {
        let diagnostics = compile_err(&source);
        assert!(
            has_code(&diagnostics, Code::CheckNameConflict),
            "a declaration colliding with a bound must be a name conflict: {source}\n{diagnostics:#?}"
        );
    }
}

#[test]
fn a_bound_has_no_call_form() {
    let diagnostics = compile_err(&wrap("pub fn f(): int {\n    return maxInt(1)\n}\n"));
    assert!(
        has_code(&diagnostics, Code::CheckType),
        "calling a value bound is a type error: {diagnostics:#?}"
    );
}
