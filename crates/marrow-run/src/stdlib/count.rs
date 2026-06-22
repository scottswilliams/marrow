use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use crate::call::expression_absent_at_resolution_site;
use crate::env::Env;
use crate::error::{RuntimeError, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::local_collection::local_collection_count;
use crate::path::{Terminal, direct_root_place, lower_for_probe, saved_path_present};
use crate::read::{count_iterable_index_branch, count_iterable_layer, validated_data_child_count};
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
    if arg.value.saved_place().is_some() {
        return Ok(Value::Bool(saved_path_present(&arg.value, span, env)?));
    }
    match eval_expr(&arg.value, env) {
        Ok(_) => Ok(Value::Bool(true)),
        Err(error) if expression_absent_at_resolution_site(&arg.value, &error) => {
            Ok(Value::Bool(false))
        }
        Err(error) => Err(error),
    }
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
        return local_collection_count(&arg.value, span, env);
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
    // A position that addresses no node — non-positive or otherwise unlowerable —
    // counts as 0, the same as a positive out-of-range hole.
    let Some(target) = count_target(&arg.value, span, env)? else {
        return Ok(Value::Int(0));
    };
    let children = match &target.child_layer {
        Some(child_layer) => validated_data_child_count(
            env.store,
            &target.address,
            &child_layer.key_scalars,
            child_layer.exact_key_count,
            span,
        )?,
        None => data_child_count(env.store, &target.address, span)?,
    };
    let count = if children > 0 {
        children
    } else {
        data_exists(env.store, &target.address, span)? as usize
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

struct CountTarget {
    address: DataAddress,
    child_layer: Option<CountChildLayer>,
}

struct CountChildLayer {
    key_scalars: Vec<Option<marrow_store::value::ScalarType>>,
    exact_key_count: usize,
}

fn count_target(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<CountTarget>, RuntimeError> {
    let Some(path) = lower_for_probe(expr, env)? else {
        return Ok(None);
    };
    let mut child_layer = None;
    let address = match &path.terminal {
        Terminal::Record => {
            if path.layer_addresses.is_empty() {
                DataAddress::record(&path.place, &path.identity, span)
            } else {
                if let (Some(layer), Some(layer_address)) =
                    (path.place.layers.last(), path.layer_addresses.last())
                    && layer_address.keys.len() < layer.key_params.len()
                {
                    child_layer = Some(CountChildLayer {
                        key_scalars: layer.key_params.iter().map(|param| param.scalar).collect(),
                        exact_key_count: layer_address.keys.len(),
                    });
                }
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
    }?;
    Ok(Some(CountTarget {
        address,
        child_layer,
    }))
}
