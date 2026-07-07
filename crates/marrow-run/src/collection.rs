//! Sequence and keyed builtins: keys/values/count/neighbor/append. `keys`/`values`
//! materialize a local collection in value position; saved data is iterated in place
//! with `for ... in`.

use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RUN_ABSENT, RUN_TYPE, RuntimeError, raise_fault, unsupported};
use crate::expr::eval_expr;
use crate::stdlib::check_key_collection;
use crate::value::{Sequence, Value};

mod append;
mod local;

pub(crate) use append::{eval_append, eval_next_id};
pub(crate) use local::{
    enumerate_local_keys_call_arg, enumerate_reversed_local_keys_call_arg,
    eval_local_collection_delete, eval_local_collection_read, eval_local_collection_write,
    eval_local_collection_write_value, local_collection_count, materialize_local_collection_dir,
    resolve_local_collection_target,
};

/// The order a saved-layer walk yields its children. A descending walk reverses as
/// one, so a composite identity is true-reversed at every level, not only its
/// outermost component.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Ascending,
    Descending,
}

/// The absent-element fault for a fixed read address. It is catchable so a
/// maybe-present read at the read site — a positional sequence element, a keyed
/// tree entry, or a stdlib cell selection — resolves through `??`/`if const`/
/// `exists`/`catch`; an unguarded one still surfaces with `run.absent_element`.
pub(crate) fn absent_read(message: String, span: SourceSpan) -> RuntimeError {
    raise_fault(RUN_ABSENT, message, span)
}

pub(crate) fn durable_collection_value(span: SourceSpan) -> RuntimeError {
    unsupported(
        "materializing durable saved data as a value; iterate it directly",
        span,
    )
}

fn single_path_arg<'a>(
    args: &'a [ExecArg],
    builtin: &str,
    span: SourceSpan,
) -> Result<&'a ExecExpr, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            format!("`{builtin}` takes one argument"),
            span,
        ));
    };
    Ok(&path.value)
}

pub(crate) fn eval_keys(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let path = single_path_arg(args, "keys", span)?;
    if let Some(keys) = enumerate_local_keys_call_arg(path, span, env)? {
        return Ok(Value::Sequence(Sequence::dense(keys)));
    }
    check_key_collection(path, span)?;
    Err(durable_collection_value(span))
}

pub(crate) fn eval_values(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    eval_values_materialized(single_path_arg(args, "values", span)?, span, env)
}

/// Materialize a local keyed collection as a value sequence. Durable saved data is
/// never materialized as a value; iterate it directly.
fn eval_values_materialized(
    path: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if path.saved_place().is_some() {
        return Err(durable_collection_value(span));
    }
    let rows = materialize_local_collection_dir(eval_expr(path, env)?, Direction::Ascending, span)?;
    Ok(Value::Sequence(Sequence::dense(
        rows.into_iter().map(|(_, value)| value).collect(),
    )))
}
