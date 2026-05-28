//! Render the syntax tree back to canonical Marrow `.mw` source.
//!
//! Canonical style follows `docs/language/`: binary operators are spaced
//! (`a + b`), ranges are not (`1..10`), unary is `-x` / `not x`, calls are
//! `f(a, b)`, dotted fields and `::` name paths have no surrounding spaces.
//! The syntax tree does not record parentheses, so the formatter re-inserts
//! the minimum needed to preserve operator precedence and associativity.

use crate::{ArgMode, Argument, BinaryOp, Expression, InterpolationPart, UnaryOp};

/// Precedence of an expression, tightest-binding last. Used to decide where
/// parentheses are required. Atoms (literals, names, calls, fields, …) bind
/// tightest; `or` binds loosest.
const PREC_ATOM: u8 = 10;
const PREC_UNARY: u8 = 9;

/// Format a single expression as canonical Marrow source.
pub fn format_expression(expression: &Expression) -> String {
    match expression {
        Expression::Literal { text, .. } => text.clone(),
        Expression::Name { segments, .. } => segments.join("::"),
        Expression::SavedRoot { name, .. } => format!("^{name}"),
        Expression::Call { callee, args, .. } => {
            let args = args
                .iter()
                .map(format_argument)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", format_child(callee, PREC_ATOM), args)
        }
        Expression::Field {
            base, name, quoted, ..
        } => {
            let segment = if *quoted {
                format!("\"{name}\"")
            } else {
                name.clone()
            };
            format!("{}.{}", format_child(base, PREC_ATOM), segment)
        }
        Expression::Unary { op, operand, .. } => {
            let operand = format_child(operand, PREC_UNARY);
            match op {
                UnaryOp::Neg => format!("-{operand}"),
                UnaryOp::Not => format!("not {operand}"),
            }
        }
        Expression::Binary {
            op, left, right, ..
        } => format_binary(*op, left, right),
        Expression::Interpolation { parts, .. } => format_interpolation(parts),
        Expression::Unparsed { text, .. } => text.clone(),
    }
}

fn format_binary(op: BinaryOp, left: &Expression, right: &Expression) -> String {
    let precedence = binary_precedence(op);
    // Left-associative operators keep an equal-precedence left operand without
    // parentheses; the right operand needs them. Non-associative operators
    // (equality, comparison, range) require parentheses on either equal side.
    let (left_min, right_min) = if is_left_associative(op) {
        (precedence, precedence + 1)
    } else {
        (precedence + 1, precedence + 1)
    };
    let left = format_child(left, left_min);
    let right = format_child(right, right_min);
    match op {
        BinaryOp::RangeExclusive => format!("{left}..{right}"),
        BinaryOp::RangeInclusive => format!("{left}..={right}"),
        _ => format!("{left} {} {right}", binary_symbol(op)),
    }
}

/// Format `child`, wrapping it in parentheses when it binds looser than
/// `min_precedence` requires.
fn format_child(child: &Expression, min_precedence: u8) -> String {
    let rendered = format_expression(child);
    if precedence(child) < min_precedence {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn format_argument(argument: &Argument) -> String {
    let mut out = String::new();
    match argument.mode {
        Some(ArgMode::Out) => out.push_str("out "),
        Some(ArgMode::InOut) => out.push_str("inout "),
        None => {}
    }
    if let Some(name) = &argument.name {
        out.push_str(name);
        out.push_str(": ");
    }
    out.push_str(&format_expression(&argument.value));
    out
}

fn format_interpolation(parts: &[InterpolationPart]) -> String {
    let mut out = String::from("$\"");
    for part in parts {
        match part {
            // Text keeps `{{`/`}}` escaped exactly as written.
            InterpolationPart::Text { text, .. } => out.push_str(text),
            InterpolationPart::Expr(expression) => {
                out.push('{');
                out.push_str(&format_expression(expression));
                out.push('}');
            }
        }
    }
    out.push('"');
    out
}

fn precedence(expression: &Expression) -> u8 {
    match expression {
        Expression::Binary { op, .. } => binary_precedence(*op),
        Expression::Unary { .. } => PREC_UNARY,
        _ => PREC_ATOM,
    }
}

fn binary_precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Equal | BinaryOp::NotEqual => 3,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => 4,
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => 5,
        BinaryOp::Concat => 6,
        BinaryOp::Add | BinaryOp::Subtract => 7,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => 8,
    }
}

/// Equality, comparison, and range are non-associative per the grammar; all
/// other binary operators are left-associative.
fn is_left_associative(op: BinaryOp) -> bool {
    !matches!(
        op,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
            | BinaryOp::RangeExclusive
            | BinaryOp::RangeInclusive
    )
}

fn binary_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Concat => "_",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "=",
        BinaryOp::NotEqual => "!=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        // Ranges are emitted without spaces by `format_binary`.
        BinaryOp::RangeExclusive => "..",
        BinaryOp::RangeInclusive => "..=",
    }
}
