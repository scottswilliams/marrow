use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, absent_read, peel_reversed};
use crate::env::Env;
use crate::error::{RUN_ABSENT, RuntimeError, assign_error, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::value::{Value, saved_key_to_value, value_to_key};

pub(crate) fn eval_local_collection_read(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // Resolve the key against the environment first, then borrow the binding to read
    // a single element. Only that one value is cloned, never the whole collection.
    match env.lookup(name) {
        Some(Value::Sequence(_)) => {
            let position = eval_local_sequence_position(args, span, env)?;
            let Some(Value::Sequence(items)) = env.lookup(name) else {
                return Ok(None);
            };
            items
                .get(position)
                .cloned()
                .ok_or_else(|| absent_read("`local sequence` is absent".into(), span))
                .map(Some)
        }
        Some(Value::LocalTree(_)) => {
            let keys = eval_local_keys(args, span, env)?;
            let Some(Value::LocalTree(tree)) = env.lookup(name) else {
                return Ok(None);
            };
            tree.get(&keys)
                .cloned()
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
    // Resolve the key and the right-hand value before borrowing the binding, since
    // both evaluate against the environment. The binding is then mutated in place so
    // a write costs one node, not a deep copy of the whole collection.
    match env.lookup(name) {
        Some(Value::Sequence(_)) => {
            let position = eval_local_sequence_position(args, span, env)?;
            let value = eval_expr(value, env)?;
            let Value::Sequence(items) = mutable_local_collection(name, span, env)? else {
                return Ok(true);
            };
            items.set(position, value);
            Ok(true)
        }
        Some(Value::LocalTree(_)) => {
            let keys = eval_local_keys(args, span, env)?;
            reject_non_positive_sequence_key(&keys, span)?;
            let value = eval_expr(value, env)?;
            let Value::LocalTree(tree) = mutable_local_collection(name, span, env)? else {
                return Ok(true);
            };
            tree.insert(keys, value);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// A mutable borrow of `name`'s collection binding, surfacing an unbound or immutable
/// binding as the same assignment fault a whole-value reassignment would. Mutating in
/// place is what keeps a local-collection write proportional to one node rather than
/// cloning the whole collection.
fn mutable_local_collection<'v>(
    name: &str,
    span: SourceSpan,
    env: &'v mut Env<'_>,
) -> Result<&'v mut Value, RuntimeError> {
    env.lookup_mut(name)
        .map_err(|error| assign_error(name, error, span))
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
    match env.lookup(name) {
        Some(Value::Sequence(_)) => {
            let position = match eval_local_sequence_position(args, span, env) {
                Ok(position) => Some(position),
                // A non-positive position addresses no node, so its delete is a no-op.
                Err(error) if error.code() == RUN_ABSENT && error.is_catchable() => None,
                Err(error) => return Err(error),
            };
            if let (Some(position), Value::Sequence(items)) =
                (position, mutable_local_collection(name, span, env)?)
            {
                items.remove(position);
            }
            Ok(())
        }
        Some(Value::LocalTree(_)) => {
            let keys = eval_local_keys(args, span, env)?;
            if let Value::LocalTree(tree) = mutable_local_collection(name, span, env)? {
                tree.remove(&keys);
            }
            Ok(())
        }
        _ => Err(unsupported("deleting from this local value", span)),
    }
}

/// Count a local collection by borrowing it: reading the cardinality needs only
/// `len`, so a bound collection is never deep-cloned just to be measured. A bare
/// name is resolved against the environment in place; any other expression is
/// evaluated to a temporary value, which carries its own ownership.
pub(crate) fn local_collection_count(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if let Some(name) = local_collection_name(arg)
        && let Some(value @ (Value::Sequence(_) | Value::LocalTree(_))) = env.lookup(name)
    {
        return local_collection_len(value, span);
    }
    local_collection_len(&eval_expr(arg, env)?, span)
}

fn local_collection_len(value: &Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let len = match value {
        Value::Sequence(items) => items.len(),
        Value::LocalTree(tree) => tree.len(),
        _ => return Err(unsupported("counting this value", span)),
    };
    i64::try_from(len)
        .map(Value::Int)
        .map_err(|_| overflow(span))
}

/// The single unqualified binding name an expression names directly, if any. A
/// borrow of that binding reads a local collection without cloning it; anything
/// qualified or computed is not a plain binding reference.
fn local_collection_name(arg: &ExecExpr) -> Option<&str> {
    match arg {
        ExecExpr::Name {
            segments,
            enum_member: None,
            ..
        } => match segments.as_slice() {
            [name] => Some(name),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn enumerate_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<Value>, RuntimeError> {
    let mut keys: Vec<Value> = match value {
        Value::Sequence(items) => items.positions().map(Value::Int).collect(),
        Value::LocalTree(tree) => {
            // Rows already iterate in key-tuple order, so the distinct first-column keys
            // come out ascending; only collapse the runs a multi-column tree repeats.
            let mut seen = Vec::<SavedKey>::new();
            for (keys, _) in tree.rows() {
                let Some(key) = keys.first() else {
                    continue;
                };
                if seen.last() != Some(key) {
                    seen.push(key.clone());
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
            .map(|(position, value)| (Value::Int(position), value.clone()))
            .collect(),
        Value::LocalTree(tree) => tree
            .into_rows()
            .map(|(keys, value)| {
                let key = keys.into_iter().next().ok_or_else(|| {
                    unsupported("entries over a local tree with no key column", span)
                })?;
                Ok((saved_key_to_value(key, span)?, value))
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
