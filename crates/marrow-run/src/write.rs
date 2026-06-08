//! Managed writes over checked saved-store facts.

use marrow_check::{CheckedSavedMember, CheckedSavedPlace, StoreLeafKind};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;
use marrow_store::value::ValueError;
use marrow_syntax::SourceSpan;

use crate::index_maintenance::{
    FieldIndexRewrite, reject_field_unique_conflicts, reject_resource_unique_conflicts,
    stage_field_index_deletes, stage_field_index_rewrites, stage_resource_index_deletes,
    stage_resource_index_rewrites,
};
use crate::store::{
    DataAddress, IndexAddress, LayerAddress, data_exists, max_int_data_child, max_int_record_child,
    read_data,
};
use crate::value::LeafValue;
use crate::write_plan::{PlanStep, WritePlan};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceValue {
    pub fields: Vec<(String, LeafValue)>,
    pub identities: Vec<SuppliedIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuppliedIdentity {
    pub field: String,
    pub keys: Vec<SavedKey>,
    pub referenced_arity: usize,
}

impl ResourceValue {
    fn supplied_identity(&self, field: &str) -> Option<&SuppliedIdentity> {
        self.identities
            .iter()
            .find(|supplied| supplied.field == field)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    pub code: &'static str,
    pub message: String,
}

pub const WRITE_REQUIRED_ABSENT: &str = "write.required_absent";
pub const WRITE_TYPE_MISMATCH: &str = "write.type_mismatch";
pub const WRITE_IDENTITY_MISMATCH: &str = "write.identity_mismatch";
pub const WRITE_STORE: &str = "write.store";
pub const WRITE_UNKNOWN_FIELD: &str = "write.unknown_field";
pub const WRITE_UNIQUE_CONFLICT: &str = "write.unique_conflict";
pub const WRITE_UNKNOWN_LAYER: &str = "write.unknown_layer";
pub const WRITE_NOT_A_LEAF_LAYER: &str = "write.not_a_leaf_layer";
pub const WRITE_NOT_A_GROUP_LAYER: &str = "write.not_a_group_layer";
pub const WRITE_LAYER_KEY_ARITY: &str = "write.layer_key_arity";
pub const WRITE_ID_OVERFLOW: &str = "write.id_overflow";
pub const WRITE_NEXT_ID_UNSUPPORTED: &str = "write.next_id_unsupported";

impl From<marrow_store::StoreError> for WriteError {
    fn from(error: marrow_store::StoreError) -> Self {
        WriteError {
            code: WRITE_STORE,
            message: format!("the store could not be read while planning: {error}"),
        }
    }
}

impl From<ValueError> for WriteError {
    fn from(error: ValueError) -> Self {
        WriteError {
            code: error.code(),
            message: error.to_string(),
        }
    }
}

pub(crate) fn plan_resource_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    value: &ResourceValue,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let mut to_write = Vec::new();
    for field in materialized_plain_fields(&place.root_members) {
        let name = materialized_field_name(&field.path);
        match supplied_field(value, &name, field.leaf)? {
            Some(bytes) => to_write.push((field.path, bytes)),
            None if field.required => {
                return Err(WriteError {
                    code: WRITE_REQUIRED_ABSENT,
                    message: format!("required field `{name}` is absent"),
                });
            }
            None => {}
        }
    }

    reject_resource_unique_conflicts(place, identity, value, store, span)?;

    let mut steps = vec![PlanStep::DeleteData {
        address: data_address(place, identity, &[], &[], span)?,
    }];
    for (path, bytes) in to_write {
        steps.push(PlanStep::WriteData {
            address: data_address(place, identity, &[], &path, span)?,
            value: bytes,
        });
    }
    stage_resource_index_rewrites(&mut steps, place, identity, value, store, span)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_resource_delete(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let mut steps = vec![PlanStep::DeleteData {
        address: data_address(place, identity, &[], &[], span)?,
    }];
    stage_resource_index_deletes(&mut steps, place, identity, store, span)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_data_delete(address: DataAddress) -> Result<WritePlan, WriteError> {
    Ok(WritePlan {
        steps: vec![PlanStep::DeleteData { address }],
    })
}

/// Plan a bare data delete of one member under `layers`. The store address is
/// resolved eagerly here; the resulting plan is applied (and any resolution
/// error surfaced) later by `env.apply_plan`, after the required-field
/// maintenance guard in `delete_field`.
pub(crate) fn plan_member_delete(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    field: &str,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    plan_data_delete(data_address(
        place,
        identity,
        layers,
        &[field.to_string()],
        span,
    )?)
}

pub(crate) fn plan_store_root_delete(
    place: &CheckedSavedPlace,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    let mut steps = vec![PlanStep::DeleteRecordSubtree {
        address: DataAddress::record(place, &[], span).map_err(store_error)?,
    }];
    for index in &place.indexes {
        steps.push(PlanStep::DeleteIndexSubtree {
            address: IndexAddress::from_checked(&index.catalog_id, Vec::new(), span)
                .map_err(store_error)?,
        });
    }
    Ok(WritePlan { steps })
}

pub(crate) fn plan_field_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    value: &LeafValue,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let (_, leaf, _) =
        checked_field_slot(&place.root_members, field).ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", place.resource_name),
        })?;
    check_type(field, leaf, value)?;
    reject_field_unique_conflicts(place, identity, field, value, store, span)?;
    let mut steps = vec![PlanStep::WriteData {
        address: data_address(place, identity, &[], &[field.to_string()], span)?,
        value: value.bytes()?,
    }];
    stage_field_index_rewrites(
        &mut steps,
        FieldIndexRewrite {
            place,
            identity,
            field,
            value,
            store,
            span,
        },
    )?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_identity_field_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    keys: &[SavedKey],
    referenced_arity: usize,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let (_, leaf, _) =
        checked_field_slot(&place.root_members, field).ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", place.resource_name),
        })?;
    Ok(WritePlan {
        steps: vec![PlanStep::WriteData {
            address: data_address(place, identity, &[], &[field.to_string()], span)?,
            value: staged_identity_value(field, leaf, keys, referenced_arity)?,
        }],
    })
}

