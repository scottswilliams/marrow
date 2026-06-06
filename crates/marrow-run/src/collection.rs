//! Sequence and keyed builtins: keys/values/entries/reversed/neighbor/append.

use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RUN_ABSENT, RUN_TYPE, RuntimeError, raise_fault, unsupported};
use crate::expr::eval_expr;
use crate::local_collection::{enumerate_local_collection_dir, materialize_local_collection_dir};
use crate::read::keys_argument;
use crate::stdlib::check_key_collection;
use crate::value::Value;

mod append;
mod materialize;

pub(crate) use append::{eval_append, eval_next_id};
pub(crate) use materialize::{
    MaterializeKind, reversed_keys, reversed_materialized, values_or_entries,
};

/// Where a saved read sits, which decides how an absent element fails. A
/// value-position read (`^book(id).title` used as a value) raises a catchable
/// `run.absent_element` fault a `try`/`catch` can bind; materialization after an
/// address/key has already been chosen stays a plain fatal fault.
#[derive(Clone, Copy)]
pub(crate) enum ReadPosition {
    Value,
    Materialization,
}

/// The order a saved-layer walk yields its children. `for`/`keys`/`values`/
/// `entries` enumerate `Ascending`; `reversed(...)` enumerates `Descending`; and
/// `next`/`prev` seek the next/previous neighbor. The whole walk reverses as one,
/// so a composite identity is true-reversed at every level, not only its
/// outermost component.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Ascending,
    Descending,
}

/// The absent-element error for a read at `position`: catchable in value
/// position, plain fatal during materialization.
pub(crate) fn absent_read(
    position: ReadPosition,
    message: String,
    span: SourceSpan,
) -> RuntimeError {
    match position {
        ReadPosition::Value => raise_fault(RUN_ABSENT, message, span),
        ReadPosition::Materialization => RuntimeError::fault(RUN_ABSENT, message, span),
    }
}

pub(crate) fn durable_collection_value(span: SourceSpan) -> RuntimeError {
    unsupported(
        "materializing durable saved data as a value; iterate it directly",
        span,
    )
}

/// The single path argument a `keys`/`values`/`entries` builtin takes, or the per-builtin
/// arity fault naming `builtin`.
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
    if path.saved_place().is_none() {
        return Ok(Value::Sequence(enumerate_local_collection_dir(
            eval_expr(path, env)?,
            Direction::Ascending,
            span,
        )?));
    }
    check_key_collection(path, span)?;
    Err(durable_collection_value(span))
}

pub(crate) fn eval_values(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    eval_materialized(
        single_path_arg(args, "values", span)?,
        MaterializeKind::Values,
        span,
        env,
    )
}

pub(crate) fn eval_entries(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    eval_materialized(
        single_path_arg(args, "entries", span)?,
        MaterializeKind::Entries,
        span,
        env,
    )
}

/// Materialize a local keyed collection in ascending order as a value sequence, projecting
/// each `(key, value)` row by `kind`: `Values` yields the value, `Entries` yields a
/// `[key, value]` pair. Durable saved data is never materialized as a value.
fn eval_materialized(
    path: &ExecExpr,
    kind: MaterializeKind,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if path.saved_place().is_some() {
        return Err(durable_collection_value(span));
    }
    let rows = materialize_local_collection_dir(eval_expr(path, env)?, Direction::Ascending, span)?;
    let values = match kind {
        MaterializeKind::Values => rows.into_iter().map(|(_, value)| value).collect(),
        MaterializeKind::Entries => rows
            .into_iter()
            .map(|(key, value)| Value::Sequence(vec![key, value]))
            .collect(),
    };
    Ok(Value::Sequence(values))
}

pub(crate) fn eval_reversed(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`reversed` takes one argument".into(),
            span,
        ));
    };
    if let Some(inner) = values_or_entries(&arg.value) {
        return reversed_materialized(inner, span, env);
    }
    if let Some(layer) = keys_argument(&arg.value) {
        return reversed_keys(layer, span, env);
    }
    if arg.value.saved_place().is_some() {
        return Err(durable_collection_value(span));
    }
    reversed_in_memory(&arg.value, span, env)
}

fn reversed_in_memory(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Sequence(mut items) => {
            items.reverse();
            Ok(Value::Sequence(items))
        }
        Value::LocalTree(entries) => Ok(Value::Sequence(
            materialize_local_collection_dir(
                Value::LocalTree(entries),
                Direction::Descending,
                span,
            )?
            .into_iter()
            .map(|(_, value)| value)
            .collect(),
        )),
        _ => Err(unsupported(
            "reversing this value (expected an iterable)",
            span,
        )),
    }
}
