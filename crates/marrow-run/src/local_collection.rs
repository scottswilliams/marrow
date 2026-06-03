use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_store::key::{SavedKey, encode_key_value};
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, ReadPosition, absent_read};
use crate::env::Env;
use crate::error::{RuntimeError, assign_error, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::value::{LocalTreeEntry, Value, saved_key_to_value, value_to_key};

pub(crate) fn eval_local_collection_read(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match env.lookup(name).cloned() {
        Some(Value::Sequence(items)) => read_local_sequence(items, args, span, env).map(Some),
        Some(Value::LocalTree(entries)) => {
            let keys = eval_local_keys(args, span, env)?;
            entries
                .into_iter()
                .find(|entry| entry.keys == keys)
                .map(|entry| entry.value)
                .ok_or_else(|| {
                    absent_read(ReadPosition::Value, "`local tree` is absent".into(), span)
                })
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
            let index = eval_local_sequence_index(args, span, env)?;
            let value = eval_expr(value, env)?;
            if index == items.len() {
                items.push(value);
            } else if let Some(slot) = items.get_mut(index) {
                *slot = value;
            } else {
                return Err(unsupported(
                    "writing a sparse local sequence position",
                    span,
                ));
            }
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
                entries.sort_by_key(|entry| local_key_sort_key(&entry.keys));
            }
            env.assign(name, Value::LocalTree(entries))
                .map_err(|error| assign_error(name, error, span))?;
            Ok(true)
        }
        _ => Ok(false),
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
    let mut keys = match value {
        Value::Sequence(items) => (1..=items.len())
            .map(|pos| {
                i64::try_from(pos)
                    .map(Value::Int)
                    .map_err(|_| overflow(span))
            })
            .collect::<Result<Vec<_>, _>>()?,
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
                .map(saved_key_to_value)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| unsupported("iterating keys of this type", span))?
        }
        _ => return Err(unsupported("keys over this value", span)),
    };
    if dir == Direction::Descending {
        keys.reverse();
    }
    Ok(keys)
}

pub(crate) fn materialize_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    let mut rows = match value {
        Value::Sequence(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let pos = i64::try_from(index + 1).map_err(|_| overflow(span))?;
                Ok((Value::Int(pos), value))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?,
        Value::LocalTree(entries) => entries
            .into_iter()
            .map(|entry| {
                let key = entry
                    .keys
                    .first()
                    .cloned()
                    .and_then(saved_key_to_value)
                    .ok_or_else(|| unsupported("iterating keys of this type", span))?;
                Ok((key, entry.value))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?,
        _ => return Err(unsupported("values/entries over this value", span)),
    };
    if dir == Direction::Descending {
        rows.reverse();
    }
    Ok(rows)
}

fn read_local_sequence(
    items: Vec<Value>,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let index = eval_local_sequence_index(args, span, env)?;
    items.get(index).cloned().ok_or_else(|| {
        absent_read(
            ReadPosition::Value,
            "`local sequence` is absent".into(),
            span,
        )
    })
}

fn eval_local_sequence_index(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("a local sequence lookup takes one key", span));
    };
    if arg.mode.is_some() || arg.name.is_some() {
        return Err(unsupported(
            "named or out arguments in a local collection lookup",
            span,
        ));
    }
    let Value::Int(pos) = eval_expr(&arg.value, env)? else {
        return Err(type_error("a local sequence key must be an int", span));
    };
    if pos < 1 {
        return Err(type_error("a local sequence key must be positive", span));
    }
    usize::try_from(pos - 1).map_err(|_| overflow(span))
}

fn eval_local_keys(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    args.iter()
        .map(|arg| {
            if arg.mode.is_some() || arg.name.is_some() {
                return Err(unsupported(
                    "named or out arguments in a local collection lookup",
                    span,
                ));
            }
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("a key of this type", span))
        })
        .collect()
}

fn local_key_sort_key(keys: &[SavedKey]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for key in keys {
        bytes.extend(encode_key_value(key));
    }
    bytes
}
