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

fn rollback_throw(
    value: crate::value::Value,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let rollback = env.store.rollback();
    env.discard_required_entry_checks(depth);
    match rollback {
        Ok(()) => Ok(Flow::Throw(value)),
        Err(error) => Err(error.located(span)),
    }
}

fn rollback_error(
    error: RuntimeError,
    depth: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let rollback = env.store.rollback();
    env.discard_required_entry_checks(depth);
    match rollback {
        Ok(()) => Err(error),
        Err(error) => Err(error.located(span)),
    }
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
        let rollback = env.store.rollback();
        env.discard_required_entry_checks(depth);
        return match rollback {
            Ok(()) => Err(error),
            Err(error) => Err(error.located(span)),
        };
    }

    let commit = env.store.commit();
    match commit {
        Ok(()) => {
            env.commit_required_entry_checks(depth);
            Ok(flow)
        }
        Err(error) => {
            env.discard_required_entry_checks(depth);
            Err(error.located(span))
        }
    }
}
