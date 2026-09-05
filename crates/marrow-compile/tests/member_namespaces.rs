//! Every namespace of declared members refuses a repeated name at the repeat.
//!
//! One conflict owner serves struct fields, enum members and payload fields, type
//! parameters, function parameters, key columns, and every member layer of a
//! resource. Each fixture below repeats one name in one namespace and asserts the
//! whole diagnostic list: exactly one `check.name_conflict`, at the line and column
//! of the repeated name token, and nothing else. A repeat that reached the verifier
//! (a span-less `image.table`) or executed (`f(1, 2)` answering `2`) would surface
//! here as a compile that succeeded.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::ProjectInput;

#[path = "common/ids.rs"]
mod ids;
#[path = "common/project.rs"]
mod project_capture;

/// The rows a project refuses with, as `(code, line, column)`.
fn refused(input: &ProjectInput) -> Vec<(String, u32, u32)> {
    let diagnostics: Vec<SourceDiagnostic> = match compile(input) {
        Ok(_) => panic!("the repeated name compiled: the namespace has no conflict owner"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    };
    diagnostics
        .iter()
        .map(|row| (row.code().to_string(), row.span().line, row.span().column))
        .collect()
}

fn storeless(body: &str) -> ProjectInput {
    let source = format!("module main\n\n{body}");
    project_capture::project(&[("src/main.mw", source.as_str())])
}

/// A durable fixture under a complete identity ledger, so the only refusal left is
/// the repeated name.
fn durable(body: &str) -> ProjectInput {
    let source = format!("module main\n\n{body}");
    ids::minted(|ledger| {
        project_capture::project_with_ids(&[("src/main.mw", source.as_str())], ledger)
    })
}

const MAIN: &str = "pub fn main() {\n    return\n}\n";

fn assert_one_conflict(input: &ProjectInput, line: u32, column: u32, what: &str) {
    assert_eq!(
        refused(input),
        vec![("check.name_conflict".to_string(), line, column)],
        "{what}: one `check.name_conflict` at the repeated name, and no other row",
    );
}

#[test]
fn a_struct_field_declared_twice() {
    let source = format!("struct P {{\n    x: int\n    x: int\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 5, 5, "struct field");
}

#[test]
fn a_generic_struct_field_declared_twice() {
    let source = format!("struct Box<T> {{\n    v: T\n    v: T\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 5, 5, "generic struct field");
}

#[test]
fn an_enum_member_declared_twice() {
    let source = format!("enum E {{\n    a\n    a\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 5, 5, "enum member");
}

#[test]
fn a_generic_enum_member_declared_twice() {
    let source = format!("enum E<T> {{\n    a(v: T)\n    a(v: T)\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 5, 5, "generic enum member");
}

#[test]
fn an_enum_payload_field_declared_twice() {
    let source = format!("enum E {{\n    r(w: int, w: int)\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 4, 15, "enum payload field");
}

#[test]
fn a_generic_enum_payload_field_declared_twice() {
    let source = format!("enum E<T> {{\n    r(w: T, w: T)\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 4, 13, "generic enum payload field");
}

#[test]
fn a_function_type_parameter_declared_twice() {
    let source = format!("fn f<T, T>(a: T): T {{\n    return a\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 3, 9, "function type parameter");
}

#[test]
fn a_struct_type_parameter_declared_twice() {
    let source = format!("struct S<T, T> {{\n    a: T\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 3, 13, "struct type parameter");
}

#[test]
fn an_enum_type_parameter_declared_twice() {
    let source = format!("enum E<T, T> {{\n    a(v: T)\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 3, 11, "enum type parameter");
}

#[test]
fn a_function_parameter_declared_twice() {
    let source = format!("fn f(a: int, a: int): int {{\n    return a\n}}\n{MAIN}");
    assert_one_conflict(&storeless(&source), 3, 14, "function parameter");
}

/// The executable shape: a second `a` used to take the second slot, so `f(1, 2)`
/// answered `2` for a body that wrote one parameter name.
#[test]
fn a_called_function_with_a_repeated_parameter_never_compiles() {
    let source = "fn f(a: int, a: int): int {\n    return a\n}\n\
                  pub fn main(): int {\n    return f(1, 2)\n}\n";
    assert_one_conflict(&storeless(source), 3, 14, "called function parameter");
}

#[test]
fn a_root_key_column_declared_twice() {
    let source = format!("resource R {{\n    n: int\n}}\nstore ^r[id: int, id: int]: R\n{MAIN}");
    assert_one_conflict(&durable(&source), 6, 19, "root key column");
}

#[test]
fn a_resource_field_declared_twice() {
    let source = format!("resource R {{\n    n: int\n    n: int\n}}\nstore ^r[id: int]: R\n{MAIN}");
    assert_one_conflict(&durable(&source), 5, 5, "resource field");
}

#[test]
fn a_group_leaf_declared_twice() {
    let source = format!(
        "resource R {{\n    g {{\n        a: int\n        a: int\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 6, 9, "group leaf");
}

#[test]
fn a_group_repeating_a_field_name() {
    let source = format!(
        "resource R {{\n    a: int\n    a {{\n        b: int\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 5, 5, "group versus field");
}

#[test]
fn a_branch_declared_twice() {
    let source = format!(
        "resource R {{\n    b[k: int] {{\n        x: int\n    }}\n    b[k: int] {{\n        y: int\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 7, 5, "branch name");
}

#[test]
fn a_branch_key_column_declared_twice() {
    let source = format!(
        "resource R {{\n    b[k: int, k: int] {{\n        x: int\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 4, 15, "branch key column");
}

#[test]
fn a_branch_member_declared_twice() {
    let source = format!(
        "resource R {{\n    b[k: int] {{\n        x: int\n        x: int\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 6, 9, "branch member");
}

#[test]
fn a_root_key_repeating_a_member_name() {
    let source = format!("resource R {{\n    id: int\n}}\nstore ^r[id: int]: R\n{MAIN}");
    assert_one_conflict(&durable(&source), 6, 10, "root key versus member");
}

#[test]
fn a_branch_member_repeating_a_key_name() {
    let source = format!(
        "resource R {{\n    b[k: int] {{\n        k: int\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 5, 9, "branch key versus member");
}

#[test]
fn a_nested_branch_declared_twice() {
    let source = format!(
        "resource R {{\n    b[k: int] {{\n        c[j: int] {{\n            x: int\n        }}\n\
         \x20       c[j: int] {{\n            x: int\n        }}\n    }}\n}}\n\
         store ^r[id: int]: R\n{MAIN}"
    );
    assert_one_conflict(&durable(&source), 8, 9, "nested branch name");
}