pub(crate) fn validate_required_fields_after_field_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    field: &str,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    let members = checked_members_for_layers(place, layers)?;
    if checked_field_slot(members, field).is_none() {
        return Err(WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", place.resource_name),
        });
    }
    for parent_len in 0..layers.len() {
        let parent = &layers[..parent_len];
        let parent_members = checked_members_for_layers(place, parent)?;
        let supplied = supplied_path_from_parent(layers, parent_len, field);
        ensure_required_fields_present(
            place,
            identity,
            parent,
            parent_members,
            Some(&supplied),
            store,
            span,
        )?;
    }
    ensure_required_fields_present(
        place,
        identity,
        layers,
        members,
        Some(&[field.to_string()]),
        store,
        span,
    )
}

fn supplied_path_from_parent(
    layers: &[LayerAddress],
    parent_len: usize,
    field: &str,
) -> Vec<String> {
    let mut path: Vec<String> = layers[parent_len..]
        .iter()
        .map(|layer| layer.name.clone())
        .collect();
    path.push(field.to_string());
    path
}

pub(crate) fn validate_required_fields_for_entry(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    exempt_layers: &[Vec<LayerAddress>],
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    let entry = DataAddress::layer_prefix(place, identity, layers, span).map_err(store_error)?;
    if !data_exists(store, &entry, span).map_err(store_error)? {
        return Ok(());
    }

    for parent_len in 0..layers.len() {
        let parent = &layers[..parent_len];
        if !entry_layers_exempt(exempt_layers, parent) {
            let members = checked_members_for_layers(place, parent)?;
            ensure_required_fields_present(place, identity, parent, members, None, store, span)?;
        }
    }

    if entry_layers_exempt(exempt_layers, layers) {
        return Ok(());
    }
    let members = checked_members_for_layers(place, layers)?;
    ensure_required_fields_present(place, identity, layers, members, None, store, span)
}

pub(crate) fn plan_field_delete(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    if checked_field_slot(&place.root_members, field).is_none() {
        return Err(WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", place.resource_name),
        });
    }
    let mut steps = vec![PlanStep::DeleteData {
        address: data_address(place, identity, &[], &[field.to_string()], span)?,
    }];
    stage_field_index_deletes(&mut steps, place, identity, field, store, span)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_layer_leaf_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    value: &LeafValue,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    let layer = leaf_layer(place, layers)?;
    let Some((leaf, _)) = layer.field() else {
        return Err(WriteError {
            code: WRITE_NOT_A_LEAF_LAYER,
            message: format!("keyed layer `{}` is a group, not a leaf", layer.name),
        });
    };
    check_type(&layer.name, leaf, value)?;
    Ok(WritePlan {
        steps: vec![PlanStep::WriteData {
            address: DataAddress::layer_prefix(place, identity, layers, span)
                .map_err(store_error)?,
            value: value.bytes()?,
        }],
    })
}

