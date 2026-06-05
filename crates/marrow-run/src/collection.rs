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
    MaterializeKind, reversed_keys, reversed_materialized, reversed_saved, values_or_entries,
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

pub(crate) fn eval_keys(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`keys` takes one argument".into(),
            span,
        ));
    };
    if path.value.saved_place().is_none() {
        return Ok(Value::Sequence(enumerate_local_collection_dir(
            eval_expr(&path.value, env)?,
            Direction::Ascending,
            span,
        )?));
    }
    check_key_collection(&path.value, span)?;
    Err(durable_collection_value(span))
}

pub(crate) fn eval_values(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`values` takes one argument".into(),
            span,
        ));
    };
    if path.value.saved_place().is_none() {
        let values = materialize_local_collection_dir(
            eval_expr(&path.value, env)?,
            Direction::Ascending,
            span,
        )?
        .into_iter()
        .map(|(_, value)| value)
        .collect();
        return Ok(Value::Sequence(values));
    }
    Err(durable_collection_value(span))
}

pub(crate) fn eval_entries(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`entries` takes one argument".into(),
            span,
        ));
    };
    if path.value.saved_place().is_none() {
        let entries = materialize_local_collection_dir(
            eval_expr(&path.value, env)?,
            Direction::Ascending,
            span,
        )?
        .into_iter()
        .map(|(key, value)| Value::Sequence(vec![key, value]))
        .collect();
        return Ok(Value::Sequence(entries));
    }
    Err(durable_collection_value(span))
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
        return reversed_saved(span);
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
