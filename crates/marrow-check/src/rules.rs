//! Structural statement rules over function bodies.
//!
//! These checks read only the parsed statement tree: control flow that escapes
//! a `finally` block, `catch` type annotations, and assignment targets. They do
//! not need type or effect facts, so they run directly on each function body.

use std::path::Path;

use marrow_syntax::{Block, CatchClause, Expression, Severity, Statement};

use crate::CheckDiagnostic;

/// A `finally` block must not let control flow escape it via `return`, `break`,
/// or `continue`.
pub const CHECK_FINALLY_CONTROL_FLOW: &str = "check.finally_control_flow";
/// A `catch` annotation must be `Error`.
pub const CHECK_CATCH_TYPE: &str = "check.catch_type";
/// An assignment or merge target is not a writable place.
pub const CHECK_INVALID_ASSIGN_TARGET: &str = "check.invalid_assign_target";

/// Apply every structural statement rule to one function body.
pub fn check_function_body(file: &Path, body: &Block, out: &mut Vec<CheckDiagnostic>) {
    walk_block(file, body, out);
}

/// Walk a block applying the catch and assign-target rules to each statement,
/// recursing into nested blocks. A `try`'s `finally` block is handed to the
/// dedicated finally walk.
fn walk_block(file: &Path, block: &Block, out: &mut Vec<CheckDiagnostic>) {
    for statement in &block.statements {
        walk_statement(file, statement, out);
    }
}

fn walk_statement(file: &Path, statement: &Statement, out: &mut Vec<CheckDiagnostic>) {
    match statement {
        Statement::Assign { target, .. } | Statement::Merge { target, .. } => {
            if !is_assignable(target) {
                out.push(diagnostic(
                    CHECK_INVALID_ASSIGN_TARGET,
                    file,
                    target,
                    "assignment target is not a writable place",
                ));
            }
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_block(file, then_block, out);
            for else_if in else_ifs {
                walk_block(file, &else_if.block, out);
            }
            if let Some(block) = else_block {
                walk_block(file, block, out);
            }
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. }
        | Statement::Lock { body, .. } => walk_block(file, body, out),
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            walk_block(file, body, out);
            if let Some(catch) = catch {
                check_catch(file, catch, out);
                walk_block(file, &catch.block, out);
            }
            if let Some(finally) = finally {
                // The finally block is also an ordinary block for the other
                // rules, plus the escaping-control-flow rule.
                walk_block(file, finally, out);
                walk_finally(file, finally, 0, &mut Vec::new(), out);
            }
        }
        _ => {}
    }
}

/// A `catch` annotation, if present, must name `Error`. A bare catch is fine.
fn check_catch(file: &Path, catch: &CatchClause, out: &mut Vec<CheckDiagnostic>) {
    if let Some(ty) = &catch.ty
        && ty.text != "Error"
    {
        out.push(CheckDiagnostic {
            code: CHECK_CATCH_TYPE.to_string(),
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("catch type must be `Error`, found `{}`", ty.text),
            line: catch.block.span.line,
            column: catch.block.span.column,
        });
    }
}

/// Walk a `finally` block reporting control flow that escapes it.
///
/// `return` always escapes. An unlabeled `break`/`continue` escapes only when no
/// loop encloses it within the finally (`loop_depth == 0`). A labeled
/// `break`/`continue` escapes unless its label names a loop introduced within
/// the finally block (`loop_labels`). A nested `try`'s own `finally` is a fresh
/// scope and is not walked here.
fn walk_finally(
    file: &Path,
    block: &Block,
    loop_depth: usize,
    loop_labels: &mut Vec<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        match statement {
            Statement::Return { .. } => out.push(diagnostic_at(
                CHECK_FINALLY_CONTROL_FLOW,
                file,
                statement,
                "`return` is not allowed in a `finally` block",
            )),
            Statement::Break { label, .. } | Statement::Continue { label, .. } => {
                if finally_jump_escapes(label.as_deref(), loop_depth, loop_labels) {
                    out.push(diagnostic_at(
                        CHECK_FINALLY_CONTROL_FLOW,
                        file,
                        statement,
                        "control flow may not leave a `finally` block",
                    ));
                }
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_finally(file, then_block, loop_depth, loop_labels, out);
                for else_if in else_ifs {
                    walk_finally(file, &else_if.block, loop_depth, loop_labels, out);
                }
                if let Some(block) = else_block {
                    walk_finally(file, block, loop_depth, loop_labels, out);
                }
            }
            Statement::While { label, body, .. } | Statement::For { label, body, .. } => {
                let pushed = label.clone();
                if let Some(label) = &pushed {
                    loop_labels.push(label.clone());
                }
                walk_finally(file, body, loop_depth + 1, loop_labels, out);
                if pushed.is_some() {
                    loop_labels.pop();
                }
            }
            Statement::Transaction { body, .. } | Statement::Lock { body, .. } => {
                walk_finally(file, body, loop_depth, loop_labels, out);
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                // The nested body and catch still sit inside this finally, so
                // their escaping jumps are also illegal. The nested finally is a
                // fresh scope handled by the ordinary walk.
                walk_finally(file, body, loop_depth, loop_labels, out);
                if let Some(catch) = catch {
                    walk_finally(file, &catch.block, loop_depth, loop_labels, out);
                }
                if let Some(finally) = finally {
                    walk_finally(file, finally, 0, &mut Vec::new(), out);
                }
            }
            _ => {}
        }
    }
}

/// Does a `break`/`continue` in a finally block escape it? An unlabeled jump
/// escapes when no enclosing loop is within the finally. A labeled jump escapes
/// unless its target loop was introduced within the finally.
fn finally_jump_escapes(label: Option<&str>, loop_depth: usize, loop_labels: &[String]) -> bool {
    match label {
        None => loop_depth == 0,
        Some(label) => !loop_labels.iter().any(|known| known == label),
    }
}

/// A writable place: a bare name, a saved root, a field of a place, or a key
/// lookup on a saved place. A parenthesized suffix on a bare name is a function
/// call, not a place, so `f(x)` is rejected while `^books(id).title` is allowed.
fn is_assignable(target: &Expression) -> bool {
    match target {
        Expression::Name { segments, .. } => segments.len() == 1,
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_assignable(base),
        Expression::Call { callee, .. } => is_key_lookup_target(callee),
        _ => false,
    }
}

/// The callee of a key-lookup place: a saved root, a field of a place, or a
/// further key lookup. A bare name callee is a function call, not a place.
fn is_key_lookup_target(callee: &Expression) -> bool {
    match callee {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_assignable(base),
        Expression::Call { callee, .. } => is_key_lookup_target(callee),
        _ => false,
    }
}

/// A diagnostic located at an expression's span.
fn diagnostic(code: &str, file: &Path, expr: &Expression, message: &str) -> CheckDiagnostic {
    let span = expr.span();
    CheckDiagnostic {
        code: code.to_string(),
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: message.to_string(),
        line: span.line,
        column: span.column,
    }
}

/// A diagnostic located at a statement's span.
fn diagnostic_at(code: &str, file: &Path, statement: &Statement, message: &str) -> CheckDiagnostic {
    let span = statement.span();
    CheckDiagnostic {
        code: code.to_string(),
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: message.to_string(),
        line: span.line,
        column: span.column,
    }
}
