use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::local_collection::local_collection_count;
use crate::path::{Terminal, direct_root_place, lower, saved_path_present};
use crate::read::{count_iterable_index_branch, count_iterable_layer};
use crate::stdlib::exact_unique_index_lookup_value;
use crate::store::{DataAddress, data_child_count, data_exists};
use crate::value::Value;

pub(crate) fn eval_exists(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`exists` takes one argument", span));
    };
    Ok(Value::Bool(saved_path_present(&arg.value, span, env)?))
}

pub(crate) fn eval_count(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`count` takes one argument", span));
    };
    if arg.value.saved_place().is_none() {
        return local_collection_count(eval_expr(&arg.value, env)?, span);
    }
    if let Some(count) = direct_primary_root_count(&arg.value, span, env)? {
        return Ok(Value::Int(count));
    }
    if let Some(value) = exact_unique_index_lookup_value(&arg.value, span, env)? {
        return Ok(Value::Int(value.count()));
    }
    if let Some(entries) = count_iterable_index_branch(&arg.value, env)? {
        return Ok(Value::Int(usize_to_i64(entries, span)?));
    }
    let address = count_address(&arg.value, span, env)?;
    let children = data_child_count(env.store, &address, span)?;
    let count = if children > 0 {
        children
    } else {
        data_exists(env.store, &address, span)? as usize
    };
    Ok(Value::Int(count as i64))
}

fn direct_primary_root_count(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<i64>, RuntimeError> {
    let Some(place) = direct_root_place(expr) else {
        return Ok(None);
    };
    match place.identity_keys.len() {
        0 => Ok(None),
        _ => usize_to_i64(count_iterable_layer(expr, env)?, span).map(Some),
    }
}

fn usize_to_i64(count: usize, span: SourceSpan) -> Result<i64, RuntimeError> {
    i64::try_from(count).map_err(|_| overflow(span))
}

fn count_address(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<DataAddress, RuntimeError> {
    let path = lower(expr, env)?;
    match &path.terminal {
        Terminal::Record => {
            if path.layer_addresses.is_empty() {
                DataAddress::record(&path.place, &path.identity, span)
            } else {
                DataAddress::layer_prefix(&path.place, &path.identity, &path.layer_addresses, span)
            }
        }
        Terminal::Field { catalog_id, .. } => DataAddress::member(
            &path.place,
            &path.identity,
            &path.layer_addresses,
            catalog_id,
            span,
        ),
        Terminal::Index => Err(unsupported("counting this index path", span)),
    }
}
