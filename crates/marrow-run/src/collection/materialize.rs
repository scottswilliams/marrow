use marrow_check::{
    CheckedBuiltinCall, CheckedCallTarget, CheckedExpr as ExecExpr, CheckedSavedPlace,
    CheckedSavedTerminal,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::durable_read::{
    LayerEntryAddress, read_layer_entry, read_layer_entry_at, read_resource,
};
use crate::env::Env;
use crate::error::{RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::local_collection::materialize_local_collection_dir;
use crate::path::{direct_root_place, lower};
use crate::read::enumerate_layer_dir;
use crate::stdlib::{check_key_collection, unique_index_lookup_values};
use crate::store::LayerAddress;
use crate::value::{Value, value_to_key};

use super::{Direction, ReadPosition};

pub(crate) enum MaterializeKind {
    Values,
    Entries,
}

pub(crate) struct ValuesOrEntries<'a> {
    pub(crate) layer: &'a ExecExpr,
    pub(crate) kind: MaterializeKind,
}

pub(crate) fn values_or_entries(expr: &ExecExpr) -> Option<ValuesOrEntries<'_>> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    let kind = match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Values) => MaterializeKind::Values,
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Entries) => MaterializeKind::Entries,
        _ => return None,
    };
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(ValuesOrEntries {
            layer: &arg.value,
            kind,
        }),
        _ => None,
    }
}

pub(crate) fn reversed_materialized(
    inner: ValuesOrEntries<'_>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let rows = if inner.layer.saved_place().is_some() {
        materialize_layer_dir(inner.layer, Direction::Descending, env)?
    } else {
        materialize_local_collection_dir(eval_expr(inner.layer, env)?, Direction::Descending, span)?
    };
    let values = match inner.kind {
        MaterializeKind::Values => rows.into_iter().map(|(_, value)| value).collect(),
        MaterializeKind::Entries => rows
            .into_iter()
            .map(|(key, value)| Value::Sequence(vec![key, value]))
            .collect(),
    };
    Ok(Value::Sequence(values))
}

pub(crate) fn reversed_keys(
    layer: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if layer.saved_place().is_none() {
        return Ok(Value::Sequence(
            crate::local_collection::enumerate_local_collection_dir(
                eval_expr(layer, env)?,
                Direction::Descending,
                span,
            )?,
        ));
    }
    check_key_collection(layer, span, env)?;
    Ok(Value::Sequence(enumerate_layer_dir(
        layer,
        Direction::Descending,
        env,
    )?))
}

pub(crate) fn reversed_saved(
    path: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if let Some(values) = unique_index_lookup_values(path, span, Direction::Descending, env)? {
        return Ok(Value::Sequence(values));
    }
    Ok(Value::Sequence(enumerate_layer_dir(
        path,
        Direction::Descending,
        env,
    )?))
}

pub(crate) fn materialize_layer(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    materialize_layer_dir(path, Direction::Ascending, env)
}

pub(crate) fn materialize_layer_dir(
    path: &ExecExpr,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    if path.saved_place().is_none() {
        return materialize_local_collection_dir(eval_expr(path, env)?, dir, path.span());
    }
    let keys = enumerate_layer_dir(path, dir, env)?;
    if let Some(place) = direct_root_place(path) {
        return materialize_root_children(place, keys, path.span(), env);
    }
    if let Some(place) = path.saved_place()
        && matches!(place.terminal, CheckedSavedTerminal::Index { .. })
    {
        return materialize_root_children(place, keys, path.span(), env);
    }
    materialize_child_layer(path, keys, env)
}

fn materialize_root_children(
    place: &CheckedSavedPlace,
    keys: Vec<Value>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    keys.into_iter()
        .map(|key| {
            let identity = identity_keys(&key, span)?;
            let value = read_resource(place, &identity, span, env)?;
            Ok((key, value))
        })
        .collect()
}

fn materialize_child_layer(
    path: &ExecExpr,
    keys: Vec<Value>,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    match path {
        ExecExpr::Field { base, .. } => {
            let span = path.span();
            let base_path = lower(base, env)?;
            let identity = base_path.identity.clone();
            let parent_layers = base_path.layer_addresses.clone();
            let Some(layer_facts) = path.saved_place().and_then(|place| place.layers.last()) else {
                return Err(unsupported("values/entries over this path", span));
            };
            let Some(place) = path.saved_place() else {
                return Err(unsupported("values/entries over this path", span));
            };
            keys.into_iter()
                .map(|key| {
                    let layer_key = value_to_key(key.clone())
                        .ok_or_else(|| unsupported("a key of this type", span))?;
                    let mut layers = parent_layers.clone();
                    layers.push(LayerAddress::from_checked(layer_facts, vec![layer_key]));
                    let value = if layers.len() == 1 {
                        read_layer_entry(
                            place,
                            &identity,
                            layer_facts,
                            &layers[0].keys,
                            ReadPosition::Materialization,
                            span,
                            env,
                        )?
                    } else {
                        read_layer_entry_at(
                            LayerEntryAddress {
                                place,
                                identity: &identity,
                                layers: &layers,
                                layer_facts,
                            },
                            ReadPosition::Materialization,
                            span,
                            env,
                        )?
                    };
                    Ok((key, value))
                })
                .collect()
        }
        other => Err(unsupported(
            "values/entries over this path (use keys(...) or direct iteration)",
            other.span(),
        )),
    }
}

fn identity_keys(key: &Value, span: SourceSpan) -> Result<Vec<SavedKey>, RuntimeError> {
    match key {
        Value::Identity(keys) => Ok(keys.clone()),
        other => Ok(vec![
            value_to_key(other.clone()).ok_or_else(|| unsupported("a key of this type", span))?,
        ]),
    }
}