pub(crate) fn plan_layer_identity_leaf_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    keys: &[SavedKey],
    referenced_arity: usize,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    let layer = leaf_layer(place, layers)?;
    let Some((leaf, _)) = layer.field() else {
        return Err(WriteError {
            code: WRITE_NOT_A_LEAF_LAYER,
            message: format!("keyed layer `{}` is a group, not a leaf", layer.name),
        });
    };
    Ok(WritePlan {
        steps: vec![PlanStep::WriteData {
            address: DataAddress::layer_prefix(place, identity, layers, span)
                .map_err(store_error)?,
            value: staged_identity_value(&layer.name, leaf, keys, referenced_arity)?,
        }],
    })
}

pub(crate) fn plan_nested_field_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    field: &str,
    value: &LeafValue,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    let members = checked_members_for_layers(place, layers)?;
    let (_, leaf, _) = checked_field_slot(members, field).ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_FIELD,
        message: format!("group layer has no field `{field}`"),
    })?;
    check_type(field, leaf, value)?;
    Ok(WritePlan {
        steps: vec![PlanStep::WriteData {
            address: data_address(place, identity, layers, &[field.to_string()], span)?,
            value: value.bytes()?,
        }],
    })
}

pub(crate) fn plan_nested_identity_field_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    field: &str,
    keys: &[SavedKey],
    referenced_arity: usize,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    let members = checked_members_for_layers(place, layers)?;
    let (_, leaf, _) = checked_field_slot(members, field).ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_FIELD,
        message: format!("group layer has no field `{field}`"),
    })?;
    Ok(WritePlan {
        steps: vec![PlanStep::WriteData {
            address: data_address(place, identity, layers, &[field.to_string()], span)?,
            value: staged_identity_value(field, leaf, keys, referenced_arity)?,
        }],
    })
}

pub(crate) fn plan_layer_group_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    value: &ResourceValue,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    let group = group_layer(place, layers)?;
    let mut to_write = Vec::new();
    for field in materialized_plain_fields(&group.group_members) {
        let name = materialized_field_name(&field.path);
        match supplied_field(value, &name, field.leaf)? {
            Some(bytes) => to_write.push((field.path, bytes)),
            None if field.required => {
                return Err(WriteError {
                    code: WRITE_REQUIRED_ABSENT,
                    message: format!("required field `{name}` is absent"),
                });
            }
            None => {}
        }
    }
    let mut steps = vec![PlanStep::DeleteData {
        address: DataAddress::layer_prefix(place, identity, layers, span).map_err(store_error)?,
    }];
    for (path, bytes) in to_write {
        steps.push(PlanStep::WriteData {
            address: data_address(place, identity, layers, &path, span)?,
            value: bytes,
        });
    }
    Ok(WritePlan { steps })
}

pub(crate) fn next_id(
    place: &CheckedSavedPlace,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<i64, WriteError> {
    if !single_int_identity(place) {
        return Err(WriteError {
            code: WRITE_NEXT_ID_UNSUPPORTED,
            message: format!(
                "`nextId` has no default allocation policy for `{}`: {}; the default \
                 per-root policy is only available for a store with one `int` identity key",
                place.root, place.next_id_shape,
            ),
        });
    }
    let highest = max_int_record_child(store, place, &[], span)
        .map_err(store_error)?
        .unwrap_or(0);
    next_after(highest)
}

pub(crate) fn next_layer_pos(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    store: &TreeStore,
    span: SourceSpan,
) -> Result<i64, WriteError> {
    checked_layer(place, layers)?;
    let address = DataAddress::layer_prefix(place, identity, layers, span).map_err(store_error)?;
    let highest = max_int_data_child(store, &address, span)
        .map_err(store_error)?
        .filter(|&pos| pos >= 1)
        .unwrap_or(0);
    next_after(highest)
}

fn resolve_store_identity(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
) -> Result<(), WriteError> {
    if identity.len() != place.identity_keys.len() {
        return Err(WriteError {
            code: WRITE_IDENTITY_MISMATCH,
            message: format!(
                "store `^{}` expects {} identity key(s), got {}",
                place.root,
                place.identity_keys.len(),
                identity.len()
            ),
        });
    }
    Ok(())
}

fn next_after(highest: i64) -> Result<i64, WriteError> {
    highest.checked_add(1).ok_or_else(|| WriteError {
        code: WRITE_ID_OVERFLOW,
        message: "the integer key space is exhausted; the highest key is i64::MAX".into(),
    })
}

fn supplied_value<'a>(value: &'a ResourceValue, field: &str) -> Option<&'a LeafValue> {
    value
        .fields
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value)
}

