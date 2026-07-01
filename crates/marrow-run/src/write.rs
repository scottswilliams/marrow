//! Managed writes over checked saved-store facts.

use marrow_check::{
    CheckedFacts, CheckedSavedMember, CheckedSavedPlace, ResourceMemberId, StoreLeafKind,
    is_single_int_sequence,
};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;
use marrow_store::value::ValueError;
use marrow_syntax::SourceSpan;

use crate::index_maintenance::{
    IndexFieldPatch, IndexFieldPatchValue, IndexWriteContext, reject_field_patch_unique_conflicts,
    reject_field_unique_conflicts, reject_identity_field_unique_conflicts,
    reject_resource_unique_conflicts, stage_field_index_deletes, stage_field_index_rewrites,
    stage_field_patch_index_rewrites, stage_identity_field_index_rewrites,
    stage_identity_only_index_writes, stage_resource_index_deletes, stage_resource_index_rewrites,
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

pub(crate) enum FieldPatchValue {
    Leaf {
        member: ResourceMemberId,
        value: LeafValue,
    },
    Identity {
        member: ResourceMemberId,
        keys: Vec<SavedKey>,
        referenced_arity: usize,
    },
}

/// An identity value bound for an identity-typed place: the keys to store paired
/// with the arity the source referenced, so `staged_identity_value` can reject a
/// value whose key count disagrees with the place's declared identity.
#[derive(Clone, Copy)]
pub(crate) struct ReferencedIdentity<'a> {
    pub(crate) keys: &'a [SavedKey],
    pub(crate) referenced_arity: usize,
}

impl ResourceValue {
    fn supplied_identity(&self, field: &str) -> Option<&SuppliedIdentity> {
        self.identities
            .iter()
            .find(|supplied| supplied.field == field)
    }
}

/// A materialized plain field whose encoded bytes must be written at the
/// field's relative path.
struct FieldWrite {
    path: Vec<String>,
    bytes: Vec<u8>,
}

struct PatchFieldWrite {
    member_catalog_id: Option<String>,
    bytes: Vec<u8>,
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
pub const WRITE_INVALID_DATA: &str = "write.invalid_data";
pub const WRITE_UNKNOWN_FIELD: &str = "write.unknown_field";
pub const WRITE_UNIQUE_CONFLICT: &str = "write.unique_conflict";
pub const WRITE_UNKNOWN_LAYER: &str = "write.unknown_layer";
pub const WRITE_NOT_A_LEAF_LAYER: &str = "write.not_a_leaf_layer";
pub const WRITE_NOT_A_GROUP_LAYER: &str = "write.not_a_group_layer";
pub const WRITE_LAYER_KEY_ARITY: &str = "write.layer_key_arity";
pub const WRITE_ID_OVERFLOW: &str = "write.id_overflow";
pub const WRITE_NEXT_ID_UNSUPPORTED: &str = "write.next_id_unsupported";

/// Why a required field is still unset, which decides the actionable remedy.
/// A bare single-field write outside a transaction is rejected for the
/// still-absent siblings, so the guidance points the developer at grouping the
/// populating writes in a transaction. A field-by-field write inside a
/// transaction is already grouped; that same guidance would contradict itself,
/// so the commit-time check instead asks for the missing field before commit.
/// A whole-value or whole-entry assignment writes the record in one shot, so
/// neither grouping nor a later write can complete it; the guidance asks the
/// developer to include the missing field in the value being assigned. When a
/// nested-entry write leaves a *containing* record incomplete, the missing field
/// belongs to an ancestor the assigned value never carried, so the guidance points
/// at completing that containing record before commit instead of the assigned value.
#[derive(Clone, Copy)]
pub(crate) enum RequiredAbsentRemedy {
    OutsideTransaction,
    AtCommit,
    PopulateInValue,
    CompleteContainingRecord,
}

/// Whether a whole-value or group write enforces its required fields as it lands
/// or leaves the check to transaction commit. A write that commits on its own is
/// rejected immediately, since a missing required field can never be filled by a
/// later independent write. Inside a transaction the record can still be
/// completed before commit, so enforcement defers to the commit-time entry check,
/// which reports the actionable at-commit remedy rather than telling the developer
/// to group writes they have already grouped.
#[derive(Clone, Copy)]
pub(crate) enum RequiredEnforcement {
    Immediate,
    DeferToCommit,
}

impl RequiredEnforcement {
    /// A source write inside a `transaction` block defers its required-field check
    /// to commit; a write that commits on its own enforces immediately.
    pub(crate) fn for_transaction_depth(depth: usize) -> Self {
        if depth > 0 {
            RequiredEnforcement::DeferToCommit
        } else {
            RequiredEnforcement::Immediate
        }
    }
}

fn required_absent_error(name: &str, remedy: RequiredAbsentRemedy) -> WriteError {
    let guidance = match remedy {
        RequiredAbsentRemedy::OutsideTransaction => {
            "group the writes that populate this \
             record in a transaction block so every required field is set in one commit"
        }
        RequiredAbsentRemedy::AtCommit => {
            "set it before the transaction commits so the record is complete"
        }
        RequiredAbsentRemedy::PopulateInValue => {
            "include it in the assigned value so the record is complete"
        }
        RequiredAbsentRemedy::CompleteContainingRecord => {
            "write it on the containing record before the transaction commits so that record is complete"
        }
    };
    WriteError {
        code: WRITE_REQUIRED_ABSENT,
        message: format!("required field `{name}` is absent; {guidance}"),
    }
}

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
    facts: &CheckedFacts,
    span: SourceSpan,
    enforcement: RequiredEnforcement,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let index_context = IndexWriteContext::new(place, identity, store, facts, span);
    let to_write = collect_supplied_field_writes(&place.root_members, value, enforcement)?;

