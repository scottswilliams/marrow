//! Sequence and keyed builtins: keys/values/entries/reversed/neighbor/append.

use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use self::local::enumerate_local_collection_dir;
use crate::env::Env;
use crate::error::{RUN_ABSENT, RUN_TYPE, RuntimeError, raise_fault, unsupported};
use crate::expr::eval_expr;
use crate::read::keys_argument;
use crate::stdlib::check_key_collection;
use crate::value::{Sequence, Value};

mod append;
mod local;
mod materialize;

pub(crate) use append::{eval_append, eval_next_id};
pub(crate) use local::{
    enumerate_local_keys_call_arg, enumerate_reversed_local_keys_call_arg,
    eval_local_collection_delete, eval_local_collection_read, eval_local_collection_write,
    eval_local_collection_write_value, local_collection_count, materialize_local_collection_dir,
    resolve_local_collection_target,
};
pub(crate) use materialize::{
    MaterializeKind, reversed_keys, reversed_materialized, values_or_entries,
};

/// The order a saved-layer walk yields its children. A descending walk reverses as
/// one, so a composite identity is true-reversed at every level, not only its
/// outermost component.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Ascending,
    Descending,
}

impl Direction {
    pub(crate) fn flip(self) -> Self {
        match self {
            Direction::Ascending => Direction::Descending,
            Direction::Descending => Direction::Ascending,
        }
    }
}

/// Peel every `reversed(...)` wrapper off an iterable, composing their directions by
/// parity so the metamorphic identity `reversed(reversed(x)) == x` holds for any
/// nesting depth. Returns the innermost non-reversed expression and the net walk
/// direction. A `reversed` over a keyed collection must not be evaluated down to a
/// collapsed value here; peeling to the base preserves its key/value pairing.
pub(crate) fn peel_reversed(expr: &ExecExpr) -> (&ExecExpr, Direction) {
    let mut current = expr;
    let mut direction = Direction::Ascending;
    while let Some(inner) = crate::read::reversed_argument(current) {
        current = inner;
        direction = direction.flip();
    }
    (current, direction)
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

pub(crate) fn eval_entries(
    args: &[ExecArg],
    span: SourceSpan,
    _env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let _ = single_path_arg(args, "entries", span)?;
    Err(unsupported(
        "entries(...) is only valid in a two-name loop head",
        span,
    ))
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
        Value::Sequence(items) => {
            let mut values = items.into_values();
            values.reverse();
            Ok(Value::Sequence(Sequence::dense(values)))
        }
        Value::LocalTree(entries) => Ok(Value::Sequence(Sequence::dense(
            enumerate_local_collection_dir(Value::LocalTree(entries), Direction::Descending, span)?,
        ))),
        _ => Err(unsupported(
            "reversing this value (expected an iterable)",
            span,
        )),
    }
}
