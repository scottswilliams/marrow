//! The optional-vs-present misuse family steers to the presence idiom.
//!
//! When an optional value `T?` is used where the present `T` is required — returned or
//! passed where a bare value is wanted, or combined under an operator that has no optional
//! form — the `check.type` diagnostic carries [`Steer::Presence`], which names the two
//! presence idioms (bind with `if const`, or supply a `??` fallback) rather than only
//! reporting the type clash. The typed steer and the typed mismatch are the contract; the
//! sentence they render is not. A genuine kind mismatch that has nothing to do with
//! optionality carries no such steer.

use crate::common::Project;
use marrow_compile::{SourceDiagnostic, Steer, TypeMismatch};

fn only_type_diagnostic(source: &str) -> SourceDiagnostic {
    let diags = Project::single(source)
        .try_image()
        .expect_err("the misuse must fail the check");
    diags.only("check.type").clone()
}

/// Whether the sole `check.type` row steers to the presence idiom.
fn steers_to_presence(source: &str) -> bool {
    only_type_diagnostic(source).steer() == Some(&Steer::Presence)
}

/// Returning an optional where the signature promises the present `T` is the misuse, and
/// the type-mismatch diagnostic steers to the idiom.
#[test]
fn returning_an_optional_where_present_is_required_steers() {
    let source = r#"pub fn subtitleOf(): string {
    var maybe: string? = "x"
    return maybe
}
"#;
    let diagnostic = only_type_diagnostic(source);
    let Some(TypeMismatch::Value { found, expected }) = diagnostic.type_mismatch() else {
        panic!("the return misuse is a value mismatch: {diagnostic:?}");
    };
    assert_eq!((found.as_str(), expected.as_str()), ("string?", "string"));
    assert_eq!(
        diagnostic.steer(),
        Some(&Steer::Presence),
        "the return mismatch steers to the presence idiom: {diagnostic:?}",
    );
}

/// An optional operand under an arithmetic operator has no present form; the binary
/// diagnostic steers to making the value present first.
#[test]
fn an_optional_operand_in_arithmetic_steers() {
    let source = r#"pub fn pagesPlusOne(): int {
    var pages: int? = 3
    return pages + 1
}
"#;
    let diagnostic = only_type_diagnostic(source);
    let Some(TypeMismatch::Binary { left, right, .. }) = diagnostic.type_mismatch() else {
        panic!("the arithmetic misuse is a binary mismatch: {diagnostic:?}");
    };
    assert_eq!((left.as_str(), right.as_str()), ("int?", "int"));
    assert_eq!(
        diagnostic.steer(),
        Some(&Steer::Presence),
        "the arithmetic mismatch steers to the presence idiom: {diagnostic:?}",
    );
}

/// A local optional passed where a bare parameter is required steers as well — the family
/// is the whole optional-where-present surface, not one durable case.
#[test]
fn passing_a_local_optional_where_bare_is_required_steers() {
    let source = r#"fn takesInt(n: int): int {
    return n
}

pub fn main(): int {
    var maybe: int? = 3
    return takesInt(maybe)
}
"#;
    assert!(
        steers_to_presence(source),
        "the argument mismatch steers to the presence idiom",
    );
}

/// A `bool?` operand under `and` steers — presence is the sole blocker.
#[test]
fn a_bool_optional_logic_operand_steers() {
    let source = r#"pub fn main(a: bool): bool {
    var maybe: bool? = true
    return maybe and a
}
"#;
    assert!(steers_to_presence(source), "a `bool?` logic operand steers");
}

/// A kind mismatch unrelated to optionality carries no presence steer: the steer is
/// specific to the optional-vs-present family and does not leak onto every type error.
#[test]
fn an_unrelated_type_mismatch_carries_no_presence_steer() {
    let source = r#"pub fn main(): int {
    return "text"
}
"#;
    assert!(
        !steers_to_presence(source),
        "a plain string-vs-int mismatch is not a presence misuse",
    );
}

/// An optional operand whose bare type still would not satisfy the operator is not
/// presence-fixable, so it carries no steer — the tightened family does not mislead.
#[test]
fn a_non_presence_fixable_optional_operand_carries_no_steer() {
    // `int? + string`: bare types differ, so making the optional present still fails.
    let mixed = r#"pub fn main(): int {
    var n: int? = 1
    return n + "x"
}
"#;
    assert!(
        !steers_to_presence(mixed),
        "a cross-type optional operand is not presence-fixable",
    );
    // `not (int?)`: the bare type is `int`, which `not` still rejects.
    let not_int = r#"pub fn main(): bool {
    var n: int? = 1
    return not n
}
"#;
    assert!(
        !steers_to_presence(not_int),
        "an optional whose bare type the unary op still rejects is not presence-fixable",
    );
    // `int? and bool`: `and` wants bool, and the bare type is `int`.
    let non_bool_logic = r#"pub fn main(a: bool): bool {
    var n: int? = 1
    return n and a
}
"#;
    assert!(
        !steers_to_presence(non_bool_logic),
        "a non-bool optional logic operand is not presence-fixable",
    );
}