    reject_resource_unique_conflicts(index_context, value)?;

    let mut steps = vec![PlanStep::DeleteData {
        address: data_address(place, identity, &[], &[], span)?,
    }];
    steps.push(record_presence_step(place, identity, span)?);
    for FieldWrite { path, bytes } in to_write {
        steps.push(PlanStep::WriteData {
            address: data_address(place, identity, &[], &path, span)?,
            value: bytes,
        });
    }
    stage_resource_index_rewrites(&mut steps, index_context, value)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_resource_delete(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let index_context = IndexWriteContext::new(place, identity, store, facts, span);
    let mut steps = vec![PlanStep::DeleteData {
        address: data_address(place, identity, &[], &[], span)?,
    }];
    stage_resource_index_deletes(&mut steps, index_context)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_data_delete(address: DataAddress) -> Result<WritePlan, WriteError> {
    Ok(WritePlan {
        steps: vec![PlanStep::DeleteData { address }],
    })
}

/// The store address is resolved eagerly here, but the resulting plan (and any
/// resolution error) is carried into `delete_field` and only surfaced later by
/// `env.apply_plan`, after the required-field maintenance guard has run. This
/// ordering keeps the guard, not address resolution, as the first failure on
/// the delete path.
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
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let index_context = IndexWriteContext::new(place, identity, store, facts, span);
    let leaf = checked_field_leaf(&place.root_members, field).ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_FIELD,
        message: format!("resource `{}` has no field `{field}`", place.resource_name),
    })?;
    check_type(field, leaf, value)?;
    reject_field_unique_conflicts(index_context, field, value)?;
    let mut steps = record_establishing_steps(index_context)?;
    steps.push(PlanStep::WriteData {
        address: data_address(place, identity, &[], &[field.to_string()], span)?,
        value: value.bytes()?,
    });
    stage_field_index_rewrites(&mut steps, index_context, field, value)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_identity_field_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    value: ReferencedIdentity<'_>,
    store: &TreeStore,
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let index_context = IndexWriteContext::new(place, identity, store, facts, span);
    let leaf = checked_field_leaf(&place.root_members, field).ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_FIELD,
        message: format!("resource `{}` has no field `{field}`", place.resource_name),
    })?;
    reject_identity_field_unique_conflicts(index_context, field, value.keys)?;
    let mut steps = record_establishing_steps(index_context)?;
    steps.push(PlanStep::WriteData {
        address: data_address(place, identity, &[], &[field.to_string()], span)?,
        value: staged_identity_value(field, leaf, value.keys, value.referenced_arity)?,
    });
    stage_identity_field_index_rewrites(&mut steps, index_context, field, value.keys)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_field_patch_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    patch: &[FieldPatchValue],
    store: &TreeStore,
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let mut data_writes = Vec::with_capacity(patch.len());
    let mut index_patch = Vec::with_capacity(patch.len());
    for value in patch {
        match value {
            FieldPatchValue::Leaf { member, value } => {
                let field = checked_root_field_member(place, *member)?;
                let leaf = field_leaf(field)?;
                check_type(&field.name, leaf, value)?;
                data_writes.push(PatchFieldWrite {
                    member_catalog_id: field.catalog_id.clone(),
                    bytes: value.bytes()?,
                });
                index_patch.push(IndexFieldPatch {
                    member: *member,
                    value: IndexFieldPatchValue::Leaf(value.clone()),
                });
            }
            FieldPatchValue::Identity {
                member,
                keys,
                referenced_arity,
            } => {
                let field = checked_root_field_member(place, *member)?;
                let leaf = field_leaf(field)?;
                let bytes = staged_identity_value(&field.name, leaf, keys, *referenced_arity)?;
                data_writes.push(PatchFieldWrite {
                    member_catalog_id: field.catalog_id.clone(),
                    bytes: bytes.clone(),
                });
                index_patch.push(IndexFieldPatch {
                    member: *member,
                    value: IndexFieldPatchValue::IdentityBytes(bytes),
                });
            }
        }
    }

    let index_context = IndexWriteContext::new(place, identity, store, facts, span);
    reject_field_patch_unique_conflicts(index_context, &index_patch)?;
    let mut steps = record_establishing_steps(index_context)?;
    for PatchFieldWrite {
        member_catalog_id,
        bytes,
    } in data_writes
    {
        steps.push(PlanStep::WriteData {
            address: DataAddress::member(place, identity, &[], &member_catalog_id, span)
                .map_err(store_error)?,
            value: bytes,
        });
    }
    stage_field_patch_index_rewrites(&mut steps, index_context, &index_patch)?;
    Ok(WritePlan { steps })
}

