use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, absent_read, peel_reversed};
use crate::env::Env;
use crate::error::{RUN_ABSENT, RuntimeError, assign_error, overflow, type_error, unsupported};
use crate::expr::eval_expr;
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
            reject_non_positive_sequence_key(&keys, span)?;
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

/// The catchable absent fault a write or read to a non-positive sequence position
/// raises, naming the position rather than the collection. A local sequence is
/// identical to a saved sequence, so this matches the saved guard's message.
fn non_positive_sequence_position(span: SourceSpan) -> RuntimeError {
    absent_read("a sequence position below 1 is absent".into(), span)
}

/// Reject a write to a single int-keyed tree at a position below 1. A single
/// int-keyed layer is a 1-based sequence, so a zero or negative position addresses
/// no node and must persist nothing; a composite or non-int key column carries
/// meaning in its own right and is left alone.
fn reject_non_positive_sequence_key(
    keys: &[SavedKey],
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if let [SavedKey::Int(position)] = keys
        && *position < 1
    {
        return Err(non_positive_sequence_position(span));
    }
    Ok(())
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

/// Enumerate the keys of a local collection in ascending key order, peeling any
/// `reversed(...)` wrappers by parity so a nested reversal composes correctly. A
/// saved place is not a local value and yields `None` for the caller to handle.
pub(crate) fn enumerate_local_keys_call_arg(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    enumerate_local_keys_in_dir(arg, Direction::Ascending, span, env)
}

/// Enumerate the keys of a local collection in descending key order, composing with
/// any `reversed(...)` wrappers so `reversed(reversed(x))` walks ascending again.
pub(crate) fn enumerate_reversed_local_keys_call_arg(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    enumerate_local_keys_in_dir(arg, Direction::Descending, span, env)
}

fn enumerate_local_keys_in_dir(
    arg: &ExecExpr,
    base_dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    let (inner, peeled) = peel_reversed(arg);
    if inner.saved_place().is_some() {
        return Ok(None);
    }
    // A descending base flips the parity the wrappers already composed.
    let dir = match base_dir {
        Direction::Ascending => peeled,
        Direction::Descending => peeled.flip(),
    };
    enumerate_local_collection_dir(eval_expr(inner, env)?, dir, span).map(Some)
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
        return Err(non_positive_sequence_position(span));
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
