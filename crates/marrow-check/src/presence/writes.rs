use marrow_syntax::{Block, Expression, InterpolationPart, Statement};

use super::calls::{callee_name, is_append_call};
use crate::CheckedProgram;

pub(super) fn call_writes_saved_data(program: &CheckedProgram, callee: &Expression) -> bool {
    let Expression::Name { segments, .. } = callee else {
        return false;
    };
    let Some(leaf) = segments.last() else {
        return false;
    };
    function_name_writes_saved_data(program, leaf, &mut Vec::new())
}

fn function_name_writes_saved_data(
    program: &CheckedProgram,
    name: &str,
    visiting: &mut Vec<String>,
) -> bool {
    if visiting.iter().any(|item| item == name) {
        return false;
    }
    visiting.push(name.to_string());
    let writes =
        program.facts.functions().iter().any(|function| {
            function.name == name && !function.direct_effects.saved_writes.is_empty()
        }) || program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .filter(|function| function.name == name)
            .any(|function| block_calls_saved_writer(program, &function.body, visiting));
    visiting.pop();
    writes
}

fn block_calls_saved_writer(
    program: &CheckedProgram,
    block: &Block,
    visiting: &mut Vec<String>,
) -> bool {
    block
        .statements
        .iter()
        .any(|statement| statement_calls_saved_writer(program, statement, visiting))
}

fn statement_calls_saved_writer(
    program: &CheckedProgram,
    statement: &Statement,
    visiting: &mut Vec<String>,
) -> bool {
    match statement {
        Statement::Const { value, .. }
        | Statement::Var {
            value: Some(value), ..
        }
        | Statement::Throw { value, .. }
        | Statement::Expr { value, .. } => expr_calls_saved_writer(program, value, visiting),
        Statement::Var { value: None, .. } => false,
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            expr_calls_saved_writer(program, target, visiting)
                || expr_calls_saved_writer(program, value, visiting)
        }
        Statement::Delete { path, .. } => expr_calls_saved_writer(program, path, visiting),
        Statement::Return { value, .. } => value
            .as_ref()
            .is_some_and(|value| expr_calls_saved_writer(program, value, visiting)),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => statement_if_calls_saved_writer(
            program, condition, then_block, else_ifs, else_block, visiting,
        ),
        Statement::While {
            condition, body, ..
        } => {
            condition
                .as_ref()
                .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
                || block_calls_saved_writer(program, body, visiting)
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => statement_for_calls_saved_writer(program, iterable, step.as_ref(), body, visiting),
        Statement::Transaction { body, .. } | Statement::Lock { body, .. } => {
            block_calls_saved_writer(program, body, visiting)
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => statement_try_calls_saved_writer(
            program,
            body,
            catch.as_ref(),
            finally.as_ref(),
            visiting,
        ),
        Statement::Match {
            scrutinee, arms, ..
        } => statement_match_calls_saved_writer(program, scrutinee.as_ref(), arms, visiting),
        Statement::Break { .. } | Statement::Continue { .. } => false,
    }
}

fn statement_if_calls_saved_writer(
    program: &CheckedProgram,
    condition: &Option<Expression>,
    then_block: &Block,
    else_ifs: &[marrow_syntax::ElseIf],
    else_block: &Option<Block>,
    visiting: &mut Vec<String>,
) -> bool {
    condition
        .as_ref()
        .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
        || block_calls_saved_writer(program, then_block, visiting)
        || else_ifs.iter().any(|else_if| {
            else_if
                .condition
                .as_ref()
                .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
                || block_calls_saved_writer(program, &else_if.block, visiting)
        })
        || else_block
            .as_ref()
            .is_some_and(|block| block_calls_saved_writer(program, block, visiting))
}

fn statement_for_calls_saved_writer(
    program: &CheckedProgram,
    iterable: &Expression,
    step: Option<&Expression>,
    body: &Block,
    visiting: &mut Vec<String>,
) -> bool {
    expr_calls_saved_writer(program, iterable, visiting)
        || step
            .as_ref()
            .is_some_and(|step| expr_calls_saved_writer(program, step, visiting))
        || block_calls_saved_writer(program, body, visiting)
}

fn statement_try_calls_saved_writer(
    program: &CheckedProgram,
    body: &Block,
    catch: Option<&marrow_syntax::CatchClause>,
    finally: Option<&Block>,
    visiting: &mut Vec<String>,
) -> bool {
    block_calls_saved_writer(program, body, visiting)
        || catch
            .as_ref()
            .is_some_and(|catch| block_calls_saved_writer(program, &catch.block, visiting))
        || finally
            .as_ref()
            .is_some_and(|finally| block_calls_saved_writer(program, finally, visiting))
}

fn statement_match_calls_saved_writer(
    program: &CheckedProgram,
    scrutinee: Option<&Expression>,
    arms: &[marrow_syntax::MatchArm],
    visiting: &mut Vec<String>,
) -> bool {
    scrutinee
        .as_ref()
        .is_some_and(|scrutinee| expr_calls_saved_writer(program, scrutinee, visiting))
        || arms
            .iter()
            .any(|arm| block_calls_saved_writer(program, &arm.block, visiting))
}

pub(super) fn expr_calls_saved_writer(
    program: &CheckedProgram,
    expr: &Expression,
    visiting: &mut Vec<String>,
) -> bool {
    match expr {
        Expression::Call { callee, args, .. } => {
            is_append_call(callee)
                || callee_name(callee)
                    .is_some_and(|name| function_name_writes_saved_data(program, name, visiting))
                || expr_calls_saved_writer(program, callee, visiting)
                || args
                    .iter()
                    .any(|arg| expr_calls_saved_writer(program, &arg.value, visiting))
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            expr_calls_saved_writer(program, base, visiting)
        }
        Expression::Unary { operand, .. } => expr_calls_saved_writer(program, operand, visiting),
        Expression::Binary { left, right, .. } => {
            expr_calls_saved_writer(program, left, visiting)
                || expr_calls_saved_writer(program, right, visiting)
        }
        Expression::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            InterpolationPart::Text { .. } => false,
            InterpolationPart::Expr(expr) => expr_calls_saved_writer(program, expr, visiting),
        }),
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {
            false
        }
    }
}
