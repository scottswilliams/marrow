use marrow_check::CheckedBody as ExecBody;
use marrow_syntax::SourceSpan;

use crate::env::{Env, Flow};
use crate::error::{Located, RuntimeError};
use crate::exec::eval_block;

pub(crate) fn eval_transaction(
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if env.transaction_depth() == 0 {
        env.store.begin().map_err(|error| error.located(span))?;
    }

    let depth = env.enter_transaction();
    notify_transaction_begin(depth, env);
    let result = eval_block(body, env);
    env.leave_transaction();

    match result {
        Ok(Flow::Throw {
            value,
            span: throw_span,
            ..
        }) => rollback_throw(value, throw_span, depth, span, env),
        Ok(flow) => commit_flow(flow, depth, span, env),
        Err(error) => rollback_error(error, depth, span, env),
    }
}

/// Roll the open transaction back and drop every deferred check it accumulated.
/// A rollback failure surfaces as a located store error, which supersedes
/// whatever outcome prompted the rollback.
fn rollback_and_discard(
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    notify_transaction_rollback(depth, env);
    if depth > 1 {
        return Ok(());
    }
    let rollback = env.store.rollback();
    env.discard_required_entry_checks();
    env.discard_transaction_metadata();
    rollback.map_err(|error| error.located(span))
}

fn rollback_throw(
    value: crate::value::Value,
    throw_span: SourceSpan,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    rollback_and_discard(depth, span, env)?;
    Ok(Flow::Throw {
        value,
        span: throw_span,
        transaction_escape: depth > 1,
    })
}

fn rollback_error(
    error: RuntimeError,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    rollback_and_discard(depth, span, env)?;
    Err(error.with_transaction_escape(depth > 1))
}

fn commit_flow(
    flow: Flow,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if depth > 1 {
        notify_transaction_commit(depth, env);
        return Ok(flow);
    }

    if depth == 1
        && let Err(error) = env.validate_required_entry_checks(span)
    {
        rollback_and_discard(depth, span, env)?;
        return Err(error);
    }

    if let Err(error) = env.stamp_transaction_commit(span) {
        rollback_and_discard(depth, span, env)?;
        return Err(error);
    }

    match env.store.commit() {
        Ok(()) => {
            env.commit_required_entry_checks();
            env.commit_transaction_metadata();
            notify_transaction_commit(depth, env);
            Ok(flow)
        }
        Err(error) => {
            env.discard_required_entry_checks();
            env.discard_transaction_metadata();
            notify_transaction_rollback(depth, env);
            Err(error.located(span))
        }
    }
}

fn notify_transaction_begin(depth: usize, env: &mut Env<'_>) {
    if let Some(hook) = env.hook.as_deref_mut() {
        hook.transaction_begin(depth);
    }
}

fn notify_transaction_commit(depth: usize, env: &mut Env<'_>) {
    if let Some(hook) = env.hook.as_deref_mut() {
        hook.transaction_commit(depth);
    }
}

fn notify_transaction_rollback(depth: usize, env: &mut Env<'_>) {
    if let Some(hook) = env.hook.as_deref_mut() {
        hook.transaction_rollback(depth);
    }
}
