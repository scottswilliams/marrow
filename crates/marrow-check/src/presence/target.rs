use super::calls::{maybe_present_result, neighbor_read};
use super::keys::{expression_key, saved_place_key};
use super::scope::NameScope;
use crate::CheckedExpr;
use crate::CheckedProgram;
use crate::MarrowType;
use crate::executable::{
    accepted_saved_place, checked_saved_index_read, checked_saved_place_effect,
};
use crate::facts::{PresenceProofPlace, PresenceProofRead, ResourceMemberId, SavedPlaceEffect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadTarget {
    pub(super) place: ReadPlace,
    pub(super) keys: Vec<String>,
    pub(super) key_bindings: Vec<u32>,
    pub(super) read: PresenceProofRead,
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

pub(crate) fn read_resolves_in_type_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> bool {
    read_resolution_in_type_scope(program, expr, type_scope, transform_old).is_some()
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

fn read_resolution_in_type_scope(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    type_scope: &[std::collections::HashMap<String, MarrowType>],
    transform_old: Option<super::TransformOldReadScope<'_>>,
) -> Option<PresenceProofRead> {
    if let CheckedExpr::Call { target, .. } = expr
        && maybe_present_result(target)
    {
        return Some(PresenceProofRead::Direct);
    }
    let scope = NameScope::from_type_scope(type_scope, transform_old);
    read_target_with_scope(program, expr, &scope).map(|target| target.read)
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
        target.keys.insert(0, expression_key(callee, scope).text);
        target.read = read;
        return Some(target);
    }
    saved_path_target(program, expr, scope)
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
    let place = accepted_saved_place(expr)?;
    let path = saved_place_key(expr, scope)?;
    if let Some(index) = checked_saved_index_read(place) {
        let root = path.root;
        return Some(ReadTarget {
            place: ReadPlace::StoreIndex { root, id: index },
            keys: path.keys,
            key_bindings: path.key_bindings,
            read: PresenceProofRead::Direct,
        });
    }
    let effect = checked_saved_place_effect(&program.facts, place)?;
    Some(ReadTarget {
        place: ReadPlace::Saved {
            root: path.root,
            members: path.members,
            effect,
        },
        keys: path.keys,
        key_bindings: path.key_bindings,
        read: PresenceProofRead::Direct,
    })
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
        key_bindings: Vec::new(),
        read: PresenceProofRead::Direct,
    })
}
