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
    env.store.begin().map_err(|error| error.located(span))?;

    let depth = env.enter_transaction();
    let result = eval_block(body, env);
    env.leave_transaction();

    match result {
        Ok(Flow::Throw(value)) => rollback_throw(value, depth, span, env),
        Ok(flow) => commit_flow(flow, depth, span, env),
        Err(error) => rollback_error(error, depth, span, env),
    }
}

/// Roll the open savepoint back and drop every deferred check it accumulated. A
/// rollback failure surfaces as a located store error, which supersedes whatever
/// outcome prompted the rollback.
fn rollback_and_discard(
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let rollback = env.store.rollback();
    env.discard_required_entry_checks(depth);
    env.discard_transaction_metadata(depth);
    rollback.map_err(|error| error.located(span))
}

fn rollback_throw(
    value: crate::value::Value,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    rollback_and_discard(depth, span, env)?;
    Ok(Flow::Throw(value))
}

fn rollback_error(
    error: RuntimeError,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    rollback_and_discard(depth, span, env)?;
    Err(error)
}

fn commit_flow(
    flow: Flow,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if depth == 1
        && let Err(error) = env.validate_required_entry_checks(depth, span)
    {
        rollback_and_discard(depth, span, env)?;
        return Err(error);
    }

    if let Err(error) = env.stamp_transaction_commit(depth, span) {
        rollback_and_discard(depth, span, env)?;
        return Err(error);
    }

    match env.store.commit() {
        Ok(()) => {
            env.commit_required_entry_checks(depth);
            env.commit_transaction_metadata(depth);
            Ok(flow)
        }
        Err(error) => {
            env.discard_required_entry_checks(depth);
            env.discard_transaction_metadata(depth);
            Err(error.located(span))
        }
    }
}
