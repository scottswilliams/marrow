use super::calls::{maybe_present_result, neighbor_read};
use super::keys::{expression_key, saved_place_key};
use super::read_only::guard_subexpr_admissible;
use super::scope::NameScope;
use crate::CheckedCallTarget;
use crate::CheckedExpr;
use crate::CheckedProgram;
use crate::MarrowType;
use crate::executable::{
    accepted_saved_place, checked_saved_index_read, checked_saved_place_effect, place_fully_keyed,
};
use crate::facts::{PresenceProofPlace, PresenceProofRead, ResourceMemberId, SavedPlaceEffect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadTarget {
    pub(super) place: ReadPlace,
    pub(super) keys: Vec<String>,
    pub(super) key_types: Vec<Option<MarrowType>>,
    pub(super) key_bindings: Vec<u32>,
    pub(super) read: PresenceProofRead,
    pub(super) value: ReadTargetValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadPlace {
    Saved {
        root: String,
        members: Vec<String>,
        effect: SavedPlaceEffect,
    },
    StoreIndex {
        root: String,
        id: crate::facts::StoreIndexId,
    },
    TransformOld {
        resource: crate::facts::ResourceId,
        member: ResourceMemberId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadTargetValue {
    Value,
    AddressOnly,
}

/// The resolution of a maybe-present read. A local guardable read (a maybe-present
/// call result or a local-collection/sparse-field read) carries no `ReadTarget`,
/// only the fact that it is a `Direct` maybe-present value; a saved read resolves to
/// a full [`ReadTarget`]. The public predicates apply their terminal check over this.
enum ReadResolution {
    LocalValue,
    Saved(ReadTarget),
}

fn resolve_read_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> Option<ReadResolution> {
    if let CheckedExpr::Call { target, .. } = expr
        && maybe_present_result(target)
    {
        return Some(ReadResolution::LocalValue);
    }
    let scope = NameScope::from_type_scope(type_scope, transform_old);
    if local_maybe_present_read(program, expr, &scope) {
        return Some(ReadResolution::LocalValue);
    }
    read_target_with_scope(program, expr, &scope).map(ReadResolution::Saved)
}

/// Whether `expr` resolves to a maybe-present *value* read — what `??` defaults.
/// An address-only place such as a keyed child layer addressed by only a partial
/// key prefix is rejected: it names an inner sub-layer to descend, not a value to
/// default.
pub(crate) fn read_value_resolves_in_type_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> bool {
    match resolve_read_target(program, expr, type_scope, transform_old) {
        Some(ReadResolution::LocalValue) => true,
        // A `next`/`prev` neighbor read resolves an iterable to a single
        // maybe-present neighbor value, so it defaults regardless of whether its
        // underlying subject is itself an addressable value.
        Some(ReadResolution::Saved(target)) => {
            target.value == ReadTargetValue::Value
                || matches!(
                    target.read,
                    PresenceProofRead::Next | PresenceProofRead::Prev
                )
        }
        None => false,
    }
}

pub(crate) fn exists_target_in_type_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> bool {
    read_resolution_in_type_scope(program, expr, type_scope, transform_old)
        .is_some_and(|read| read == PresenceProofRead::Direct)
}

pub(crate) fn bindable_saved_value_read_in_type_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> bool {
    match resolve_read_target(program, expr, type_scope, transform_old) {
        Some(ReadResolution::LocalValue) => true,
        Some(ReadResolution::Saved(target)) => {
            target.read == PresenceProofRead::Direct && target.value == ReadTargetValue::Value
        }
        None => false,
    }
}

fn read_resolution_in_type_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> Option<PresenceProofRead> {
    match resolve_read_target(program, expr, type_scope, transform_old)? {
        ReadResolution::LocalValue => Some(PresenceProofRead::Direct),
        ReadResolution::Saved(target) => Some(target.read),
    }
}

pub(super) fn read_target_with_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    if let CheckedExpr::Call {
        callee,
        args,
        target: call_target,
        ..
    } = expr
        && let Some(read) = neighbor_read(call_target)
    {
        let mut target = args
            .first()
            .and_then(|arg| read_target_with_scope(program, &arg.value, scope))?;
        let key = expression_key(callee, scope);
        target.keys.insert(0, key.text);
        target.key_types.insert(0, key.ty);
        target.read = read;
        return Some(target);
    }
    saved_path_target(program, expr, scope)
}

pub(super) fn saved_path_read_target_with_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    saved_place_target(program, expr, scope)
}

/// Whether `expr` is a resolvable maybe-present read that carries no persisted
/// presence proof: a local-collection indexed read of a bound name, or a
/// sparse-field read of a bound materialized value. Such a read is guardable by
/// `??`/`if const`/`exists`, and the runtime resolves it at the read site by
/// catching the absent fault, so the checker accepts the guard and rejects a bare
/// read without recording a saved-data proof.
///
/// The guardable set is widened strictly by construction. A `LocalCollection`
/// call target is a read of a bound sequence or keyed tree — never `append`, which
/// is a distinct builtin — and its key sub-expressions are screened through the
/// production read-only effect analysis, so a write, allocation (`nextId(^s)`),
/// host call, or throw smuggled into a key stays rejected. A sparse-field read's
/// base must be a bound materialized value — a bound name or a chained group layer
/// rooted at one — never a call or constructor in the read place. A call result
/// must be bound to a name first; evaluating an inline call as the guard base would
/// run its body, which may write saved data, open a transaction, call a host
/// capability, or throw, and no such effect may ride into a guard.
pub(super) fn local_maybe_present_read(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> bool {
    match expr {
        CheckedExpr::Call {
            args,
            target: CheckedCallTarget::LocalCollection { .. },
            ..
        } => args
            .iter()
            .all(|arg| guard_subexpr_admissible(program, &arg.value)),
        CheckedExpr::Field { base, name, .. } => bound_base_value_type(program, base, scope)
            .is_some_and(|base_type| crate::infer::sparse_member(program, &base_type, name)),
        _ => false,
    }
}

/// The materialized type of a sparse-field guard's base, admitted only when the
/// base is a bound value with no call in the read place: a bound name resolves
/// through the presence scope, and a chained group layer (`p.address`) descends
/// through the group member to its bound root. A call or constructor base yields
/// `None`, so its result must be bound to a name before its sparse field is
/// guarded — there is no expression to evaluate in the read place but the name.
fn bound_base_value_type(
    program: &CheckedProgram,
    base: &CheckedExpr,
    scope: &NameScope,
) -> Option<MarrowType> {
    match base {
        CheckedExpr::Name { segments, .. } => {
            let [bound] = segments.as_slice() else {
                return None;
            };
            scope.lookup_type(bound).cloned()
        }
        CheckedExpr::Field { base, name, .. } => {
            let inner = bound_base_value_type(program, base, scope)?;
            crate::infer::member_value_type(program, &inner, name)
        }
        _ => None,
    }
}

pub(super) fn proof_place(target: &ReadTarget) -> Option<PresenceProofPlace> {
    match &target.place {
        ReadPlace::Saved { effect, .. } => Some(PresenceProofPlace::Saved(effect.clone())),
        ReadPlace::TransformOld { resource, member } => {
            Some(PresenceProofPlace::Saved(SavedPlaceEffect {
                resource: *resource,
                members: vec![*member],
            }))
        }
        ReadPlace::StoreIndex { id, .. } => Some(PresenceProofPlace::StoreIndex(*id)),
    }
}

pub(super) fn read_file(
    program: &CheckedProgram,
    place: &PresenceProofPlace,
) -> Option<std::path::PathBuf> {
    let module = match place {
        PresenceProofPlace::Saved(place) => {
            let resource = program.facts.resource(place.resource);
            resource.module
        }
        PresenceProofPlace::StoreIndex(index) => {
            let index = program.facts.store_index(*index);
            let store = program.facts.store(index.store);
            store.module
        }
    };
    Some(
        program
            .facts
            .modules()
            .get(module.0 as usize)?
            .source_file
            .clone(),
    )
}

fn saved_path_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    if let Some(target) = transform_old_target(program, expr, scope) {
        return Some(target);
    }
    saved_place_target(program, expr, scope)
}

fn saved_place_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    let place = accepted_saved_place(expr)?;
    let path = saved_place_key(expr, scope)?;
    if let Some(index) = checked_saved_index_read(place) {
        let root = path.root;
        let value = store_index_value(place);
        return Some(ReadTarget {
            place: ReadPlace::StoreIndex { root, id: index },
            keys: path.keys,
            key_types: path.key_types,
            key_bindings: path.key_bindings,
            read: PresenceProofRead::Direct,
            value,
        });
    }
    let effect = checked_saved_place_effect(&program.facts, place)?;
    let value = saved_target_value(place);
    Some(ReadTarget {
        place: ReadPlace::Saved {
            root: path.root,
            members: path.members,
            effect,
        },
        keys: path.keys,
        key_types: path.key_types,
        key_bindings: path.key_bindings,
        read: PresenceProofRead::Direct,
        value,
    })
}

fn saved_target_value(place: &crate::CheckedSavedPlace) -> ReadTargetValue {
    // A composite layer is a value read only once every key column is filled. A
    // partial prefix names an inner sub-layer to descend, not a maybe-present value.
    if !place_fully_keyed(place) {
        return ReadTargetValue::AddressOnly;
    }
    match &place.terminal {
        crate::CheckedSavedTerminal::Record | crate::CheckedSavedTerminal::Field { .. } => {
            ReadTargetValue::Value
        }
        crate::CheckedSavedTerminal::Index { .. } => ReadTargetValue::AddressOnly,
    }
}

fn store_index_value(place: &crate::CheckedSavedPlace) -> ReadTargetValue {
    match &place.terminal {
        crate::CheckedSavedTerminal::Index {
            unique,
            arg_count,
            args,
            ..
        } if *unique && args.len() == *arg_count => ReadTargetValue::Value,
        _ => ReadTargetValue::AddressOnly,
    }
}

fn transform_old_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    let (base, name) = match expr {
        CheckedExpr::Field { base, name, .. } | CheckedExpr::OptionalField { base, name, .. } => {
            (base, name)
        }
        _ => return None,
    };
    if !matches!(
        base.as_ref(),
        CheckedExpr::Name { segments, .. } if segments.as_slice() == ["old"]
    ) {
        return None;
    }
    let member =
        crate::evolution::transform_old_member(program, scope.transform_old_resource()?, name)?;
    if member.required {
        return None;
    }
    Some(ReadTarget {
        place: ReadPlace::TransformOld {
            resource: member.resource,
            member: member.member,
        },
        keys: Vec::new(),
        key_types: Vec::new(),
        key_bindings: Vec::new(),
        read: PresenceProofRead::Direct,
        value: ReadTargetValue::Value,
    })
}
