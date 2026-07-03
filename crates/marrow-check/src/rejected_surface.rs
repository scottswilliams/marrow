use std::path::Path;

use marrow_syntax::{Block, Declaration, EvolveStep, Expression, SourceSpan, Statement};

use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::walk::for_each_child_expr;
use crate::{
    CHECK_REJECTED_SURFACE, CheckDiagnostic, CheckedProgram, DiagnosticPayload, RejectedSurface,
};

pub(crate) fn check_rejected_surface(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for declaration in &parsed.file.declarations {
        match declaration {
            Declaration::Resource(_) | Declaration::Store(_) | Declaration::Surface(_) => {}
            Declaration::Function(function) => {
                check_block(program, file, &function.body, diagnostics);
            }
            Declaration::Const(constant) => {
                if let Some(value) = &constant.value {
                    check_expr(program, file, value, diagnostics);
                }
            }
            Declaration::Enum(_) => {}
            Declaration::Evolve(evolve) => {
                for step in &evolve.steps {
                    if let EvolveStep::Transform { body, .. } = step {
                        check_block(program, file, body, diagnostics);
                    }
                }
            }
        }
    }
}

fn check_block(
    program: &CheckedProgram,
    file: &Path,
    block: &Block,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        check_statement(program, file, statement, diagnostics);
    }
}

fn check_statement(
    program: &CheckedProgram,
    file: &Path,
    statement: &Statement,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        Statement::Const { value, .. } | Statement::Throw { value, .. } => {
            check_expr(program, file, value, diagnostics);
        }
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                check_expr(program, file, value, diagnostics);
            }
        }
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            check_expr(program, file, target, diagnostics);
            check_expr(program, file, value, diagnostics);
        }
        Statement::Delete { path, .. } => {
            check_expr(program, file, path, diagnostics);
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                check_expr(program, file, value, diagnostics);
            }
        }
        Statement::Expr { value, .. } => {
            check_expr(program, file, value, diagnostics);
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                check_expr(program, file, condition, diagnostics);
            }
            check_block(program, file, then_block, diagnostics);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    check_expr(program, file, condition, diagnostics);
                }
                check_block(program, file, &else_if.block, diagnostics);
            }
            if let Some(block) = else_block {
                check_block(program, file, block, diagnostics);
            }
        }
        Statement::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            check_expr(program, file, value, diagnostics);
            check_block(program, file, then_block, diagnostics);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    check_expr(program, file, condition, diagnostics);
                }
                check_block(program, file, &else_if.block, diagnostics);
            }
            if let Some(block) = else_block {
                check_block(program, file, block, diagnostics);
            }
        }
        Statement::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                check_expr(program, file, condition, diagnostics);
            }
            check_block(program, file, body, diagnostics);
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => {
            check_expr(program, file, iterable, diagnostics);
            if let Some(step) = step {
                check_expr(program, file, step, diagnostics);
            }
            check_block(program, file, body, diagnostics);
        }
        Statement::Transaction { body, .. } => {
            check_block(program, file, body, diagnostics);
        }
        Statement::Try { body, catch, .. } => {
            check_block(program, file, body, diagnostics);
            if let Some(catch) = catch {
                check_block(program, file, &catch.block, diagnostics);
            }
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                check_expr(program, file, scrutinee, diagnostics);
            }
            for arm in arms {
                check_block(program, file, &arm.block, diagnostics);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
    }
}

fn check_expr(
    program: &CheckedProgram,
    file: &Path,
    expr: &Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    // Rejected-surface diagnostics must land in source position: the traversal-method
    // push sits at the call span, then the callee subtree is walked, then each
    // argument is flagged and recursed in turn. Non-call expressions carry no rejected
    // call surface and defer their child shape to the shared visitor.
    if let Expression::Call {
        callee, args, span, ..
    } = expr
    {
        if let Some(method) = rejected_traversal_method(program, file, callee) {
            push(
                file,
                *span,
                &format!(
                    "saved traversal method `.{method}(...)` is not a v0.1 source surface; stream durable iterables with ordinary `for` loops"
                ),
                DiagnosticPayload::RejectedSurface(RejectedSurface::SavedTraversalMethod {
                    method: method.to_string(),
                }),
                diagnostics,
            );
        }
        check_expr(program, file, callee, diagnostics);
        for arg in args {
            check_expr(program, file, &arg.value, diagnostics);
        }
    } else {
        for_each_child_expr(expr, |child| check_expr(program, file, child, diagnostics));
    }
}

/// The saved-traversal operators rejected as v0.1 source surface. This slice is the
/// single owner of that vocabulary: a call whose method name appears here against a
/// saved path is flagged, and durable iterables are streamed with ordinary `for`
/// loops instead.
const REJECTED_TRAVERSAL_METHODS: &[&str] = &[
    "take", "window", "after", "from", "until", "resume", "reverse",
];

fn rejected_traversal_method<'a>(
    program: &CheckedProgram,
    file: &Path,
    callee: &'a Expression,
) -> Option<&'a str> {
    let Expression::Field {
        base, name, quoted, ..
    } = callee
    else {
        return None;
    };
    if *quoted
        || !saved_path_like_syntax(base)
        || callee_names_declared_saved_surface(program, file, callee)
    {
        return None;
    }
    REJECTED_TRAVERSAL_METHODS
        .contains(&name.as_str())
        .then_some(name.as_str())
}

fn callee_names_declared_saved_surface(
    program: &CheckedProgram,
    file: &Path,
    callee: &Expression,
) -> bool {
    lower_expr_for_file(program, file, callee, &[])
        .is_some_and(|callee| SavedPlaceResolver::new(program).declared_member_or_index(&callee))
}

fn saved_path_like_syntax(expr: &Expression) -> bool {
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Call { callee, .. } => saved_path_like_syntax(callee),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            saved_path_like_syntax(base)
        }
        _ => false,
    }
}

fn push(
    file: &Path,
    span: SourceSpan,
    message: &str,
    payload: DiagnosticPayload,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    diagnostics.push(
        CheckDiagnostic::error(CHECK_REJECTED_SURFACE, file, span, message).with_payload(payload),
    );
}
