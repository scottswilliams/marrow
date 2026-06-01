use std::path::Path;

use marrow_syntax::{
    ArgMode, Block, Declaration, Expression, InterpolationPart, Severity, SourceSpan, Statement,
};

use crate::{CHECK_PROTOTYPE_ONLY, CheckDiagnostic, CheckedProgram, is_saved_path_expression};

pub(crate) fn check_prototype_only(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for declaration in &parsed.file.declarations {
        match declaration {
            Declaration::Resource(_) => {}
            Declaration::Function(function) => {
                check_block(program, file, &function.body, diagnostics);
            }
            Declaration::Const(constant) => {
                if let Some(value) = &constant.value {
                    check_expr(program, file, value, diagnostics);
                }
            }
            Declaration::Enum(_) => {}
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
        Statement::Merge {
            target,
            value,
            span,
        } => {
            push(
                file,
                *span,
                "`merge` is prototype-only; use explicit checked writes or a future checked transform",
                diagnostics,
            );
            check_expr(program, file, target, diagnostics);
            check_expr(program, file, value, diagnostics);
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
        Statement::Lock { path, body, span } => {
            push(
                file,
                *span,
                "`lock` is prototype-only; v0.1 uses transactions without a source-level lock primitive",
                diagnostics,
            );
            if let Some(path) = path {
                check_expr(program, file, path, diagnostics);
            }
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
        Expression::Call { callee, args, .. } => {
            check_expr(program, file, callee, diagnostics);
            for arg in args {
                if matches!(arg.mode, Some(ArgMode::InOut))
                    && is_saved_path_expression(program, &arg.value)
                {
                    push(
                        file,
                        arg.value.span(),
                        "saved `inout` is prototype-only; saved writes must be explicit checked effects",
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

fn push(file: &Path, span: SourceSpan, message: &str, diagnostics: &mut Vec<CheckDiagnostic>) {
    diagnostics.push(CheckDiagnostic {
        code: CHECK_PROTOTYPE_ONLY,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: message.to_string(),
        span,
    });
}
