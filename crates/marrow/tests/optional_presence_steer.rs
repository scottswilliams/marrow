//! The optional-vs-present misuse family steers to the presence idiom (DX06 item 3).
//!
//! When an optional value `T?` is used where the present `T` is required — returned or
//! passed where a bare value is wanted, or combined under an operator that has no optional
//! form — the `check.type` diagnostic names the two presence idioms (bind with `if const`,
//! or supply a `??` fallback) rather than only reporting the type clash. The code and the
//! span at the misuse are the contract; the steer substring is asserted because it is the
//! actionable payload the M3 actionability standard scores, not prose style. A genuine
//! kind mismatch that has nothing to do with optionality carries no such steer.

mod common;

use common::Project;

/// The steer sentence the family appends. Asserted as the load-bearing payload: it names
/// the `if const` binding and the `??` fallback, the two ways to make a value present.
const STEER: &str = "This value is optional; prove it present by binding it with `if const";

fn only_type_message(source: &str) -> String {
    let diags = Project::single(source)
        .try_image()
        .expect_err("the misuse must fail the check");
    diags.only("check.type").message.clone()
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
    let message = only_type_message(source);
    assert!(
        message.contains("string?") && message.contains(STEER),
        "the return mismatch steers to the presence idiom: {message:?}",
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
    let message = only_type_message(source);
    assert!(
        message.contains("int?") && message.contains(STEER),
        "the arithmetic mismatch steers to the presence idiom: {message:?}",
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
    let message = only_type_message(source);
    assert!(
        message.contains(STEER),
        "the argument mismatch steers to the presence idiom: {message:?}",
    );
}

/// A kind mismatch unrelated to optionality carries no presence steer: the steer is
/// specific to the optional-vs-present family and does not leak onto every type error.
#[test]
fn an_unrelated_type_mismatch_carries_no_presence_steer() {
    let source = r#"pub fn main(): int {
    return "text"
}
"#;
    let message = only_type_message(source);
    assert!(
        !message.contains(STEER),
        "a plain string-vs-int mismatch is not a presence misuse: {message:?}",
    );
}
