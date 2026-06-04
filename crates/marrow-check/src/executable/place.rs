use marrow_schema::{KeyDef, Node, NodeKind, Type};
use marrow_syntax::SourceSpan;

use crate::facts::{ModuleId, ResourceId, ResourceMemberId, StoreId, StoreIndexFact};
use crate::program::CheckedProgram;
use crate::resolve::{resolve_store_by_root, resolve_store_by_root as store_by_root};

use super::{
    CheckedArg, CheckedExpr, CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam,
    CheckedSavedLayer, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    CheckedSavedTerminal,
};

pub(super) fn checked_root_place(
    program: &CheckedProgram,
    root: &str,
    span: SourceSpan,
) -> Option<CheckedSavedPlace> {
    let store = resolve_store_by_root(program, root)?;
    let module_id = module_id(program, &store.module.name)?;
    let store_id = program.facts.store_id(module_id, root)?;
    let store_fact = program.facts.store(store_id);
    let resource_id = store_fact.resource;
    let resource_fact = program.facts.resource(resource_id);
    let members = checked_saved_members(
        program,
        store.module,
        resource_id,
        &[],
        &store.resource.members,
    );
    Some(CheckedSavedPlace {
        root: root.to_string(),
        store_id,
        resource_id,
        store_catalog_id: store_fact.catalog_id.clone(),
        resource_name: resource_fact.name.clone(),
        root_members: members.clone(),
        members,
        indexes: checked_saved_indexes(program, store_id),
        identity_args: Vec::new(),
        identity_keys: checked_key_params(&store.store.identity_keys),
        index_count: program
            .facts
            .store_indexes()
            .iter()
            .filter(|index| index.store == store_id)
            .count(),
        next_id_shape: store_fact.next_id_shape.clone(),
        layers: Vec::new(),
        terminal: CheckedSavedTerminal::Record,
        span,
    })
}

pub(super) fn checked_call_place(
    callee: &CheckedExpr,
    args: &[CheckedArg],
    program: &CheckedProgram,
    span: SourceSpan,
) -> Option<CheckedSavedPlace> {
    if let CheckedExpr::Field { base, name, .. } = callee
        && let CheckedExpr::SavedRoot { .. } = base.as_ref()
        && let Some(place) = base.saved_place()
        && let Some(index_fact) = checked_index_fact(program, place.store_id, name)
    {
        let mut indexed = place.clone();
        indexed.terminal = CheckedSavedTerminal::Index {
            name: name.clone(),
            catalog_id: index_fact.catalog_id,
            args: args.to_vec(),
            unique: index_fact.unique,
            arg_count: index_fact.keys.len(),
        };
        indexed.span = span;
        return Some(indexed);
    }

    let mut place = callee.saved_place()?.clone();
    if !matches!(place.terminal, CheckedSavedTerminal::Record) {
        return None;
    }
    if matches!(callee, CheckedExpr::SavedRoot { .. }) {
        place.identity_args = args.to_vec();
        place.span = span;
        return Some(place);
    }
    let layer = place.layers.last_mut()?;
    layer.args = args.to_vec();
    layer.span = span;
    place.span = span;
    Some(place)
}

pub(super) fn checked_field_place(
    base: &CheckedExpr,
    name: &str,
    program: &CheckedProgram,
    span: SourceSpan,
) -> Option<CheckedSavedPlace> {
    let mut place = base.saved_place()?.clone();
    if !matches!(place.terminal, CheckedSavedTerminal::Record) {
        return None;
    }
    if matches!(base, CheckedExpr::SavedRoot { .. })
        && let Some(index) = checked_index_fact(program, place.store_id, name)
    {
        place.terminal = CheckedSavedTerminal::Index {
            name: name.to_string(),
            catalog_id: index.catalog_id,
            args: Vec::new(),
            unique: index.unique,
            arg_count: index.keys.len(),
        };
        place.span = span;
        return Some(place);
    }
    if let Some(member) = checked_plain_field_member(&place.members, name) {
        place.terminal = CheckedSavedTerminal::Field {
            name: name.to_string(),
            catalog_id: member.catalog_id.clone(),
            leaf: member.leaf.clone(),
        };
        place.span = span;
        return Some(place);
    }
    let Some(member) = checked_layer_member(&place.members, name) else {
        place.terminal = CheckedSavedTerminal::Field {
            name: name.to_string(),
            catalog_id: None,
            leaf: None,
        };
        place.span = span;
        return Some(place);
    };
    place.layers.push(CheckedSavedLayer {
        name: name.to_string(),
        catalog_id: member.catalog_id.clone(),
        args: Vec::new(),
        key_params: member.key_params.clone(),
        leaf: member.leaf.clone(),
        members: member.group_members.clone(),
        span,
    });
    place.members = member.group_members.clone();
    place.span = span;
    Some(place)
}

