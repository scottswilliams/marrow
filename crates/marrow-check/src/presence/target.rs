use marrow_schema::NodeKind;

use super::calls::neighbor_read;
use super::keys::{SavedPathParts, expression_key, saved_path_parts};
use super::scope::NameScope;
use crate::CheckedExpr;
use crate::CheckedProgram;
use crate::facts::{
    CheckedFacts, PresenceProofPlace, PresenceProofRead, ResourceMemberId, SavedPlaceEffect,
    StoreIndexId,
};
use crate::resolve::resolve_store_by_root;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadTarget {
    pub(super) place: ReadPlace,
    pub(super) keys: Vec<String>,
    pub(super) key_bindings: Vec<u32>,
    pub(super) read: PresenceProofRead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadPlace {
    Saved { root: String, members: Vec<String> },
    StoreIndex { root: String, index: String },
}

pub(crate) fn read_target(program: &CheckedProgram, expr: &CheckedExpr) -> Option<ReadTarget> {
    read_target_with_scope(program, expr, &NameScope::default())
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

pub(super) fn proof_place(
    program: &CheckedProgram,
    target: &ReadTarget,
) -> Option<PresenceProofPlace> {
    match &target.place {
        ReadPlace::Saved { root, members } => Some(PresenceProofPlace::Saved(saved_place(
            &program.facts,
            root,
            members,
        )?)),
        ReadPlace::StoreIndex { root, index } => Some(PresenceProofPlace::StoreIndex(
            store_index_place(program, root, index)?,
        )),
    }
}

pub(super) fn declaration_proves_presence(program: &CheckedProgram, target: &ReadTarget) -> bool {
    let ReadPlace::Saved { root, members } = &target.place else {
        return false;
    };
    let Some(store) = resolve_store_by_root(program, root) else {
        return false;
    };
    let member_names: Vec<&str> = members.iter().map(String::as_str).collect();
    matches!(
        node_for_path(&store.resource.members, &member_names),
        Some(node) if matches!(&node.kind, NodeKind::Slot { required: true, .. })
    )
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

pub(super) fn saved_place(
    facts: &CheckedFacts,
    root: &str,
    members: &[String],
) -> Option<SavedPlaceEffect> {
    let store = facts.store_by_root(root)?;
    let member_names: Vec<&str> = members.iter().map(String::as_str).collect();
    Some(SavedPlaceEffect {
        resource: store.resource,
        members: member_path_ids(facts, store.resource, &member_names)?,
    })
}

fn member_path_ids(
    facts: &CheckedFacts,
    resource: crate::facts::ResourceId,
    path: &[&str],
) -> Option<Vec<ResourceMemberId>> {
    let mut ids = Vec::new();
    for index in 0..path.len() {
        ids.push(facts.resource_member_id(resource, &path[..=index])?);
    }
    Some(ids)
}

fn saved_path_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    let path = saved_path_parts(expr, scope)?;
    if store_index_read(program, &path).is_some() {
        let root = path.root;
        let index = path.members[0].clone();
        return Some(ReadTarget {
            place: ReadPlace::StoreIndex { root, index },
            keys: path.keys,
            key_bindings: path.key_bindings,
            read: PresenceProofRead::Direct,
        });
    }
    let store = resolve_store_by_root(program, &path.root)?;
    if !path.members.is_empty() {
        let member_names: Vec<&str> = path.members.iter().map(String::as_str).collect();
        node_for_path(&store.resource.members, &member_names)?;
    }
    Some(ReadTarget {
        place: ReadPlace::Saved {
            root: path.root,
            members: path.members,
        },
        keys: path.keys,
        key_bindings: path.key_bindings,
        read: PresenceProofRead::Direct,
    })
}

fn store_index_read<'a>(
    program: &'a CheckedProgram,
    path: &SavedPathParts,
) -> Option<&'a marrow_schema::IndexSchema> {
    let [index_name] = path.members.as_slice() else {
        return None;
    };
    let schema = resolve_store_by_root(program, &path.root)?;
    schema
        .store
        .indexes
        .iter()
        .find(|index| index.name == *index_name && index.unique)
}

fn store_index_place(
    program: &CheckedProgram,
    root: &str,
    index_name: &str,
) -> Option<StoreIndexId> {
    let store = program.facts.store_by_root(root)?;
    program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store.id && index.name == index_name)
        .map(|index| index.id)
}

fn node_for_path<'a>(
    nodes: &'a [marrow_schema::Node],
    path: &[&str],
) -> Option<&'a marrow_schema::Node> {
    let (first, rest) = path.split_first()?;
    let node = nodes.iter().find(|node| node.name == *first)?;
    if rest.is_empty() {
        return Some(node);
    }
    node_for_path(&node.members, rest)
}
