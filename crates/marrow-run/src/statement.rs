//! Checked statement execution.

use marrow_check::{
    CheckedBody as ExecBody, CheckedCallTarget, CheckedCatchClause, CheckedElseIf,
    CheckedExpr as ExecExpr, CheckedStmt as ExecStmt,
};
use marrow_schema::Type;
use marrow_syntax::SourceSpan;

use crate::call::{
    eval_call, expr_call_maybe_present, expr_return_absence_can_propagate,
    expression_absent_at_resolution_site,
};
use crate::call_args::default_value;
use crate::durable_read::read_saved_value_if_present;
use crate::env::{Env, Flow};
use crate::error::{RuntimeError, assign_error, type_error, unsupported};
use crate::exec::{eval_block, eval_match, local_target};
use crate::expr::{eval_condition, eval_expr};
use crate::group_write::eval_group_entry_write;
use crate::host::Frame;
use crate::local_collection::eval_local_collection_write;
use crate::loop_exec::{eval_for, eval_while};
use crate::path::direct_root_place;
use crate::transaction::eval_transaction;
use crate::value::Value;
use crate::write_dispatch::{
    eval_delete, eval_local_field_set, eval_resource_write, eval_saved_field_write,
};

pub(crate) fn eval_statement(
    statement: &ExecStmt,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    before_statement_hook(statement, env)?;
    if let Some(flow) = eval_binding_or_write_statement(statement, env)? {
        return Ok(flow);
    }
    eval_control_statement(statement, env)
}

fn eval_binding_or_write_statement(
    statement: &ExecStmt,
    env: &mut Env<'_>,
) -> Result<Option<Flow>, RuntimeError> {
    match statement {
        ExecStmt::Const { name, value, .. } => {
            let value = eval_expr(value, env)?;
            env.bind(name.clone(), value, false);
            Ok(Some(Flow::Normal))
        }
        ExecStmt::Var {
            name,
            key_count,
            ty,
            resource_default,
            value,
            span,
            ..
        } => eval_var(
            name,
            *key_count,
            ty.as_ref(),
            *resource_default,
            value.as_ref(),
            *span,
            env,
        )
        .map(Some),
        ExecStmt::Assign {
            target,
            value,
            span,
            ..
        } => {
            eval_assignment(target, value, *span, env)?;
            Ok(Some(Flow::Normal))
        }
        ExecStmt::Delete { path, span, .. } => {
            eval_delete(path, *span, env)?;
            Ok(Some(Flow::Normal))
        }
        ExecStmt::Return { value, .. } => {
            let value = match value.as_ref() {
                Some(expr) if expr_return_absence_can_propagate(expr) => match eval_expr(expr, env)
                {
                    Ok(value) => Some(value),
                    Err(error) if expression_absent_at_resolution_site(expr, &error) => {
                        return Ok(Some(Flow::ReturnAbsent));
                    }
                    Err(error) => return Err(error),
                },
                Some(expr) => Some(eval_expr(expr, env)?),
                None => None,
            };
            Ok(Some(Flow::Return(value)))
        }
        ExecStmt::ReturnAbsent { .. } => Ok(Some(Flow::ReturnAbsent)),
        ExecStmt::Expr { value, .. } => {
            eval_expr_statement(value, env)?;
            Ok(Some(Flow::Normal))
        }
        ExecStmt::Throw { value, span } => eval_throw(value, *span, env).map(Some),
        _ => Ok(None),
    }
}

fn eval_control_statement(statement: &ExecStmt, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    match statement {
        ExecStmt::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            span,
        } => eval_if(
            condition.as_ref(),
            then_block,
            else_ifs,
            else_block.as_ref(),
            *span,
            env,
        ),
        ExecStmt::IfConst {
            name,
            value,
            then_block,
            else_ifs,
            else_block,
            span,
            ..
        } => eval_if_const(
            name,
            value,
            then_block,
            else_ifs,
            else_block.as_ref(),
            *span,
            env,
        ),
        ExecStmt::Match {
            scrutinee,
            arms,
            enum_ref,
            span,
        } => eval_match(scrutinee.as_ref(), arms, *enum_ref, *span, env),
        ExecStmt::Break { .. } => Ok(Flow::Break),
        ExecStmt::Continue { .. } => Ok(Flow::Continue),
        ExecStmt::While {
            condition,
            body,
            span,
        } => eval_while(condition.as_ref(), body, *span, env),
        ExecStmt::For {
            binding,
            iterable,
            step,
            body,
            span,
        } => eval_for(binding, iterable, step.as_ref(), body, *span, env),
        ExecStmt::Transaction { body, span, .. } => eval_transaction(body, *span, env),
        ExecStmt::Try { body, catch, .. } => eval_try(body, catch.as_ref(), env),
        _ => unreachable!("binding/write statements are handled before control dispatch"),
    }
}

fn before_statement_hook(statement: &ExecStmt, env: &mut Env<'_>) -> Result<(), RuntimeError> {
    // The hook is moved out so the callback can borrow `env` for its `Frame`
    // without aliasing the hook, then restored for the next statement.
    let Some(hook) = env.hook.take() else {
        return Ok(());
    };
    let result = hook.before_statement(statement.span(), Frame { env });
    env.hook = Some(hook);
    result
}

