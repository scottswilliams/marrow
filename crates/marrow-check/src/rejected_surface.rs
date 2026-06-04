use std::path::Path;

use marrow_syntax::{
    ArgMode, Block, Declaration, EvolveStep, Expression, InterpolationPart, Severity, SourceSpan,
    Statement,
};

use crate::infer::saved_layer_chain;
use crate::resolve::resolve_store_by_root;
use crate::{CHECK_REJECTED_SURFACE, CheckDiagnostic, CheckedProgram};

pub(crate) fn check_rejected_surface(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for declaration in &parsed.file.declarations {
        match declaration {
            Declaration::Resource(_) | Declaration::Store(_) => {}
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
        Statement::Assign { target, value, .. } => {
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
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            check_block(program, file, body, diagnostics);
            if let Some(catch) = catch {
                check_block(program, file, &catch.block, diagnostics);
            }
            if let Some(finally) = finally {
                check_block(program, file, finally, diagnostics);
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
        Statement::Break { .. } | Statement::Continue { .. } => {}
    }
}

fn check_expr(
    program: &CheckedProgram,
    file: &Path,
    expr: &Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match expr {
        Expression::Call {
            callee, args, span, ..
        } => {
            if let Some(method) = rejected_traversal_method(program, callee) {
                push(
                    file,
                    *span,
                    &format!(
                        "saved traversal method `.{method}(...)` is not a v0.1 source surface; stream durable iterables with ordinary `for` loops"
                    ),
                    diagnostics,
                );
            }
            check_expr(program, file, callee, diagnostics);
            for arg in args {
                if matches!(arg.mode, Some(ArgMode::InOut)) && saved_path_like_syntax(&arg.value) {
                    push(
                        file,
                        arg.value.span(),
                        "saved `inout` is not a v0.1 source surface; saved writes must be explicit checked effects",
                        diagnostics,
                    );
                }
                check_expr(program, file, &arg.value, diagnostics);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            check_expr(program, file, base, diagnostics);
        }
        Expression::Unary { operand, .. } => {
            check_expr(program, file, operand, diagnostics);
        }
        Expression::Binary { left, right, .. } => {
            check_expr(program, file, left, diagnostics);
            check_expr(program, file, right, diagnostics);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    check_expr(program, file, expr, diagnostics);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn rejected_traversal_method<'a>(
    program: &CheckedProgram,
    callee: &'a Expression,
) -> Option<&'a str> {
    let Expression::Field {
        base, name, quoted, ..
    } = callee
    else {
        return None;
    };
    if *quoted || !saved_path_like_syntax(base) || declared_saved_member_or_index(program, callee) {
        return None;
    }
    matches!(
        name.as_str(),
        "take" | "window" | "after" | "from" | "until" | "resume" | "reverse"
    )
    .then_some(name.as_str())
}

fn declared_saved_member_or_index(program: &CheckedProgram, callee: &Expression) -> bool {
    let Expression::Field { base, name, .. } = callee else {
        return false;
    };
    if let Expression::SavedRoot { name: root, .. } = base.as_ref()
        && let Some(store) = resolve_store_by_root(program, root)
        && store.store.indexes.iter().any(|index| &index.name == name)
    {
        return true;
    }
    let Some((root, layers)) = saved_layer_chain(callee) else {
        return false;
    };
    let Some(store) = resolve_store_by_root(program, root) else {
        return false;
    };
    let resource = store.resource;
    resource.descend_layers(&layers).is_some() || resource.field_type(&layers).is_some()
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

fn push(file: &Path, span: SourceSpan, message: &str, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_REJECTED_SURFACE,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: message.to_string(),
        span,
    });
}
