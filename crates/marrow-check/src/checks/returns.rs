//! Return-value placement and the divergence under-approximation: each `return`'s
//! value presence against the declared return type, and whether a block provably
//! returns or diverges on every path.

use std::path::Path;

use marrow_syntax::Severity;

use crate::{CHECK_RETURN_VALUE, CheckDiagnostic, DiagnosticPayload};

/// Flag each `return` whose value presence does not match the declared return
/// type. Recurses into nested blocks; `finally` is left to
/// `check.finally_control_flow`.
pub(crate) fn check_return_values(
    file: &Path,
    body: &marrow_syntax::Block,
    returns_value: bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    for statement in &body.statements {
        match statement {
            Statement::Return { value, span } => {
                let message = match (returns_value, value.is_some()) {
                    (true, false) => "a value-returning function must return a value",
                    (false, true) => "a function with no return type cannot return a value",
                    _ => continue,
                };
                diagnostics.push(CheckDiagnostic {
                    code: CHECK_RETURN_VALUE,
                    severity: Severity::Error,
                    file: file.to_path_buf(),
                    message: message.to_string(),
                    span: *span,
                    payload: DiagnosticPayload::None,
                });
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                check_return_values(file, then_block, returns_value, diagnostics);
                for else_if in else_ifs {
                    check_return_values(file, &else_if.block, returns_value, diagnostics);
                }
                if let Some(block) = else_block {
                    check_return_values(file, block, returns_value, diagnostics);
                }
            }
            Statement::While { body, .. }
            | Statement::For { body, .. }
            | Statement::Transaction { body, .. } => {
                check_return_values(file, body, returns_value, diagnostics);
            }
            Statement::Try { body, catch, .. } => {
                check_return_values(file, body, returns_value, diagnostics);
                if let Some(clause) = catch {
                    check_return_values(file, &clause.block, returns_value, diagnostics);
                }
                // `finally` cannot contain `return` (check.finally_control_flow).
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    check_return_values(file, &arm.block, returns_value, diagnostics);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::Delete { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. } => {}
        }
    }
}

/// A sound under-approximation of "every reachable path returns or diverges". It
/// is conservative — a body ending in a call or a loop may diverge, so it is not
/// flagged — favoring no false positives over catching every genuine case.
pub(crate) fn block_returns(block: &marrow_syntax::Block) -> bool {
    block.statements.last().is_some_and(statement_returns)
}

pub(crate) fn statement_returns(statement: &marrow_syntax::Statement) -> bool {
    use marrow_syntax::{Expression, Statement};
    match statement {
        Statement::Return { .. } | Statement::Throw { .. } => true,
        // A call may throw or loop forever, so a function ending in one is allowed.
        Statement::Expr { value, .. } => matches!(value, Expression::Call { .. }),
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => else_block.as_ref().is_some_and(|else_block| {
            block_returns(then_block)
                && else_ifs.iter().all(|else_if| block_returns(&else_if.block))
                && block_returns(else_block)
        }),
        Statement::Transaction { body, .. } => block_returns(body),
        Statement::Try { body, catch, .. } => {
            block_returns(body)
                && catch
                    .as_ref()
                    .is_none_or(|clause| block_returns(&clause.block))
        }
        // An exhaustive match returns on every path exactly when every arm does;
        // an empty match cannot arise, so `all` over no arms is not a false yes.
        Statement::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| block_returns(&arm.block))
        }
        // A loop may not run or may run forever; conservatively treat its end as
        // diverging rather than risk a false positive.
        Statement::While { .. } | Statement::For { .. } => true,
        Statement::Const { .. }
        | Statement::Var { .. }
        | Statement::Assign { .. }
        | Statement::Delete { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. } => false,
    }
}
