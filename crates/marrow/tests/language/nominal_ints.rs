//! Nominal ints through the production path: `int(a)` yields the underlying `int`,
//! `string(a)` renders it, and `add` and `subtract` are the whole capability set.

use crate::common::Project;
use marrow_vm::Value;

#[test]
fn int_and_string_read_a_nominal_value() {
    let mut session = Project::single(
        r#"type Age: int in 0..=150 supports add

pub fn base(): int {
    const a = Age(41) + 1
    return int(a)
}

pub fn rendered(): string {
    return string(Age(7))
}
"#,
    )
    .session();
    assert_eq!(session.call("base", vec![]), Some(Value::Int(42)));
    assert_eq!(
        session.call("rendered", vec![]),
        Some(Value::Text("7".into()))
    );
}

/// No capability admits a product: `Name * int` is an operator defined for no pair of
/// these operand types.
#[test]
fn a_nominal_product_is_a_check_type_diagnostic() {
    let diagnostics = Project::single(
        "type Age: int in 0..=150 supports add, subtract\n\n\
         pub fn f(): int {\n\
         \x20   const a = Age(2) * 3\n\
         \x20   return 0\n\
         }\n",
    )
    .try_image()
    .expect_err("a product is refused");
    let row = diagnostics.only("check.type");
    assert_eq!(row.line(), 4, "{:?}", diagnostics.all());
}
