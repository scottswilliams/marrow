use marrow_syntax::{Expression, InterpolationPart, SourceSpan};

/// Apply `visit` to each immediate sub-expression of `expr`, in source order. This
/// is the single owner of the expression-tree shape for the checker's read-only
/// passes: a pass inspects a node, then recurses by handing its children back here,
/// so the per-variant child enumeration is written once. Statements and binding
/// scope are not the concern of this helper — passes that thread scope or descend
/// statement bodies own that structure themselves.
pub(crate) fn for_each_child_expr<'e>(expr: &'e Expression, mut visit: impl FnMut(&'e Expression)) {
    match expr {
        Expression::Call { callee, args, .. } => {
            visit(callee);
            for arg in args {
                visit(&arg.value);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => visit(base),
        Expression::Unary { operand, .. } => visit(operand),
        Expression::Binary { left, right, .. } => {
            visit(left);
            visit(right);
        }
        Expression::Range {
            start, end, step, ..
        } => {
            if let Some(start) = start {
                visit(start);
            }
            if let Some(end) = end {
                visit(end);
            }
            if let Some(step) = step {
                visit(step);
            }
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(inner) = part {
                    visit(inner);
                }
            }
        }
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. }
        | Expression::Error { .. } => {}
    }
}

/// Apply `visit` to the `(root, span)` of every `^root` in the expression tree
/// rooted at `expr`. A saved root is the sole way a saved address is spelled, so a
/// pass validating declared roots visits them all through this one recursion.
pub(crate) fn for_each_saved_root(expr: &Expression, visit: &mut impl FnMut(&str, SourceSpan)) {
    if let Expression::SavedRoot { name, span } = expr {
        visit(name, *span);
    }
    for_each_child_expr(expr, |child| for_each_saved_root(child, visit));
}
