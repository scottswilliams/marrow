//! The single owner of statically-known integer folding at check. A sequence
//! position or a `const` initializer can be a literal, a `const` binding, or integer
//! arithmetic over either, and the value of all three is determined at check, not run.
//! `eval_const_int` resolves a `const` name through the in-scope known-integer
//! environment, evaluates the integer operators, and distinguishes an arithmetic
//! `i64` overflow from a genuinely dynamic value: a position the spec can prove
//! non-positive is caught at check rather than faulting at run, and
//! `check_const_int_overflow` rejects a constant whose arithmetic exceeds the range —
//! like the value-equal literal — at the same phase. `fold_const_int` is the
//! `Option`-valued view of the same evaluation for callers that need only the value.
//!
//! A genuinely dynamic value — a parameter, a `var`, a function result — is not a
//! constant and folds to `None`/`NotConstant`, staying a catchable run fault. The
//! fold never reaches into runtime values.

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::{BinaryOp, Expression, LiteralKind, UnaryOp};

use crate::program::CheckedConst;
use crate::typerules::{LiteralSign, negated_integer_literal, parse_integer_literal};
use crate::{CHECK_LITERAL_RANGE, CheckDiagnostic};

/// The in-scope binding environment for the integer fold, frame by frame mirroring
/// the type scope. Each frame maps a bound name to `Some(value)` when it is a `const`
/// that folds to a known integer, or `None` when it is any other binding — a `var`, a
/// parameter, a loop or catch variable. A nearer `None` masks a like-named outer
/// constant, so a binding that shadows a constant is correctly dynamic and does not
/// fold to the shadowed value.
pub(crate) type ConstIntScope = Vec<HashMap<String, Option<i64>>>;

/// A module's `const` declarations folded to their known integer values, in
/// declaration order so a later constant defined over an earlier one resolves. A
/// constant whose value is not a statically-known integer maps to `None`, masking
/// any like-named scope so it stays dynamic.
pub(crate) fn module_const_int_scope(constants: &[CheckedConst]) -> HashMap<String, Option<i64>> {
    let mut scope = HashMap::new();
    for constant in constants {
        let value = constant
            .value
            .as_ref()
            .and_then(|value| fold_const_int(value, std::slice::from_ref(&scope)));
        scope.insert(constant.name.clone(), value);
    }
    scope
}

/// The integer a constant expression denotes, or `None` when any leaf is not a
/// statically-known integer. A division or remainder by a zero constant, and an
/// arithmetic overflow, both fold to `None`: the value is not a statically-known
/// `i64`, so it is left to the run-time fault. Use [`eval_const_int`] when the
/// overflow must be distinguished from a genuinely dynamic value.
pub(crate) fn fold_const_int(
    expr: &Expression,
    scope: &[HashMap<String, Option<i64>>],
) -> Option<i64> {
    match eval_const_int(expr, scope) {
        ConstIntEval::Value(value) => Some(value),
        ConstIntEval::Overflow | ConstIntEval::NotConstant => None,
    }
}

/// The verdict for evaluating a constant integer expression: a value, an `i64`
/// overflow produced by fully-constant arithmetic over in-range operands, or not
/// statically known. An out-of-range integer literal leaf is `NotConstant`, not
/// `Overflow`: that leaf is already range-checked on its own, so reserving `Overflow`
/// for the arithmetic result keeps the verdicts from double-reporting the same value.
pub(crate) enum ConstIntEval {
    Value(i64),
    Overflow,
    NotConstant,
}

/// Evaluate a constant integer expression, distinguishing an arithmetic `i64`
/// overflow from a genuinely dynamic value. A `const` initializer is a compile-time
/// constant expression, so arithmetic that overflows is out of range at check just
/// like the value-equal literal. Integer literals, negation, the integer arithmetic
/// operators, and a single-segment name bound to a known integer constant take part;
/// any other leaf, or a name that is shadowed or not a known constant, is
/// `NotConstant` and left to the runtime.
pub(crate) fn eval_const_int(
    expr: &Expression,
    scope: &[HashMap<String, Option<i64>>],
) -> ConstIntEval {
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => parse_integer_literal(text, LiteralSign::Bare)
            .map_or(ConstIntEval::NotConstant, ConstIntEval::Value),
        Expression::Name { segments, .. } => match segments.as_slice() {
            [name] => scope
                .iter()
                .rev()
                .find_map(|frame| frame.get(name))
                .copied()
                .flatten()
                .map_or(ConstIntEval::NotConstant, ConstIntEval::Value),
            _ => ConstIntEval::NotConstant,
        },
        Expression::Unary {
            op: op @ UnaryOp::Neg,
            operand,
            ..
        } => match negated_integer_literal(*op, operand) {
            Some((text, _)) => parse_integer_literal(text, LiteralSign::Negated)
                .map_or(ConstIntEval::NotConstant, ConstIntEval::Value),
            None => match eval_const_int(operand, scope) {
                ConstIntEval::Value(value) => value
                    .checked_neg()
                    .map_or(ConstIntEval::Overflow, ConstIntEval::Value),
                other => other,
            },
        },
        Expression::Binary {
            op, left, right, ..
        } => match (eval_const_int(left, scope), eval_const_int(right, scope)) {
            (ConstIntEval::Overflow, _) | (_, ConstIntEval::Overflow) => ConstIntEval::Overflow,
            (ConstIntEval::Value(left), ConstIntEval::Value(right)) => {
                match integer_binary_op(*op, left, right) {
                    Some(value) => ConstIntEval::Value(value),
                    // A division or remainder by a zero constant is left to the
                    // runtime fault, as is a non-arithmetic operator; only a known
                    // arithmetic pair that overflows is an overflow.
                    None if is_integer_arithmetic(*op) && right != 0 => ConstIntEval::Overflow,
                    None => ConstIntEval::NotConstant,
                }
            }
            _ => ConstIntEval::NotConstant,
        },
        _ => ConstIntEval::NotConstant,
    }
}

fn integer_binary_op(op: BinaryOp, left: i64, right: i64) -> Option<i64> {
    match op {
        BinaryOp::Add => left.checked_add(right),
        BinaryOp::Subtract => left.checked_sub(right),
        BinaryOp::Multiply => left.checked_mul(right),
        BinaryOp::Divide => left.checked_div(right),
        BinaryOp::Remainder => left.checked_rem(right),
        _ => None,
    }
}

fn is_integer_arithmetic(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Remainder
    )
}

/// Report a `const` value whose constant integer arithmetic overflows `i64`. An
/// out-of-range literal leaf is range-checked on its own, so this fires only for an
/// overflow produced by the arithmetic itself, keeping the two checks from
/// double-reporting the same value.
pub(crate) fn check_const_int_overflow(
    file: &Path,
    value: &Expression,
    scope: &[HashMap<String, Option<i64>>],
    out: &mut Vec<CheckDiagnostic>,
) {
    if let ConstIntEval::Overflow = eval_const_int(value, scope) {
        out.push(CheckDiagnostic::error(
            CHECK_LITERAL_RANGE,
            file,
            value.span(),
            "int value is out of range",
        ));
    }
}