fn checked_root_field_member(
    place: &CheckedSavedPlace,
    member: ResourceMemberId,
) -> Result<&CheckedSavedMember, WriteError> {
    place
        .root_members
        .iter()
        .find(|checked| checked.id == Some(member))
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no requested field", place.resource_name),
        })
}

fn field_leaf(member: &CheckedSavedMember) -> Result<&StoreLeafKind, WriteError> {
    member
        .plain_field()
        .map(|(leaf, _)| leaf)
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("member `{}` is not a plain field", member.name),
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
    if checked_field_leaf(members, field).is_none() {
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
            RequiredFieldEntry {
                place,
                identity,
                layers: parent,
            },
            parent_members,
            Some(&supplied),
            store,
            span,
            RequiredAbsentRemedy::OutsideTransaction,
        )?;
    }
    ensure_required_fields_present(
        RequiredFieldEntry {
            place,
            identity,
            layers,
        },
        members,
        Some(&[field.to_string()]),
        store,
        span,
        RequiredAbsentRemedy::OutsideTransaction,
    )
}

pub(crate) fn validate_required_fields_after_group_write(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    group_layer(place, layers)?;
    for parent_len in 1..layers.len() {
        if !layers[parent_len - 1].typed_entry {
            continue;
        }
        let parent = &layers[..parent_len];
        let parent_members = checked_members_for_layers(place, parent)?;
        let supplied = supplied_layer_path_from_parent(layers, parent_len);
        ensure_required_fields_present(
            RequiredFieldEntry {
                place,
                identity,
                layers: parent,
            },
            parent_members,
            Some(&supplied),
            store,
            span,
            RequiredAbsentRemedy::OutsideTransaction,
        )?;
    }
    Ok(())
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

fn supplied_layer_path_from_parent(layers: &[LayerAddress], parent_len: usize) -> Vec<String> {
    layers[parent_len..]
        .iter()
        .map(|layer| layer.name.clone())
        .collect()
}

/// `remedy` describes how the *assigned* entry was written and applies only to
/// that entry's own missing required fields, whose value the writer controls. An
/// ancestor entry left incomplete belongs to a containing record the assigned
/// value never carried, so it takes the containing-record remedy regardless of
/// how the assigned entry was written.
pub(crate) fn validate_required_fields_for_entry(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    exempt_layers: &[Vec<LayerAddress>],
    store: &TreeStore,
    span: SourceSpan,
    remedy: RequiredAbsentRemedy,
) -> Result<(), WriteError> {
    let entry = DataAddress::layer_prefix(place, identity, layers, span).map_err(store_error)?;
    if !data_exists(store, &entry, span).map_err(store_error)? {
        return Ok(());
    }

    for parent_len in 0..layers.len() {
        let parent = &layers[..parent_len];
        if !entry_layers_exempt(exempt_layers, parent) {
            let members = checked_members_for_layers(place, parent)?;
            ensure_required_fields_present(
                RequiredFieldEntry {
                    place,
                    identity,
                    layers: parent,
                },
                members,
                None,
                store,
                span,
                RequiredAbsentRemedy::CompleteContainingRecord,
            )?;
        }
    }

    if entry_layers_exempt(exempt_layers, layers) {
        return Ok(());
    }
    let members = checked_members_for_layers(place, layers)?;
    ensure_required_fields_present(
        RequiredFieldEntry {
            place,
            identity,
            layers,
        },
        members,
        None,
        store,
        span,
        remedy,
    )
}

pub(crate) fn plan_field_delete(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    store: &TreeStore,
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<WritePlan, WriteError> {
    resolve_store_identity(place, identity)?;
    let index_context = IndexWriteContext::new(place, identity, store, facts, span);
    if checked_field_leaf(&place.root_members, field).is_none() {
        return Err(WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", place.resource_name),
        });
    }
    let mut steps = vec![PlanStep::DeleteData {
        address: data_address(place, identity, &[], &[field.to_string()], span)?,
    }];
    stage_field_index_deletes(&mut steps, index_context, field)?;
    Ok(WritePlan { steps })
}

pub(crate) fn plan_layer_leaf_write(
    context: IndexWriteContext<'_>,
    layers: &[LayerAddress],
    value: &LeafValue,
) -> Result<WritePlan, WriteError> {
    let place = context.place();
    let layer = leaf_layer(place, layers)?;
    let Some((leaf, _)) = layer.field() else {
        return Err(WriteError {
            code: WRITE_NOT_A_LEAF_LAYER,
            message: format!("keyed layer `{}` is a group, not a leaf", layer.name),
        });
    };
    check_type(&layer.name, leaf, value)?;
    let mut steps = record_establishing_steps(context)?;
    steps.push(PlanStep::WriteData {
        address: DataAddress::layer_prefix(place, context.identity(), layers, context.span())
            .map_err(store_error)?,
        value: value.bytes()?,
    });
    Ok(WritePlan { steps })
}

pub(crate) fn plan_nested_field_write(
    context: IndexWriteContext<'_>,
    layers: &[LayerAddress],
    field: &str,
    value: &LeafValue,
) -> Result<WritePlan, WriteError> {
    let place = context.place();
    let members = checked_members_for_layers(place, layers)?;
    let leaf = checked_field_leaf(members, field).ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_FIELD,
        message: format!("group layer has no field `{field}`"),
    })?;
    check_type(field, leaf, value)?;
    let mut steps = record_establishing_steps(context)?;
    steps.push(PlanStep::WriteData {
        address: data_address(
            place,
            context.identity(),
            layers,
            &[field.to_string()],
            context.span(),
        )?,
        value: value.bytes()?,
    });
    Ok(WritePlan { steps })
}