fn eval_var(
    name: &str,
    key_count: usize,
    ty: Option<&Type>,
    resource_default: bool,
    value: Option<&ExecExpr>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if key_count != 0 {
        env.bind(name.to_string(), Value::LocalTree(Vec::new()), true);
        return Ok(Flow::Normal);
    }
    let value = match value {
        Some(expr) => eval_expr(expr, env)?,
        None => match ty {
            Some(Type::Named(_)) if resource_default => Value::Resource(Vec::new()),
            Some(ty) => default_value(ty)
                .ok_or_else(|| unsupported("an uninitialized variable of this type", span))?,
            None => return Err(unsupported("an uninitialized variable", span)),
        },
    };
    env.bind(name.to_string(), value, true);
    Ok(Flow::Normal)
}

fn eval_assignment(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let ExecExpr::Field { base, name, .. } = target {
        if base.saved_place().is_some() {
            eval_saved_field_write(target, value, span, env)
        } else {
            eval_local_field_set(base, name, value, span, env)
        }
    } else if direct_root_place(target).is_some() {
        eval_resource_write(target, value, span, env)
    } else if let ExecExpr::Call {
        callee,
        args,
        target: call_target,
        ..
    } = target
    {
        eval_call_assignment(target, callee, args, call_target, value, span, env)
    } else {
        let name = local_target(target, span)?;
        let evaluated = eval_expr(value, env)?;
        env.assign(name, evaluated)
            .map_err(|error| assign_error(name, error, span))
    }
}

fn eval_call_assignment(
    target: &ExecExpr,
    callee: &ExecExpr,
    args: &[marrow_check::CheckedArg],
    call_target: &CheckedCallTarget,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let CheckedCallTarget::LocalCollection { name } = call_target {
        return if eval_local_collection_write(name, args, value, span, env)? {
            Ok(())
        } else {
            Err(unsupported("a checked local collection write", span))
        };
    }
    if let ExecExpr::Field { base, .. } = callee
        && base.saved_place().is_some()
    {
        eval_group_entry_write(target, value, span, env)
    } else {
        eval_resource_write(target, value, span, env)
    }
}

fn eval_expr_statement(value: &ExecExpr, env: &mut Env<'_>) -> Result<(), RuntimeError> {
    if let ExecExpr::Call {
        args, target, span, ..
    } = value
    {
        eval_call(value, args, target, *span, env)?;
    } else {
        eval_expr(value, env)?;
    }
    Ok(())
}

fn eval_if(
    condition: Option<&ExecExpr>,
    then_block: &ExecBody,
    else_ifs: &[CheckedElseIf],
    else_block: Option<&ExecBody>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if eval_condition(condition, span, env)? {
        return eval_block(then_block, env);
    }
    for else_if in else_ifs {
        if eval_condition(else_if.condition.as_ref(), span, env)? {
            return eval_block(&else_if.block, env);
        }
    }
    match else_block {
        Some(block) => eval_block(block, env),
        None => Ok(Flow::Normal),
    }
}

fn eval_if_const(
    name: &str,
    value: &ExecExpr,
    then_block: &ExecBody,
    else_ifs: &[CheckedElseIf],
    else_block: Option<&ExecBody>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if let Some(value) = eval_if_const_value(value, env)? {
        return eval_bound_if_const(name, value, then_block, env);
    }
    eval_if_const_fallback(else_ifs, else_block, span, env)
}

fn eval_if_const_value(value: &ExecExpr, env: &mut Env<'_>) -> Result<Option<Value>, RuntimeError> {
    if value.saved_place().is_some() {
        return read_saved_value_if_present(value, value.span(), env);
    }
    if expr_call_maybe_present(value) {
        return match eval_expr(value, env) {
            Ok(value) => Ok(Some(value)),
            Err(error) if expression_absent_at_resolution_site(value, &error) => Ok(None),
            Err(error) => Err(error),
        };
    }
    Ok(Some(eval_expr(value, env)?))
}

fn eval_bound_if_const(
    name: &str,
    value: Value,
    then_block: &ExecBody,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    env.push_scope();
    env.bind(name.to_string(), value, false);
    let result = eval_block(then_block, env);
    env.pop_scope();
    result
}

fn eval_if_const_fallback(
    else_ifs: &[CheckedElseIf],
    else_block: Option<&ExecBody>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    for else_if in else_ifs {
        if eval_condition(else_if.condition.as_ref(), span, env)? {
            return eval_block(&else_if.block, env);
        }
    }
    match else_block {
        Some(block) => eval_block(block, env),
        None => Ok(Flow::Normal),
    }
}

fn eval_throw(value: &ExecExpr, span: SourceSpan, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    let thrown = eval_expr(value, env)?;
    if !matches!(thrown, Value::Resource(_)) {
        return Err(type_error("`throw` requires an `Error` value", span));
    }
    Ok(Flow::Throw {
        value: thrown,
        span,
        transaction_escape: false,
    })
}

fn eval_try(
    body: &ExecBody,
    catch: Option<&CheckedCatchClause>,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let outcome = eval_block(body, env);
    match (outcome, catch) {
        (
            Ok(Flow::Throw {
                value,
                transaction_escape: false,
                ..
            }),
            Some(clause),
        ) => eval_catch(clause, value, env),
        (Err(error), Some(clause)) if !error.is_transaction_escape() && error.is_catchable() => {
            let error = error
                .into_error_value()
                .expect("a catchable runtime fault materializes an Error value");
            eval_catch(clause, error, env)
        }
        (outcome, _) => outcome,
    }
}

fn eval_catch(
    clause: &CheckedCatchClause,
    error: Value,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    env.push_scope();
    env.bind(clause.name.clone(), error, false);
    let caught = eval_block(&clause.block, env);
    env.pop_scope();
    caught
}
