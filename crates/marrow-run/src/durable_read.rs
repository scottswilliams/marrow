//! Durable saved-place reads and materialization.

use marrow_check::{
    CheckedExpr as ExecExpr, CheckedRuntimeProgram, CheckedSavedLayer, CheckedSavedMember,
    CheckedSavedPlace, CheckedSavedTerminal,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::{ReadPosition, absent_read};
use crate::env::Env;
use crate::error::{RUN_TYPE, RuntimeError, type_error, unsupported};
use crate::path::{lower, lower_keys};
use crate::read::eval_local_field_get;
use crate::stdlib::read_exact_unique_index_lookup_value;
use crate::store::{DataAddress, LayerAddress, read_data};
use crate::value::{Value, decode_leaf};

pub(crate) fn eval_saved_field(expr: &ExecExpr, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    let ExecExpr::Field { .. } = expr else {
        return Err(unsupported("this read", expr.span()));
    };
    read_saved_field(expr, expr.span(), env)
}

pub(crate) fn read_saved_field(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let path = lower(expr, env)?;
    if !matches!(path.terminal, crate::path::Terminal::Field { .. }) {
        return Err(unsupported("this read", span));
    }
    path.read(ReadPosition::Value, span, env)
}

pub(crate) fn eval_optional_field(
    expr: &ExecExpr,
    base: &ExecExpr,
    name: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let ExecExpr::OptionalField { .. } = expr else {
        return Err(unsupported("this read", span));
    };
    if base.saved_place().is_some() {
        read_saved_field(expr, span, env)
    } else {
        eval_local_field_get(base, name, span, env)
    }
}

pub(crate) fn eval_index_lookup(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    read_exact_unique_index_lookup_value(place, span, env)
}

pub(crate) fn eval_saved_layer_read(
    call: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let path = lower(call, env)?;
    let Some(layer_facts) = call.saved_place().and_then(|place| place.layers.last()) else {
        return Err(unsupported("this read", span));
    };
    read_layer_entry_at(
        LayerEntryAddress {
            place: &path.place,
            identity: &path.identity,
            layers: &path.layer_addresses,
            layer_facts,
        },
        ReadPosition::Value,
        span,
        env,
    )
}

pub(crate) fn read_layer_entry(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layer_facts: &CheckedSavedLayer,
    layer_keys: &[SavedKey],
    position: ReadPosition,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    read_layer_entry_at(
        LayerEntryAddress {
            place,
            identity,
            layers: &[LayerAddress::from_checked(layer_facts, layer_keys.to_vec())],
            layer_facts,
        },
        position,
        span,
        env,
    )
}

pub(crate) struct LayerEntryAddress<'a> {
    pub(crate) place: &'a CheckedSavedPlace,
    pub(crate) identity: &'a [SavedKey],
    pub(crate) layers: &'a [LayerAddress],
    pub(crate) layer_facts: &'a CheckedSavedLayer,
}

pub(crate) fn read_layer_entry_at(
    address: LayerEntryAddress<'_>,
    position: ReadPosition,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let entry = DataAddress::layer_prefix(address.place, address.identity, address.layers, span)?;
    let Some(leaf) = &address.layer_facts.leaf else {
        let fields = materialize_resource_members(
            env.program,
            address.place,
            address.identity,
            address.layers,
            &address.layer_facts.members,
            span,
            env,
        )?;
        return Ok(Value::Resource(fields));
    };
    let bytes = read_data(env.store, &entry, span)?;
    let Some(bytes) = bytes else {
        return Err(absent_read(
            position,
            format!("`{}` entry is absent", address.layer_facts.name),
            span,
        ));
    };
    decode_leaf(env.program, &bytes, leaf).ok_or_else(|| {
        RuntimeError::fault(
            RUN_TYPE,
            format!(
                "stored value in `{}` did not decode to a runtime value",
                address.layer_facts.name
            ),
            span,
        )
    })
}

pub(crate) fn eval_resource_read(
    call: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some(place) = call.saved_place() else {
        return Err(unsupported("this read", span));
    };
    if !matches!(place.terminal, CheckedSavedTerminal::Record) || !place.layers.is_empty() {
        return Err(unsupported("this read", span));
    }
    let identity = lower_keys(
        &place.identity_args,
        span,
        true,
        Some(&place.root),
        &place.identity_keys,
        env,
    )?;
    read_resource(place, &identity, span, env)
}

pub(crate) fn read_resource(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let arity = place.identity_keys.len();
    if identity.len() != arity {
        return Err(type_error(
            &format!(
                "`^{}` expects {arity} identity key(s), got {}",
                place.root,
                identity.len()
            ),
            span,
        ));
    }
    let fields =
        materialize_resource_members(env.program, place, identity, &[], &place.members, span, env)?;
    Ok(Value::Resource(fields))
}

fn materialize_resource_members(
    program: &CheckedRuntimeProgram,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    members: &[CheckedSavedMember],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<(String, Value)>, RuntimeError> {
    let mut fields = Vec::new();
    for member in members {
        if let Some((_, required)) = member.plain_field() {
            let address = DataAddress::member(place, identity, layers, &member.catalog_id, span)?;
            let bytes = read_data(env.store, &address, span)?;
            let Some(bytes) = bytes else {
                if required {
                    return Err(RuntimeError::fault(
                        RUN_TYPE,
                        format!("required stored field `{}` is absent", member.name),
                        span,
                    ));
                }
                continue;
            };
            let leaf = member
                .leaf
                .as_ref()
                .ok_or_else(|| unsupported("reading this field type", span))?;
            let value = decode_leaf(program, &bytes, leaf).ok_or_else(|| {
                RuntimeError::fault(
                    RUN_TYPE,
                    format!("stored value for `{}` did not decode", member.name),
                    span,
                )
            })?;
            fields.push((member.name.clone(), value));
        } else if member.is_unkeyed_group() {
            let mut nested_layers = layers.to_vec();
            nested_layers.push(LayerAddress {
                name: member.name.clone(),
                catalog_id: member.catalog_id.clone(),
                keys: Vec::new(),
            });
            let nested = materialize_resource_members(
                program,
                place,
                identity,
                &nested_layers,
                &member.group_members,
                span,
                env,
            )?;
            if !nested.is_empty() {
                fields.push((member.name.clone(), Value::Resource(nested)));
            }
        }
    }
    Ok(fields)
}