pub(crate) fn plan_nested_identity_field_write(
    context: IndexWriteContext<'_>,
    layers: &[LayerAddress],
    field: &str,
    value: ReferencedIdentity<'_>,
) -> Result<WritePlan, WriteError> {
    let place = context.place();
    let members = checked_members_for_layers(place, layers)?;
    let leaf = checked_field_leaf(members, field).ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_FIELD,
        message: format!("group layer has no field `{field}`"),
    })?;
    let mut steps = record_establishing_steps(context)?;
    steps.push(PlanStep::WriteData {
        address: data_address(
            place,
            context.identity(),
            layers,
            &[field.to_string()],
            context.span(),
        )?,
        value: staged_identity_value(field, leaf, value.keys, value.referenced_arity)?,
    });
    Ok(WritePlan { steps })
}

pub(crate) fn plan_layer_group_write(
    context: IndexWriteContext<'_>,
    layers: &[LayerAddress],
    value: &ResourceValue,
    enforcement: RequiredEnforcement,
) -> Result<WritePlan, WriteError> {
    let place = context.place();
    let identity = context.identity();
    let span = context.span();
    let group = group_layer(place, layers)?;
    let to_write = collect_supplied_field_writes(&group.group_members, value, enforcement)?;
    let entry = DataAddress::layer_prefix(place, identity, layers, span).map_err(store_error)?;
    let mut steps = vec![PlanStep::DeleteData {
        address: entry.clone(),
    }];
    steps.extend(record_establishing_steps(context)?);
    steps.push(PlanStep::WriteDataNode { address: entry });
    for FieldWrite { path, bytes } in to_write {
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
    if !is_single_int_sequence(&place.identity_keys) {
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

/// Allocate the position one past `highest`, the single owner of the 1-based
/// key-space-exhaustion contract. `nextId`, saved `append`, and local sequence
/// `append` all route here, so a position past `i64::MAX` raises the same
/// catchable `write.id_overflow` fault rather than wrapping.
pub(crate) fn next_after(highest: i64) -> Result<i64, WriteError> {
    highest.checked_add(1).ok_or_else(|| WriteError {
        code: WRITE_ID_OVERFLOW,
        message: "the integer key space is exhausted; the highest key is i64::MAX".into(),
    })
}

fn record_presence_step(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    span: SourceSpan,
) -> Result<PlanStep, WriteError> {
    Ok(PlanStep::WriteRecordPresence {
        address: DataAddress::record(place, identity, span).map_err(store_error)?,
    })
}

/// Establish the record's store identity and the index entries it determines
/// regardless of any field. Every managed write that can create a record routes
/// through here, so an index keyed solely by identity components — which no field
/// write would list — is populated incrementally exactly as a restore rebuild
/// populates it from identity alone.
fn record_establishing_steps(context: IndexWriteContext<'_>) -> Result<Vec<PlanStep>, WriteError> {
    let mut steps = vec![record_presence_step(
        context.place(),
        context.identity(),
        context.span(),
    )?];
    stage_identity_only_index_writes(&mut steps, context)?;
    Ok(steps)
}

/// Resolve every materialized plain field of `members` against the supplied
/// value, returning the field paths whose bytes must be written. A required field
/// with no supplied value is rejected only when enforcement is immediate; inside a
/// transaction the missing field is left for the commit-time entry check, which
/// still catches an incomplete record but lets a later write in the same
/// transaction complete it first.
fn collect_supplied_field_writes(
    members: &[CheckedSavedMember],
    value: &ResourceValue,
    enforcement: RequiredEnforcement,
) -> Result<Vec<FieldWrite>, WriteError> {
    let mut to_write = Vec::new();
    for field in materialized_plain_fields(members) {
        let name = materialized_field_name(&field.path);
        match supplied_field(value, &name, field.leaf)? {
            Some(bytes) => to_write.push(FieldWrite {
                path: field.path,
                bytes,
            }),
            None if field.required && matches!(enforcement, RequiredEnforcement::Immediate) => {
                return Err(required_absent_error(
                    &name,
                    RequiredAbsentRemedy::PopulateInValue,
                ));
            }
            None => {}
        }
    }
    Ok(to_write)
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
                let key = saved.as_key()?.ok_or_else(|| WriteError {
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

pub(crate) fn checked_field_leaf<'a>(
    members: &'a [CheckedSavedMember],
    field: &str,
) -> Option<&'a StoreLeafKind> {
    members
        .iter()
        .find(|member| member.name == field)
        .and_then(|member| member.plain_field())
        .map(|(leaf, _)| leaf)
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
    entry: RequiredFieldEntry<'_>,
    members: &[CheckedSavedMember],
    supplied: Option<&[String]>,
    store: &TreeStore,
    span: SourceSpan,
    remedy: RequiredAbsentRemedy,
) -> Result<(), WriteError> {
    for field in materialized_plain_fields(members) {
        if !field.required || supplied.is_some_and(|supplied| supplied == field.path.as_slice()) {
            continue;
        }
        let address = data_address(entry.place, entry.identity, entry.layers, &field.path, span)?;
        if read_data(store, &address, span)
            .map_err(store_error)?
            .is_none()
        {
            return Err(required_absent_error(
                &materialized_field_name(&field.path),
                remedy,
            ));
        }
    }
    Ok(())
}

/// The store entry a required-field check addresses: a record identity at a
/// layer prefix.
#[derive(Clone, Copy)]
struct RequiredFieldEntry<'a> {
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    layers: &'a [LayerAddress],
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
        (StoreLeafKind::Identity { arity, .. }, LeafValue::Identity { keys })
            if keys.len() == *arity =>
        {
            Ok(())
        }
        _ => Err(WriteError {
            code: WRITE_TYPE_MISMATCH,
            message: format!("field `{field}` has the wrong type"),
        }),
    }
}

fn store_error(error: crate::error::RuntimeError) -> WriteError {
    WriteError {
        code: WRITE_STORE,
        message: error.message,
    }
}
