use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, absent_read};
use crate::env::Env;
use crate::error::{RUN_ABSENT, RuntimeError, assign_error, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::read::reversed_argument;
use crate::value::{LocalTreeEntry, Sequence, Value, saved_key_to_value, value_to_key};

pub(crate) fn eval_local_collection_read(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match env.lookup(name).cloned() {
        Some(Value::Sequence(items)) => read_local_sequence(&items, args, span, env).map(Some),
        Some(Value::LocalTree(entries)) => {
            let keys = eval_local_keys(args, span, env)?;
            entries
                .into_iter()
                .find(|entry| entry.keys == keys)
                .map(|entry| entry.value)
                .ok_or_else(|| absent_read("`local tree` is absent".into(), span))
                .map(Some)
        }
        _ => Ok(None),
    }
}

pub(crate) fn eval_local_collection_write(
    name: &str,
    args: &[ExecArg],
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    match env.lookup(name).cloned() {
        Some(Value::Sequence(mut items)) => {
            let position = eval_local_sequence_position(args, span, env)?;
            let value = eval_expr(value, env)?;
            items.set(position, value);
            env.assign(name, Value::Sequence(items))
                .map_err(|error| assign_error(name, error, span))?;
            Ok(true)
        }
        Some(Value::LocalTree(mut entries)) => {
            let keys = eval_local_keys(args, span, env)?;
            let value = eval_expr(value, env)?;
            if let Some(entry) = entries.iter_mut().find(|entry| entry.keys == keys) {
                entry.value = value;
            } else {
                entries.push(LocalTreeEntry { keys, value });
                entries.sort_by(|left, right| left.keys.cmp(&right.keys));
            }
            env.assign(name, Value::LocalTree(entries))
                .map_err(|error| assign_error(name, error, span))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Delete an entry from a local collection by position or key. A delete names a node
/// to remove, so a hole, an out-of-range position, or a non-positive position
/// addresses no node and is a tolerant no-op, the same as deleting any absent saved
/// position. A sequence delete leaves a hole; append never reuses it.
pub(crate) fn eval_local_collection_delete(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    match env.lookup(name).cloned() {
        Some(Value::Sequence(mut items)) => {
            match eval_local_sequence_position(args, span, env) {
                Ok(position) => {
                    items.remove(position);
                }
                // A non-positive position addresses no node, so its delete is a no-op.
                Err(error) if error.code() == RUN_ABSENT && error.is_catchable() => {}
                Err(error) => return Err(error),
            }
            env.assign(name, Value::Sequence(items))
                .map_err(|error| assign_error(name, error, span))
        }
        Some(Value::LocalTree(mut entries)) => {
            let keys = eval_local_keys(args, span, env)?;
            entries.retain(|entry| entry.keys != keys);
            env.assign(name, Value::LocalTree(entries))
                .map_err(|error| assign_error(name, error, span))
        }
        _ => Err(unsupported("deleting from this local value", span)),
    }
}

pub(crate) fn local_collection_count(
    value: Value,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match value {
        Value::Sequence(items) => i64::try_from(items.len())
            .map(Value::Int)
            .map_err(|_| overflow(span)),
        Value::LocalTree(entries) => i64::try_from(entries.len())
            .map(Value::Int)
            .map_err(|_| overflow(span)),
        _ => Err(unsupported("counting this value", span)),
    }
}

pub(crate) fn enumerate_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<Value>, RuntimeError> {
    let mut keys: Vec<Value> = match value {
        Value::Sequence(items) => items.positions().map(Value::Int).collect(),
        Value::LocalTree(entries) => {
            let mut seen = Vec::<SavedKey>::new();
            for entry in entries {
                let Some(key) = entry.keys.first().cloned() else {
                    continue;
                };
                if !seen.contains(&key) {
                    seen.push(key);
                }
            }
            seen.into_iter()
                .map(|key| saved_key_to_value(key, span))
                .collect::<Result<_, _>>()?
        }
        _ => return Err(unsupported("keys over this value", span)),
    };
    apply_direction(&mut keys, dir);
    Ok(keys)
}

pub(crate) fn enumerate_local_keys_call_arg(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    if let Some(inner) = reversed_argument(arg) {
        if inner.saved_place().is_some() {
            return Ok(None);
        }
        return enumerate_keys_over_reversed(eval_expr(inner, env)?, span).map(Some);
    }
    if arg.saved_place().is_some() {
        return Ok(None);
    }
    enumerate_local_collection_dir(eval_expr(arg, env)?, Direction::Ascending, span).map(Some)
}

pub(crate) fn enumerate_reversed_local_keys_call_arg(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    let Some(mut keys) = enumerate_local_keys_call_arg(arg, span, env)? else {
        return Ok(None);
    };
    keys.reverse();
    Ok(Some(keys))
}

fn enumerate_keys_over_reversed(
    value: Value,
    span: SourceSpan,
) -> Result<Vec<Value>, RuntimeError> {
    let dir = match &value {
        Value::LocalTree(_) => Direction::Descending,
        Value::Sequence(_) => Direction::Ascending,
        _ => {
            return Err(unsupported(
                "reversing this value (expected an iterable)",
                span,
            ));
        }
    };
    enumerate_local_collection_dir(value, dir, span)
}

pub(crate) fn materialize_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    let mut rows = match value {
        Value::Sequence(items) => items
            .rows()
            .iter()
            .map(|(position, value)| (Value::Int(*position), value.clone()))
            .collect(),
        Value::LocalTree(entries) => entries
            .into_iter()
            .map(|entry| {
                let key = entry.keys.first().cloned().ok_or_else(|| {
                    unsupported("entries over a local tree with no key column", span)
                })?;
                Ok((saved_key_to_value(key, span)?, entry.value))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?,
        _ => return Err(unsupported("values/entries over this value", span)),
    };
    apply_direction(&mut rows, dir);
    Ok(rows)
}

/// Reverse the rows for a descending walk. The whole row reverses as one, so keyed
/// entries stay paired with their values.
fn apply_direction<T>(rows: &mut [T], dir: Direction) {
    if dir == Direction::Descending {
        rows.reverse();
    }
}

fn read_local_sequence(
    items: &Sequence,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let position = eval_local_sequence_position(args, span, env)?;
    items
        .get(position)
        .cloned()
        .ok_or_else(|| absent_read("`local sequence` is absent".into(), span))
}

/// The 1-based integer position a local-sequence lookup addresses. A non-int key is
/// a type fault; a zero or negative position addresses no node, so it raises the
/// catchable absent fault — a guarded read resolves it through `??`/`if const`/
/// `exists`/`catch`, and a write aborts before mutating the binding, keeping the
/// spec's "resolved at the read site" promise for every int position and matching
/// the saved side's non-positive rule.
fn eval_local_sequence_position(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<i64, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("a local sequence lookup takes one key", span));
    };
    reject_named_lookup_arg(arg, span)?;
    let Value::Int(position) = eval_expr(&arg.value, env)? else {
        return Err(type_error("a local sequence key must be an int", span));
    };
    if position < 1 {
        return Err(absent_read("`local sequence` is absent".into(), span));
    }
    Ok(position)
}

fn eval_local_keys(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    args.iter()
        .map(|arg| {
            reject_named_lookup_arg(arg, span)?;
            value_to_key(eval_expr(&arg.value, env)?, span)?
                .ok_or_else(|| unsupported("a key of this type", span))
        })
        .collect()
}

/// A local-collection lookup takes only positional value keys.
fn reject_named_lookup_arg(arg: &ExecArg, span: SourceSpan) -> Result<(), RuntimeError> {
    if arg.name.is_some() {
        return Err(unsupported(
            "named arguments in a local collection lookup",
            span,
        ));
    }
    Ok(())
}
