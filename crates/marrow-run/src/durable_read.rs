//! Durable saved-place reads and materialization.

use marrow_check::{
    CheckedArg, CheckedExpr as ExecExpr, CheckedRuntimeProgram, CheckedSavedLayer,
    CheckedSavedMember, CheckedSavedPlace, CheckedSavedTerminal,
};
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_syntax::SourceSpan;

use crate::collection::{ReadPosition, absent_read};
use crate::env::Env;
use crate::error::{Located, RUN_TYPE, RUN_UNSUPPORTED, RuntimeError, type_error, unsupported};
use crate::expr::eval_expr;
use crate::path::{lower, lower_keys};
use crate::read::eval_local_field_get;
use crate::store::{DataAddress, IndexAddress, LayerAddress, read_data};
use crate::value::{Value, decode_leaf, identity_value, value_to_key};

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
    let lookup = checked_unique_index_lookup(place, span)?;
    let keys = index_lookup_keys(lookup, span, env)?;
    let address = IndexAddress::from_place(place, lookup.name, keys, span)?;
    let page = env
        .store
        .scan_index_tuple(&address.index, &address.keys, 1)
        .map_err(|error| error.located(span))?;
    let Some(entry) = page.entries.first() else {
        return Err(absent_read(
            ReadPosition::Value,
            format!("`{}` has no entry for that key", lookup.name),
            span,
        ));
    };
    decode_identity_payload_arity(&entry.value, place.identity_keys.len())
        .map(|keys| identity_value(&place.root, keys))
        .ok_or_else(|| RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!(
                "the `{}` index entry did not decode to an identity",
                lookup.name
            ),
            span,
        })
}

#[derive(Clone, Copy)]
struct IndexLookup<'a> {
    name: &'a str,
    args: &'a [CheckedArg],
    arg_count: usize,
}

fn checked_unique_index_lookup<'a>(
    place: &'a CheckedSavedPlace,
    span: SourceSpan,
) -> Result<IndexLookup<'a>, RuntimeError> {
    let CheckedSavedTerminal::Index {
        name,
        args,
        unique,
        arg_count,
        ..
    } = &place.terminal
    else {
        return Err(unsupported("a checked saved index lookup", span));
    };
    if !unique {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_UNSUPPORTED,
            message: format!(
                "non-unique index `{}` has no single identity in value position; \
                 iterate it with `keys(...)`",
                name
            ),
            span,
        });
    }
    if args.len() != *arg_count {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!(
                "unique index `{}` expects {} key argument(s), but {} were given",
                name,
                arg_count,
                args.len()
            ),
            span,
        });
    }
    Ok(IndexLookup {
        name,
        args,
        arg_count: *arg_count,
    })
}

fn index_lookup_keys(
    lookup: IndexLookup<'_>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    debug_assert_eq!(lookup.args.len(), lookup.arg_count);
    let mut keys = Vec::with_capacity(lookup.args.len());
    for arg in lookup.args {
        if arg.mode.is_some() || arg.name.is_some() {
            return Err(unsupported(
                "an index lookup with named or inout arguments",
                span,
            ));
        }
        keys.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", span))?,
        );
    }
    Ok(keys)
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
    decode_leaf(env.program, &bytes, leaf).ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_TYPE,
        message: format!(
            "stored value in `{}` did not decode to a runtime value",
            address.layer_facts.name
        ),
        span,
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
                    return Err(required_field_absent(&member.name, span));
                }
                continue;
            };
            let leaf = member
                .leaf
                .as_ref()
                .ok_or_else(|| unsupported("reading this field type", span))?;
            let value = decode_leaf(program, &bytes, leaf).ok_or_else(|| RuntimeError {
                throw: None,
                origin: None,
                code: RUN_TYPE,
                message: format!("stored value for `{}` did not decode", member.name),
                span,
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

fn required_field_absent(name: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError::fault(
        RUN_TYPE,
        format!("required stored field `{name}` is absent"),
        span,
    )
}