fn supplied_field(
    value: &ResourceValue,
    field: &str,
    leaf: &StoreLeafKind,
) -> Result<Option<Vec<u8>>, WriteError> {
    if let StoreLeafKind::Identity { store_root, .. } = leaf {
        if let Some(supplied) = value.supplied_identity(field) {
            let staged =
                staged_identity_value(field, leaf, &supplied.keys, supplied.referenced_arity)?;
            return Ok(Some(staged));
        }
        return match supplied_value(value, field) {
            Some(saved) => {
                let key = saved.as_key().ok_or_else(|| WriteError {
                    code: WRITE_TYPE_MISMATCH,
                    message: format!(
                        "field `{field}` references `{store_root}`, but the value is not an identity"
                    ),
                })?;
                // A raw scalar value forms a single-key identity; its referenced
                // arity is the one key, which `staged_identity_value` then checks
                // against the leaf's declared arity.
                let keys = [key];
                Ok(Some(staged_identity_value(field, leaf, &keys, keys.len())?))
            }
            None => Ok(None),
        };
    }
    match supplied_value(value, field) {
        Some(saved) => {
            check_type(field, leaf, saved)?;
            Ok(Some(saved.bytes()?))
        }
        None => Ok(None),
    }
}

pub(crate) struct MaterializedField<'a> {
    pub(crate) path: Vec<String>,
    pub(crate) leaf: &'a StoreLeafKind,
    pub(crate) required: bool,
}

pub(crate) fn materialized_plain_fields(
    members: &[CheckedSavedMember],
) -> Vec<MaterializedField<'_>> {
    let mut fields = Vec::new();
    collect_materialized_plain_fields(members, &mut Vec::new(), &mut fields);
    fields
}

fn collect_materialized_plain_fields<'a>(
    members: &'a [CheckedSavedMember],
    prefix: &mut Vec<String>,
    fields: &mut Vec<MaterializedField<'a>>,
) {
    for member in members {
        if let Some((leaf, required)) = member.plain_field() {
            let mut path = prefix.clone();
            path.push(member.name.clone());
            fields.push(MaterializedField {
                path,
                leaf,
                required,
            });
        } else if member.is_unkeyed_group() {
            prefix.push(member.name.clone());
            collect_materialized_plain_fields(&member.group_members, prefix, fields);
            prefix.pop();
        }
    }
}

fn materialized_field_name(path: &[String]) -> String {
    path.join(".")
}

pub(crate) fn checked_field_slot<'a>(
    members: &'a [CheckedSavedMember],
    field: &str,
) -> Option<(&'a CheckedSavedMember, &'a StoreLeafKind, bool)> {
    members
        .iter()
        .find(|member| member.name == field)
        .and_then(|member| {
            member
                .plain_field()
                .map(|(leaf, required)| (member, leaf, required))
        })
}

fn staged_identity_value(
    name: &str,
    leaf: &StoreLeafKind,
    keys: &[SavedKey],
    referenced_arity: usize,
) -> Result<Vec<u8>, WriteError> {
    let StoreLeafKind::Identity { store_root, arity } = leaf else {
        return Err(WriteError {
            code: WRITE_TYPE_MISMATCH,
            message: format!("field `{name}` does not hold an identity"),
        });
    };
    if keys.len() != referenced_arity || keys.len() != *arity {
        return Err(WriteError {
            code: WRITE_TYPE_MISMATCH,
            message: format!(
                "field `{name}` references `{store_root}`, whose identity has \
                 {arity} key(s), but the value has {}",
                keys.len()
            ),
        });
    }
    Ok(encode_identity_payload(keys))
}

fn data_address(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    field: &[String],
    span: SourceSpan,
) -> Result<DataAddress, WriteError> {
    DataAddress::member_path(place, identity, layers, field, span).map_err(store_error)
}

