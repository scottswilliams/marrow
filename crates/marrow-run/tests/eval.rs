//! Evaluate pure scalar functions: arithmetic, comparison, logical operators,
//! locals, and conditionals over integer and boolean values.

use marrow_run::{
    RUN_DIVIDE_BY_ZERO, RUN_OVERFLOW, RUN_TYPE, RUN_UNBOUND_NAME, RUN_UNSUPPORTED, Value,
    evaluate_function,
};
use marrow_syntax::{Declaration, FunctionDecl, parse_source};

/// Parse `source` and return the single function it declares.
fn function(source: &str) -> FunctionDecl {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .expect("a function declaration")
}

#[test]
fn evaluates_arithmetic_over_parameters() {
    let add = function("fn add(a: int, b: int): int\n    return a + b\n");
    assert_eq!(
        evaluate_function(&add, &[Value::Int(2), Value::Int(40)]),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn respects_arithmetic_precedence() {
    // 2 + 3 * 4 == 14, not 20.
    let f = function("fn f(): int\n    return 2 + 3 * 4\n");
    assert_eq!(evaluate_function(&f, &[]), Ok(Some(Value::Int(14))));
}

#[test]
fn evaluates_conditionals() {
    let max =
        function("fn max(a: int, b: int): int\n    if a > b\n        return a\n    return b\n");
    assert_eq!(
        evaluate_function(&max, &[Value::Int(7), Value::Int(3)]),
        Ok(Some(Value::Int(7)))
    );
    assert_eq!(
        evaluate_function(&max, &[Value::Int(3), Value::Int(7)]),
        Ok(Some(Value::Int(7)))
    );
}

#[test]
fn evaluates_locals_and_reassignment() {
    let f =
        function("fn f(n: int): int\n    var total = n\n    total = total + 1\n    return total\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Int(41)]),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn evaluates_boolean_logic() {
    let f = function("fn f(a: bool, b: bool): bool\n    return a and not b\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Bool(true), Value::Bool(false)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        evaluate_function(&f, &[Value::Bool(true), Value::Bool(true)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn equality_compares_values() {
    // Marrow spells equality `=` (and inequality `!=`); assignment `=` is a
    // statement, so this `=` in expression position is the equality operator.
    let f = function("fn f(a: int, b: int): bool\n    return a = b\n");
    assert_eq!(
        evaluate_function(&f, &[Value::Int(5), Value::Int(5)]),
        Ok(Some(Value::Bool(true)))
    );
    assert_eq!(
        evaluate_function(&f, &[Value::Int(5), Value::Int(6)]),
        Ok(Some(Value::Bool(false)))
    );
}

#[test]
fn a_function_that_returns_nothing_yields_none() {
    // Falls off the end with no `return`.
    let f = function("fn f(a: int)\n    let x = a + 1\n");
    assert_eq!(evaluate_function(&f, &[Value::Int(1)]), Ok(None));
}

#[test]
fn rejects_division_by_zero() {
    let f = function("fn f(a: int): int\n    return a / 0\n");
    let result = evaluate_function(&f, &[Value::Int(10)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_DIVIDE_BY_ZERO),
        "{result:?}"
    );
}

#[test]
fn detects_integer_overflow() {
    let f = function("fn f(a: int): int\n    return a * a\n");
    let result = evaluate_function(&f, &[Value::Int(i64::MAX)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}

#[test]
fn rejects_an_unbound_name() {
    let f = function("fn f(): int\n    return x\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNBOUND_NAME),
        "{result:?}"
    );
}

#[test]
fn rejects_assignment_to_an_immutable_binding() {
    let f = function("fn f(): int\n    let x = 1\n    x = 2\n    return x\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn rejects_an_argument_count_mismatch() {
    let add = function("fn add(a: int, b: int): int\n    return a + b\n");
    let result = evaluate_function(&add, &[Value::Int(1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn reports_an_unsupported_construct() {
    // A string value is not yet evaluable in this slice.
    let f = function("fn f(): string\n    return \"hi\"\n");
    let result = evaluate_function(&f, &[]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_UNSUPPORTED),
        "{result:?}"
    );
}

#[test]
fn an_if_condition_must_be_boolean() {
    let f = function("fn f(a: int): int\n    if a\n        return 1\n    return 0\n");
    let result = evaluate_function(&f, &[Value::Int(5)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_TYPE),
        "{result:?}"
    );
}

#[test]
fn an_inner_scope_shadows_then_restores_an_outer_binding() {
    // `let x = 1` inside the if-block shadows only within that block; after it,
    // the outer `x` (99) is what `return x` sees.
    let f = function("fn f(): int\n    let x = 99\n    if true\n        let x = 1\n    return x\n");
    assert_eq!(evaluate_function(&f, &[]), Ok(Some(Value::Int(99))));
}

#[test]
fn an_else_if_chain_selects_the_matching_branch() {
    let grade = function(
        "fn grade(n: int): int\n    if n > 90\n        return 1\n    else if n > 80\n        return 2\n    else\n        return 3\n",
    );
    assert_eq!(
        evaluate_function(&grade, &[Value::Int(95)]),
        Ok(Some(Value::Int(1)))
    );
    assert_eq!(
        evaluate_function(&grade, &[Value::Int(85)]),
        Ok(Some(Value::Int(2)))
    );
    assert_eq!(
        evaluate_function(&grade, &[Value::Int(50)]),
        Ok(Some(Value::Int(3)))
    );
}

#[test]
fn detects_min_over_negative_one_overflow() {
    let f = function("fn f(a: int, b: int): int\n    return a / b\n");
    let result = evaluate_function(&f, &[Value::Int(i64::MIN), Value::Int(-1)]);
    assert!(
        matches!(result, Err(ref error) if error.code == RUN_OVERFLOW),
        "{result:?}"
    );
}
