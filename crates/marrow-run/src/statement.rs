//! Checked statement execution.

use marrow_check::{
    CheckedBinaryOp as BinaryOp, CheckedBody as ExecBody, CheckedCallTarget, CheckedCatchClause,
    CheckedElseIf, CheckedExpr as ExecExpr, CheckedStmt as ExecStmt, MarrowType,
};
use marrow_schema::Type;
use marrow_syntax::SourceSpan;

use crate::call::eval_call;
use crate::call_args::default_value;
use crate::collection::{
    eval_local_collection_write, eval_local_collection_write_value, resolve_local_collection_target,
};
use crate::env::{Env, Flow};
use crate::error::{RuntimeError, assign_error, type_error, unsupported};
use crate::exec::{eval_block, eval_match, local_target};
use crate::expr::{
    eval_arithmetic_with_left_value, eval_condition, eval_expr, eval_into_slot, eval_optional,
};
use crate::group_write::eval_group_entry_write_value;
use crate::host::Frame;
use crate::loop_exec::{eval_for, eval_while};
use crate::path::{direct_root_place, lower};
use crate::stdlib::convert_to_error_code;
use crate::transaction::eval_transaction;
use crate::value::{LocalTree, Value};
use crate::write_dispatch::{
    eval_delete, eval_local_field_set, eval_local_field_set_value, eval_resource_write,
    eval_resource_write_value, eval_saved_field_write, eval_saved_field_write_value,
};

pub(crate) fn eval_statement(
    statement: &ExecStmt,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    before_statement_hook(statement, env)?;
    match statement {
        ExecStmt::Const {
            name,
            binding_type,
            value,
            coerce_error_code,
            span,
        } => {
            let value = eval_into_slot(value, binding_type.as_ref(), env)?;
            let value = coerce_error_code_value(value, *coerce_error_code, *span)?;
            env.bind(name.clone(), value, false);
            Ok(Flow::Normal)
        }
        ExecStmt::Var {
            name,
            key_count,
            ty,
            binding_type,
            resource_default,
            value,
            coerce_error_code,
            span,
        } => eval_var(
            name,
            *key_count,
            ty.as_ref(),
            binding_type.as_ref(),
            *resource_default,
            value.as_ref(),
            *coerce_error_code,
            *span,
            env,
        ),
        ExecStmt::Assign {
            target,
            value,
            coerce_error_code,
            span,
        } => {
            eval_assignment(target, value, *coerce_error_code, *span, env)?;
            Ok(Flow::Normal)
        }
        ExecStmt::CompoundAssign {
            target,
            op,
            value,
            coerce_error_code,
            span,
        } => {
            eval_compound_assignment(target, *op, value, *coerce_error_code, *span, env)?;
            Ok(Flow::Normal)
        }
        ExecStmt::Delete { path, span, .. } => {
            eval_delete(path, *span, env)?;
            Ok(Flow::Normal)
        }
        ExecStmt::Return { value, .. } => eval_return(value.as_ref(), env),
        ExecStmt::Break { .. } => Ok(Flow::Break),
        ExecStmt::Continue { .. } => Ok(Flow::Continue),
        ExecStmt::Throw { value, span } => eval_throw(value, *span, env),
        ExecStmt::Expr { value, .. } => {
            eval_expr_statement(value, env)?;
            Ok(Flow::Normal)
        }
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
        ExecStmt::While {
            condition,
            body,
            span,
        } => eval_while(condition.as_ref(), body, *span, env),
        ExecStmt::For {
            binding,
            order,
            iterable,
            step,
            body,
            span,
        } => eval_for(binding, *order, iterable, step.as_ref(), body, *span, env),
        ExecStmt::Transaction { body, span, .. } => eval_transaction(body, *span, env),
        ExecStmt::Try { body, catch, .. } => eval_try(body, catch.as_ref(), env),
    }
}

fn before_statement_hook(statement: &ExecStmt, env: &mut Env<'_>) -> Result<(), RuntimeError> {
    // The hook is moved out so the callback can borrow `env` for its `Frame`
    // without aliasing the hook, then restored for the next statement.
    let Some(hook) = env.hook.take() else {
        return Ok(());
    };
    let span = statement.span();
    let result = hook.before_statement(span, Frame { env, span });
    env.hook = Some(hook);
    result
}

