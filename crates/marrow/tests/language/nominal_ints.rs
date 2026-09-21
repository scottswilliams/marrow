//! Nominal ints through the production path: `int(a)` yields the underlying `int`,
//! `string(a)` renders it, and `add` and `subtract` are the whole capability set.

use crate::common::Project;
use marrow_vm::Value;

/// `int(a)` and `string(a)` read a nominal value under every capability set: they
/// are conversions, not operators, so no capability admits or withholds them.
#[test]
fn int_and_string_read_a_nominal_value_under_every_capability_set() {
    for supports in [
        "",
        " supports add",
        " supports subtract",
        " supports add, subtract",
    ] {
        let mut session = Project::single(&format!(
            "type Age: int in 0..=150{supports}\n\n\
             pub fn base(): int {{\n\
             \x20   return int(Age(41))\n\
             }}\n\n\
             pub fn rendered(): string {{\n\
             \x20   return string(Age(7))\n\
             }}\n"
        ))
        .session();
        assert_eq!(
            session.call("base", vec![]),
            Some(Value::Int(41)),
            "{supports}"
        );
        assert_eq!(
            session.call("rendered", vec![]),
            Some(Value::Text("7".into())),
            "{supports}"
        );
    }
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