fn ensure_required_fields_present(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    members: &[CheckedSavedMember],
    supplied: Option<&[String]>,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    for field in materialized_plain_fields(members) {
        if !field.required || supplied.is_some_and(|supplied| supplied == field.path.as_slice()) {
            continue;
        }
        let address = data_address(place, identity, layers, &field.path, span)?;
        if read_data(store, &address, span)
            .map_err(store_error)?
            .is_none()
        {
            return Err(WriteError {
                code: WRITE_REQUIRED_ABSENT,
                message: format!(
                    "required field `{}` is absent",
                    materialized_field_name(&field.path)
                ),
            });
        }
    }
    Ok(())
}

fn checked_members_for_layers<'a>(
    place: &'a CheckedSavedPlace,
    layers: &[LayerAddress],
) -> Result<&'a [CheckedSavedMember], WriteError> {
    let mut members = place.root_members.as_slice();
    for layer in layers {
        let Some(member) = members
            .iter()
            .find(|member| member.catalog_id == layer.catalog_id)
        else {
            return Err(WriteError {
                code: WRITE_UNKNOWN_LAYER,
                message: format!("checked layer `{}` is missing", layer.name),
            });
        };
        members = member.group_members.as_slice();
    }
    Ok(members)
}

/// Resolve the innermost addressed layer to its checked member and return that
/// member paired with the [`LayerAddress`] it resolved, so callers that need the
/// terminal layer's keys reuse the slot this already proved present instead of
/// re-deriving it.
fn checked_layer<'a, 'l>(
    place: &'a CheckedSavedPlace,
    layers: &'l [LayerAddress],
) -> Result<(&'a CheckedSavedMember, &'l LayerAddress), WriteError> {
    let Some(last) = layers.last() else {
        return Err(WriteError {
            code: WRITE_UNKNOWN_LAYER,
            message: "a keyed layer write needs a layer".into(),
        });
    };
    let parent = checked_members_for_layers(place, &layers[..layers.len() - 1])?;
    let member = parent
        .iter()
        .find(|member| member.catalog_id == last.catalog_id)
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_LAYER,
            message: format!("checked layer `{}` is missing", last.name),
        })?;
    Ok((member, last))
}

fn leaf_layer<'a>(
    place: &'a CheckedSavedPlace,
    layers: &[LayerAddress],
) -> Result<&'a CheckedSavedMember, WriteError> {
    let (layer, last) = checked_layer(place, layers)?;
    if !layer.key_params.is_empty() && layer.key_params.len() != last.keys.len() {
        return Err(WriteError {
            code: WRITE_LAYER_KEY_ARITY,
            message: format!(
                "keyed layer `{}` expects {} key(s), got {}",
                layer.name,
                layer.key_params.len(),
                last.keys.len()
            ),
        });
    }
    Ok(layer)
}

fn group_layer<'a>(
    place: &'a CheckedSavedPlace,
    layers: &[LayerAddress],
) -> Result<&'a CheckedSavedMember, WriteError> {
    let (layer, _) = checked_layer(place, layers)?;
    if layer.is_field() {
        return Err(WriteError {
            code: WRITE_NOT_A_GROUP_LAYER,
            message: format!("keyed layer `{}` is a leaf, not a group", layer.name),
        });
    }
    Ok(layer)
}

fn entry_layers_exempt(exempt_layers: &[Vec<LayerAddress>], layers: &[LayerAddress]) -> bool {
    exempt_layers
        .iter()
        .any(|exempt| exempt.as_slice() == layers)
}

fn check_type(field: &str, leaf: &StoreLeafKind, value: &LeafValue) -> Result<(), WriteError> {
    match (leaf, value) {
        (StoreLeafKind::Enum { .. }, value) if value.is_enum() => Ok(()),
        (StoreLeafKind::Scalar(expected), LeafValue::Scalar(value)) if *expected == value.ty() => {
            Ok(())
        }
        _ => Err(WriteError {
            code: WRITE_TYPE_MISMATCH,
            message: format!("field `{field}` has the wrong type"),
        }),
    }
}

fn single_int_identity(place: &CheckedSavedPlace) -> bool {
    place.identity_keys.as_slice().first().is_some_and(|key| {
        place.identity_keys.len() == 1 && key.scalar == Some(marrow_schema::ScalarType::Int)
    })
}

fn store_error(error: crate::error::RuntimeError) -> WriteError {
    WriteError {
        code: WRITE_STORE,
        message: error.message,
    }
}
