//! Fold a statically-known integer position out of a write target. A sequence
//! position can be a literal, a `const` binding, or integer arithmetic over either,
//! and the value of all three is determined at check, not run. This is the single
//! owner of that fold: it resolves a `const` name through the in-scope known-integer
//! environment and evaluates the integer operators, so a position that the spec can
//! prove non-positive is caught at check rather than faulting at run.
//!
//! A genuinely dynamic position — a parameter, a `var`, a function result — is not a
//! constant and folds to `None`, staying a catchable run fault. The fold never
//! reaches into runtime values.

use std::collections::HashMap;

use marrow_syntax::{BinaryOp, Expression, LiteralKind, UnaryOp};

/// The in-scope binding environment for the integer fold, frame by frame mirroring
/// the type scope. Each frame maps a bound name to `Some(value)` when it is a `const`
/// that folds to a known integer, or `None` when it is any other binding — a `var`, a
/// parameter, a loop or catch variable. A nearer `None` masks a like-named outer
/// constant, so a binding that shadows a constant is correctly dynamic and does not
/// fold to the shadowed value.
pub(crate) type ConstIntScope = Vec<HashMap<String, Option<i64>>>;

/// The integer a constant expression denotes, or `None` when any leaf is not a
/// statically-known integer. Supports integer literals, negation, the integer
/// arithmetic operators, and a single-segment name bound to a known integer
/// constant. A name shadowed by a non-constant binding, or one that is not a known
/// constant, folds to `None`. A division or remainder by zero folds to `None`: the
/// value is not statically determined, so it is left to the run-time fault.
pub(crate) fn fold_const_int(
    expr: &Expression,
    scope: &[HashMap<String, Option<i64>>],
) -> Option<i64> {
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => text.parse().ok(),
        Expression::Name { segments, .. } => {
            let [name] = segments.as_slice() else {
                return None;
            };
            scope
                .iter()
                .rev()
                .find_map(|frame| frame.get(name))
                .copied()
                .flatten()
        }
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => fold_const_int(operand, scope)?.checked_neg(),
        Expression::Binary {
            op, left, right, ..
        } => {
            let left = fold_const_int(left, scope)?;
            let right = fold_const_int(right, scope)?;
            match op {
                BinaryOp::Add => left.checked_add(right),
                BinaryOp::Subtract => left.checked_sub(right),
                BinaryOp::Multiply => left.checked_mul(right),
                BinaryOp::Divide => left.checked_div(right),
                BinaryOp::Remainder => left.checked_rem(right),
                _ => None,
            }
        }
        _ => None,
    }
}