fn checked_index_fact(
    program: &CheckedProgram,
    store_id: StoreId,
    name: &str,
) -> Option<CheckedSavedIndex> {
    program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store_id && index.name == name)
        .and_then(checked_saved_index)
}

fn checked_saved_indexes(program: &CheckedProgram, store_id: StoreId) -> Vec<CheckedSavedIndex> {
    program
        .facts
        .store_indexes()
        .iter()
        .filter(|index| index.store == store_id)
        .filter_map(checked_saved_index)
        .collect()
}

fn checked_saved_index(index: &StoreIndexFact) -> Option<CheckedSavedIndex> {
    Some(CheckedSavedIndex {
        id: index.id,
        name: index.name.clone(),
        catalog_id: index.catalog_id.clone(),
        unique: index.unique,
        keys: index
            .keys
            .iter()
            .map(|key| CheckedSavedIndexKey {
                name: key.name.clone(),
                source: key.source,
                value_meaning: key.value_meaning.clone(),
            })
            .collect(),
    })
}

fn checked_saved_members(
    program: &CheckedProgram,
    module: &crate::CheckedModule,
    resource_id: ResourceId,
    parent_path: &[String],
    members: &[Node],
) -> Vec<CheckedSavedMember> {
    members
        .iter()
        .map(|node| {
            let mut path = parent_path.to_vec();
            path.push(node.name.clone());
            let member_id = resource_member_id(program, resource_id, &path);
            CheckedSavedMember {
                id: member_id,
                name: node.name.clone(),
                key_params: checked_key_params(&node.key_params),
                kind: checked_saved_member_kind(node),
                catalog_id: member_id.and_then(|id| resource_member_catalog_id(program, id)),
                leaf: match &node.kind {
                    NodeKind::Slot { ty, .. } => checked_store_leaf_kind(program, module, ty),
                    NodeKind::Group => None,
                },
                group_members: match node.kind {
                    NodeKind::Group => {
                        checked_saved_members(program, module, resource_id, &path, &node.members)
                    }
                    NodeKind::Slot { .. } => Vec::new(),
                },
            }
        })
        .collect()
}

fn module_id(program: &CheckedProgram, name: &str) -> Option<ModuleId> {
    program
        .modules
        .iter()
        .position(|candidate| candidate.name == name)
        .map(|index| ModuleId(index as u32))
}

fn resource_member_id(
    program: &CheckedProgram,
    resource_id: ResourceId,
    path: &[String],
) -> Option<ResourceMemberId> {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    program.facts.resource_member_id(resource_id, &path)
}

fn resource_member_catalog_id(program: &CheckedProgram, id: ResourceMemberId) -> Option<String> {
    program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.id == id)
        .and_then(|member| member.catalog_id.clone())
}

fn checked_plain_field_member<'a>(
    members: &'a [CheckedSavedMember],
    name: &str,
) -> Option<&'a CheckedSavedMember> {
    members
        .iter()
        .find(|member| member.name == name && member.is_plain_field())
}

fn checked_layer_member<'a>(
    members: &'a [CheckedSavedMember],
    name: &str,
) -> Option<&'a CheckedSavedMember> {
    members
        .iter()
        .find(|member| member.name == name && !member.is_plain_field())
}

fn checked_saved_member_kind(node: &Node) -> CheckedSavedMemberKind {
    match &node.kind {
        NodeKind::Slot { required, .. } => CheckedSavedMemberKind::Field {
            required: *required,
        },
        NodeKind::Group => CheckedSavedMemberKind::Group,
    }
}

fn checked_key_params(keys: &[KeyDef]) -> Vec<CheckedSavedKeyParam> {
    keys.iter()
        .map(|key| CheckedSavedKeyParam {
            name: key.name.clone(),
            scalar: key.ty.scalar(),
        })
        .collect()
}

fn checked_store_leaf_kind(
    program: &CheckedProgram,
    module: &crate::CheckedModule,
    ty: &Type,
) -> Option<crate::StoreLeafKind> {
    match ty {
        Type::Identity(root) => {
            let store = store_by_root(program, root)?;
            Some(crate::StoreLeafKind::Identity {
                store_root: root.clone(),
                arity: store.store.identity_keys.len(),
            })
        }
        Type::Named(name) => checked_enum_leaf_kind(program, module, name),
        other => other.stored_scalar().map(crate::StoreLeafKind::Scalar),
    }
}

fn checked_enum_leaf_kind(
    program: &CheckedProgram,
    module: &crate::CheckedModule,
    name: &str,
) -> Option<crate::StoreLeafKind> {
    let (module_name, enum_name) = name
        .rsplit_once("::")
        .unwrap_or((module.name.as_str(), name));
    let module_index = program
        .modules
        .iter()
        .position(|candidate| candidate.name == module_name)?;
    let enum_id = program
        .facts
        .enum_id(ModuleId(module_index as u32), enum_name)?;
    Some(crate::StoreLeafKind::Enum { enum_id })
}
