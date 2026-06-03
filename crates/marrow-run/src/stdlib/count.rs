use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::Env;
use crate::error::{Located, RuntimeError, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::local_collection::local_collection_count;
use crate::path::{Terminal, direct_root_place, lower, saved_path_present};
use crate::read::enumerate_layer;
use crate::stdlib::{is_iterable_index_branch, unique_index_lookup_values};
use crate::store::{DataAddress, catalog_id, data_child_count, data_exists};
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
    if let Some(values) = unique_index_lookup_values(&arg.value, span, Direction::Ascending, env)? {
        return Ok(Value::Int(values.len() as i64));
    }
    if is_iterable_index_branch(&arg.value, env) {
        let entries = enumerate_layer(&arg.value, env)?.len();
        return Ok(Value::Int(entries as i64));
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
        1 => {
            let store = catalog_id(&place.store_catalog_id, "store", span)?;
            let count = env
                .store
                .record_child_count(&store, &[])
                .map_err(|error| error.located(span))?;
            i64::try_from(count).map(Some).map_err(|_| overflow(span))
        }
        _ => {
            let count = enumerate_layer(expr, env)?.len();
            i64::try_from(count).map(Some).map_err(|_| overflow(span))
        }
    }
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