#[allow(clippy::too_many_arguments)]
fn eval_var(
    name: &str,
    key_count: usize,
    ty: Option<&Type>,
    binding_type: Option<&MarrowType>,
    resource_default: bool,
    value: Option<&ExecExpr>,
    coerce_error_code: bool,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if key_count != 0 {
        env.bind(
            name.to_string(),
            Value::LocalTree(LocalTree::default()),
            true,
        );
        return Ok(Flow::Normal);
    }
    let value = match value {
        Some(expr) => coerce_error_code_value(
            eval_into_slot(expr, binding_type, env)?,
            coerce_error_code,
            span,
        )?,
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

/// Enforce the error-code grammar on a value stored into an `ErrorCode` place when
/// the place demands it; otherwise pass the value through. A string literal is
/// rejected earlier at check, so this guards dynamic values reaching the place.
pub(crate) fn coerce_error_code_value(
    value: Value,
    coerce_error_code: bool,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    if coerce_error_code {
        convert_to_error_code(value, span)
    } else {
        Ok(value)
    }
}

fn eval_assignment(
    target: &ExecExpr,
    value: &ExecExpr,
    coerce_error_code: bool,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let ExecExpr::Field { base, name, .. } = target {
        if base.saved_place().is_some() {
            eval_saved_field_write(target, value, coerce_error_code, span, env)
        } else {
            eval_local_field_set(base, name, value, coerce_error_code, span, env)
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

fn eval_assignment_value(
    target: &ExecExpr,
    evaluated: Value,
    coerce_error_code: bool,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let ExecExpr::Field { base, name, .. } = target {
        if base.saved_place().is_some() {
            eval_saved_field_write_value(target, evaluated, coerce_error_code, span, env)
        } else {
            eval_local_field_set_value(base, name, evaluated, coerce_error_code, span, env)
        }
    } else if direct_root_place(target).is_some() {
        eval_resource_write_value(target, evaluated, span, env)
    } else if let ExecExpr::Call {
        callee,
        args,
        target: call_target,
        ..
    } = target
    {
        eval_call_assignment_value(target, callee, args, call_target, evaluated, span, env)
    } else {
        let name = local_target(target, span)?;
        env.assign(name, evaluated)
            .map_err(|error| assign_error(name, error, span))
    }
}

fn eval_compound_assignment(
    target: &ExecExpr,
    op: BinaryOp,
    value: &ExecExpr,
    coerce_error_code: bool,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if target.saved_place().is_some() {
        let path = lower(target, env)?;
        let current = path.read(span, env)?;
        let evaluated = eval_arithmetic_with_left_value(op, current, value, span, env)?;
        let evaluated = coerce_error_code_value(evaluated, coerce_error_code, span)?;
        return path.write(evaluated, span, env);
    }

    if let ExecExpr::Call {
        args,
        target: CheckedCallTarget::LocalCollection { name },
        ..
    } = target
    {
        let Some(collection_target) = resolve_local_collection_target(name, args, span, env)?
        else {
            return Err(unsupported("a checked local collection write", span));
        };
        let current = collection_target.read(env)?;
        let evaluated = eval_arithmetic_with_left_value(op, current, value, span, env)?;
        let evaluated = coerce_error_code_value(evaluated, coerce_error_code, span)?;
        return collection_target.write(evaluated, env);
    }

    let current = eval_expr(target, env)?;
    let evaluated = eval_arithmetic_with_left_value(op, current, value, span, env)?;
    eval_assignment_value(target, evaluated, coerce_error_code, span, env)
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
        // A saved keyed leaf or group entry is present-or-clear: an absent optional
        // routes to the entry-delete planner instead of a write.
        match eval_optional(value, env)? {
            Some(value) => eval_group_entry_write_value(target, value, span, env),
            None => eval_delete(target, span, env),
        }
    } else {
        eval_resource_write(target, value, span, env)
    }
}

fn eval_call_assignment_value(
    target: &ExecExpr,
    callee: &ExecExpr,
    args: &[marrow_check::CheckedArg],
    call_target: &CheckedCallTarget,
    evaluated: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let CheckedCallTarget::LocalCollection { name } = call_target {
        return if eval_local_collection_write_value(name, args, evaluated, span, env)? {
            Ok(())
        } else {
            Err(unsupported("a checked local collection write", span))
        };
    }
    if let ExecExpr::Field { base, .. } = callee
        && base.saved_place().is_some()
    {
        eval_group_entry_write_value(target, evaluated, span, env)
    } else {
        eval_resource_write_value(target, evaluated, span, env)
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

fn eval_return(value: Option<&ExecExpr>, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    let value = match value {
        Some(expr) => eval_optional(expr, env)?,
        None => None,
    };
    Ok(Flow::Return(value))
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
    eval_else_chain(else_ifs, else_block, span, env)
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
    if let Some(value) = eval_optional(value, env)? {
        return eval_bound_if_const(name, value, then_block, env);
    }
    eval_else_chain(else_ifs, else_block, span, env)
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

fn eval_else_chain(
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
        (Err(error), Some(clause)) => {
            if error.is_transaction_escape() {
                return Err(error);
            }
            match error.into_catch_value() {
                Ok(error) => eval_catch(clause, error, env),
                Err(error) => Err(error),
            }
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
