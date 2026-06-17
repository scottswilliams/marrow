use std::collections::HashMap;

use marrow_catalog::CatalogEntryKind;
use marrow_schema::{KeyDef, Node, NodeKind, Type};
use marrow_syntax::SourceSpan;

use crate::catalog::{CatalogKey, active_proposal_id_map, resource_member_path, store_index_path};
use crate::facts::{
    CheckedFacts, ModuleId, ResourceId, ResourceMemberId, SavedPlaceEffect, StoreId,
    StoreIndexFact, StoreIndexId, StoredValueMeaning,
};
use crate::program::{CheckedProgram, MarrowType};
use crate::resolve::resolve_store_by_root;
use crate::typerules::type_compatible;
use crate::{StoreLeafKind, resolve_resource_schema_type};

use super::{
    CheckedArg, CheckedExpr, CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam,
    CheckedSavedLayer, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    CheckedSavedTerminal,
};

pub(crate) struct SavedPlaceResolver<'a> {
    program: &'a CheckedProgram,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SavedAccessRejection {
    GeneratedIndexBranch,
    NoMatchingIndex { declaration: String },
    KeyedRootMemberWithoutIdentity(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SavedMemberRefKind {
    Field,
    Layer,
    Index,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavedMemberRef {
    pub(crate) root: String,
    pub(crate) chain: Vec<String>,
    pub(crate) span: SourceSpan,
    pub(crate) kind: SavedMemberRefKind,
}

impl<'a> SavedPlaceResolver<'a> {
    pub(crate) fn new(program: &'a CheckedProgram) -> Self {
        Self { program }
    }

    pub(crate) fn root_place(&self, root: &str, span: SourceSpan) -> Option<CheckedSavedPlace> {
        checked_root_place(self.program, root, span)
    }

    pub(crate) fn call_place(
        &self,
        callee: &CheckedExpr,
        args: &[CheckedArg],
        span: SourceSpan,
    ) -> Option<CheckedSavedPlace> {
        checked_call_place(callee, args, self.program, span)
    }

    pub(crate) fn field_place(
        &self,
        base: &CheckedExpr,
        name: &str,
        span: SourceSpan,
    ) -> Option<CheckedSavedPlace> {
        checked_field_place(base, name, self.program, span)
    }

    pub(crate) fn access_rejection(&self, expr: &CheckedExpr) -> Option<SavedAccessRejection> {
        saved_access_rejection(self.program, expr, SuggestedIndexContext::Disabled)
    }

    pub(crate) fn access_rejection_with_suggested_index(
        &self,
        expr: &CheckedExpr,
        scope: &[HashMap<String, MarrowType>],
    ) -> Option<SavedAccessRejection> {
        saved_access_rejection(self.program, expr, SuggestedIndexContext::Enabled { scope })
    }

    pub(crate) fn is_saved_path(&self, expr: &CheckedExpr) -> bool {
        accepted_saved_place(expr).is_some()
    }

    pub(crate) fn is_saved_path_callee(&self, callee: &CheckedExpr) -> bool {
        match callee {
            CheckedExpr::SavedRoot { .. } => callee.saved_place().is_some(),
            CheckedExpr::Call { .. } => self.is_saved_path(callee),
            CheckedExpr::Field { base, name, .. }
            | CheckedExpr::OptionalField { base, name, .. } => {
                if let CheckedExpr::SavedRoot { .. } = base.as_ref() {
                    let Some(place) = base.saved_place() else {
                        return false;
                    };
                    place.identity_keys.is_empty() || saved_root_index(base, name).is_some()
                } else {
                    self.is_saved_path(base)
                }
            }
            _ => false,
        }
    }

    pub(crate) fn value_type(&self, expr: &CheckedExpr) -> Option<MarrowType> {
        self.place_value_type(expr.saved_place()?)
    }

    /// The element value type of a Record-root collection: the store's resource
    /// type. Each element of a keyed root is a record, so this holds regardless of
    /// whether the root itself is addressed — unlike [`Self::value_type`], which
    /// reports a value only for an addressed place.
    pub(crate) fn record_root_element_type(&self, place: &CheckedSavedPlace) -> MarrowType {
        MarrowType::Resource(crate::resource_type_name(
            &self.store_module_name(place),
            &place.resource_name,
        ))
    }

    pub(crate) fn key_type(&self, expr: &CheckedExpr) -> Option<MarrowType> {
        self.place_key_type(expr.saved_place()?)
    }

    pub(crate) fn is_index_branch(&self, expr: &CheckedExpr) -> bool {
        index_branch(expr).is_some()
    }

    pub(crate) fn is_key_range_path(&self, expr: &CheckedExpr) -> bool {
        self.range_arg_position(expr).is_some()
            && (self.is_index_branch(expr) || self.layer_or_root_range_subject(expr).is_some())
    }

    pub(crate) fn has_key_range_arg(&self, expr: &CheckedExpr) -> bool {
        expr.saved_place().is_some_and(place_has_range_arg)
    }

    pub(crate) fn is_index_range_path(&self, expr: &CheckedExpr) -> bool {
        self.range_arg_position(expr).is_some() && self.is_index_branch(expr)
    }

    pub(crate) fn index_branch_info<'b>(
        &self,
        expr: &'b CheckedExpr,
    ) -> Option<IndexBranchInfo<'b>> {
        let place = expr.saved_place()?;
        let CheckedSavedTerminal::Index {
            name,
            args,
            unique,
            arg_count,
            ..
        } = &place.terminal
        else {
            return None;
        };
        (args.len() <= *arg_count).then_some(IndexBranchInfo {
            place,
            name,
            unique: *unique,
            arg_count: args.len(),
            key_count: *arg_count,
        })
    }

    pub(crate) fn non_unique_index_branch_yields_identity(&self, expr: &CheckedExpr) -> bool {
        let Some(info) = self.index_branch_info(expr) else {
            return false;
        };
        if info.unique {
            return false;
        }
        let identity_start = info
            .key_count
            .saturating_sub(info.place.identity_keys.len());
        info.arg_count >= identity_start
    }

    pub(crate) fn saved_key_params<'p>(
        &self,
        callee: &'p CheckedExpr,
    ) -> Option<SavedKeyParamTarget<'p>> {
        match callee.saved_place()?.terminal {
            CheckedSavedTerminal::Record if matches!(callee, CheckedExpr::SavedRoot { .. }) => {
                Some(SavedKeyParamTarget::Root(callee.saved_place()?))
            }
            CheckedSavedTerminal::Index { .. } => {
                Some(SavedKeyParamTarget::Index(callee.saved_place()?))
            }
            CheckedSavedTerminal::Record => {
                let place = callee.saved_place()?;
                place.layers.last().map(SavedKeyParamTarget::Layer)
            }
            CheckedSavedTerminal::Field { .. } => None,
        }
    }

    pub(crate) fn declared_member_or_index(&self, expr: &CheckedExpr) -> bool {
        let Some(place) = accepted_saved_place(expr) else {
            return false;
        };
        match &place.terminal {
            CheckedSavedTerminal::Index { .. } => true,
            CheckedSavedTerminal::Field {
                catalog_id, leaf, ..
            } => catalog_id.is_some() || leaf.is_some(),
            CheckedSavedTerminal::Record => !place.layers.is_empty(),
        }
    }

    pub(crate) fn saved_index_key_type(&self, key: &CheckedSavedIndexKey) -> MarrowType {
        self.index_key_type(key)
    }

    pub(crate) fn saved_key_param_type(key: &CheckedSavedKeyParam) -> MarrowType {
        checked_key_param_type(key).unwrap_or(MarrowType::Unknown)
    }

    pub(crate) fn record_replacement_members<'p>(
        &self,
        place: &'p CheckedSavedPlace,
    ) -> Option<&'p [CheckedSavedMember]> {
        if !matches!(place.terminal, CheckedSavedTerminal::Record) {
            return None;
        }
        if !root_record_addressed(place) || !place.layers.iter().all(layer_record_addressed) {
            return None;
        }
        let Some(layer) = place.layers.last() else {
            return Some(&place.root_members);
        };
        layer.leaf.is_none().then_some(layer.members.as_slice())
    }

    pub(crate) fn binding_member_ref(&self, expr: &CheckedExpr) -> Option<SavedMemberRef> {
        match expr {
            CheckedExpr::Field { name, span, .. }
            | CheckedExpr::OptionalField { name, span, .. } => {
                let place = expr.saved_place()?;
                let mut chain = place
                    .layers
                    .iter()
                    .map(|layer| layer.name.clone())
                    .collect::<Vec<_>>();
                match place.terminal {
                    CheckedSavedTerminal::Field { .. } => {
                        chain.push(name.clone());
                        Some(SavedMemberRef {
                            root: place.root.clone(),
                            chain,
                            span: *span,
                            kind: SavedMemberRefKind::Field,
                        })
                    }
                    CheckedSavedTerminal::Record if chain.last() == Some(name) => {
                        Some(SavedMemberRef {
                            root: place.root.clone(),
                            chain,
                            span: *span,
                            kind: SavedMemberRefKind::Layer,
                        })
                    }
                    _ => None,
                }
            }
            CheckedExpr::Call { callee, .. } => {
                let CheckedExpr::Field {
                    base, name, span, ..
                } = callee.as_ref()
                else {
                    return None;
                };
                let place = callee.saved_place()?;
                if matches!(base.as_ref(), CheckedExpr::SavedRoot { .. }) {
                    return matches!(place.terminal, CheckedSavedTerminal::Index { .. }).then(
                        || SavedMemberRef {
                            root: place.root.clone(),
                            chain: vec![name.clone()],
                            span: *span,
                            kind: SavedMemberRefKind::Index,
                        },
                    );
                }
                if !matches!(base.as_ref(), CheckedExpr::Call { .. }) {
                    return None;
                }
                let chain = place
                    .layers
                    .iter()
                    .map(|layer| layer.name.clone())
                    .collect::<Vec<_>>();
                if chain.last() != Some(name) {
                    return None;
                }
                Some(SavedMemberRef {
                    root: place.root.clone(),
                    chain,
                    span: *span,
                    kind: SavedMemberRefKind::Layer,
                })
            }
            _ => None,
        }
    }

    fn place_value_type(&self, place: &CheckedSavedPlace) -> Option<MarrowType> {
        match &place.terminal {
            CheckedSavedTerminal::Record => {
                if let Some(layer) = place.layers.last() {
                    if let Some(leaf) = &layer.leaf {
                        return self.leaf_type(leaf);
                    }
                    return self.group_entry_type(place);
                }
                (place.identity_keys.is_empty() || !place.identity_args.is_empty()).then(|| {
                    MarrowType::Resource(crate::resource_type_name(
                        &self.store_module_name(place),
                        &place.resource_name,
                    ))
                })
            }
            CheckedSavedTerminal::Field { leaf, .. } => self.leaf_type(leaf.as_ref()?),
            CheckedSavedTerminal::Index { unique, .. } => {
                unique.then(|| MarrowType::Identity(place.root.clone()))
            }
        }
    }

    fn place_key_type(&self, place: &CheckedSavedPlace) -> Option<MarrowType> {
        match &place.terminal {
            CheckedSavedTerminal::Record if place.identity_args.is_empty() => {
                (!place.identity_keys.is_empty()).then(|| MarrowType::Identity(place.root.clone()))
            }
            CheckedSavedTerminal::Record => {
                if let Some(layer) = place.layers.last() {
                    if let Some(position) = range_arg_position_in(&layer.args) {
                        return (position + 1 == layer.args.len()
                            && layer.args.len() == layer.key_params.len())
                        .then(|| checked_key_param_type(&layer.key_params[position]))
                        .flatten();
                    }
                    return match layer.key_params.as_slice() {
                        [key] => checked_key_param_type(key),
                        _ => None,
                    };
                }
                let position = range_arg_position_in(&place.identity_args)?;
                (position + 1 == place.identity_args.len()
                    && place.identity_args.len() == place.identity_keys.len())
                .then(|| checked_key_param_type(&place.identity_keys[position]))
                .flatten()
            }
            CheckedSavedTerminal::Index {
                args,
                unique,
                arg_count,
                ..
            } => {
                if *unique {
                    return Some(MarrowType::Identity(place.root.clone()));
                }
                let identity_start = arg_count.saturating_sub(place.identity_keys.len());
                if args.len() < identity_start {
                    let index = self.terminal_index(place)?;
                    return index
                        .keys
                        .get(args.len())
                        .map(|key| self.index_key_type(key));
                }
                Some(MarrowType::Identity(place.root.clone()))
            }
            CheckedSavedTerminal::Field { .. } => None,
        }
    }

    fn group_entry_type(&self, place: &CheckedSavedPlace) -> Option<MarrowType> {
        let store = resolve_store_by_root(self.program, &place.root)?;
        let layers = place
            .layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect::<Vec<_>>();
        let node = store.resource.descend_layers(&layers)?;
        if let Some(entry_type) = &node.entry_type
            && let Some(resource_type) =
                resolve_resource_schema_type(self.program, &store.module.name, entry_type)
        {
            return Some(resource_type);
        }
        Some(MarrowType::GroupEntry {
            resource: crate::resource_type_name(&store.module.name, &store.resource.name),
            layers: place
                .layers
                .iter()
                .map(|layer| layer.name.clone())
                .collect(),
        })
    }

    fn layer_or_root_range_subject<'p>(
        &self,
        expr: &'p CheckedExpr,
    ) -> Option<&'p CheckedSavedPlace> {
        let place = expr.saved_place()?;
        match &place.terminal {
            CheckedSavedTerminal::Record => Some(place),
            _ => None,
        }
    }

    fn range_arg_position(&self, expr: &CheckedExpr) -> Option<usize> {
        let args = match expr {
            CheckedExpr::Call { args, .. } => args,
            _ => return None,
        };
        args.iter().position(|arg| checked_range_expr(&arg.value))
    }

    fn terminal_index<'p>(&self, place: &'p CheckedSavedPlace) -> Option<&'p CheckedSavedIndex> {
        let CheckedSavedTerminal::Index { name, .. } = &place.terminal else {
            return None;
        };
        place.indexes.iter().find(|index| index.name == *name)
    }

    fn leaf_type(&self, leaf: &StoreLeafKind) -> Option<MarrowType> {
        match leaf {
            StoreLeafKind::Scalar(scalar) => Some(MarrowType::Primitive(*scalar)),
            StoreLeafKind::Identity { store_root, .. } => {
                Some(MarrowType::Identity(store_root.clone()))
            }
            StoreLeafKind::Enum { enum_id } => {
                let enum_fact = self.program.facts.enum_(*enum_id)?;
                let module = self
                    .program
                    .facts
                    .modules()
                    .get(enum_fact.module.0 as usize)?;
                Some(MarrowType::Enum {
                    module: module.name.clone(),
                    name: enum_fact.name.clone(),
                })
            }
        }
    }

    fn index_key_type(&self, key: &CheckedSavedIndexKey) -> MarrowType {
        match &key.value_meaning {
            StoredValueMeaning::Scalar(scalar) => MarrowType::Primitive(*scalar),
            StoredValueMeaning::Identity { root, .. } => MarrowType::Identity(root.clone()),
            StoredValueMeaning::Enum { enum_id, .. } => {
                let Some(enum_fact) = self.program.facts.enum_(*enum_id) else {
                    return MarrowType::Unknown;
                };
                let Some(module) = self
                    .program
                    .facts
                    .modules()
                    .get(enum_fact.module.0 as usize)
                else {
                    return MarrowType::Unknown;
                };
                MarrowType::Enum {
                    module: module.name.clone(),
                    name: enum_fact.name.clone(),
                }
            }
        }
    }

    fn store_module_name(&self, place: &CheckedSavedPlace) -> String {
        let store = self.program.facts.store(place.store_id);
        self.program
            .facts
            .modules()
            .get(store.module.0 as usize)
            .map(|module| module.name.clone())
            .unwrap_or_default()
    }
}

pub(crate) fn accepted_saved_place(expr: &CheckedExpr) -> Option<&CheckedSavedPlace> {
    let place = expr.saved_place()?;
    saved_access_rejection_without_program(expr)
        .is_none()
        .then_some(place)
}

pub(crate) fn checked_saved_place_effect(
    facts: &CheckedFacts,
    place: &CheckedSavedPlace,
) -> Option<SavedPlaceEffect> {
    if matches!(place.terminal, CheckedSavedTerminal::Index { .. }) {
        return None;
    }
    let store = facts.store(place.store_id);
    let members = checked_saved_member_ids(facts, store.resource, place)?;
    Some(SavedPlaceEffect {
        resource: store.resource,
        members,
    })
}

pub(crate) fn checked_saved_index_read(place: &CheckedSavedPlace) -> Option<StoreIndexId> {
    let CheckedSavedTerminal::Index { name, .. } = &place.terminal else {
        return None;
    };
    place
        .indexes
        .iter()
        .find(|index| index.name == *name)
        .map(|index| index.id)
}

fn checked_saved_member_ids(
    facts: &CheckedFacts,
    resource: ResourceId,
    place: &CheckedSavedPlace,
) -> Option<Vec<ResourceMemberId>> {
    let names = checked_saved_member_names(place);
    let mut ids = Vec::new();
    for index in 0..names.len() {
        ids.push(facts.resource_member_id(resource, &names[..=index])?);
    }
    Some(ids)
}

fn checked_saved_member_names(place: &CheckedSavedPlace) -> Vec<&str> {
    let mut names = place
        .layers
        .iter()
        .map(|layer| layer.name.as_str())
        .collect::<Vec<_>>();
    if let CheckedSavedTerminal::Field { name, .. } = &place.terminal {
        names.push(name);
    }
    names
}

#[derive(Clone, Copy)]
enum SuggestedIndexContext<'a> {
    Disabled,
    Enabled {
        scope: &'a [HashMap<String, MarrowType>],
    },
}

fn saved_access_rejection_without_program(expr: &CheckedExpr) -> Option<SavedAccessRejection> {
    saved_access_rejection_program_optional(None, expr, SuggestedIndexContext::Disabled)
}

fn saved_access_rejection(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    suggested_index: SuggestedIndexContext<'_>,
) -> Option<SavedAccessRejection> {
    saved_access_rejection_program_optional(Some(program), expr, suggested_index)
}

fn saved_access_rejection_program_optional(
    program: Option<&CheckedProgram>,
    expr: &CheckedExpr,
    suggested_index: SuggestedIndexContext<'_>,
) -> Option<SavedAccessRejection> {
    match expr {
        CheckedExpr::Field { base, name, .. } | CheckedExpr::OptionalField { base, name, .. } => {
            if matches!(expr, CheckedExpr::OptionalField { .. })
                && saved_root_index(base, name).is_some()
            {
                return Some(SavedAccessRejection::GeneratedIndexBranch);
            }
            if index_branch(base).is_some() {
                return Some(SavedAccessRejection::GeneratedIndexBranch);
            }
            if let CheckedExpr::SavedRoot { name: root, .. } = base.as_ref()
                && let Some(place) = base.saved_place()
                && !place.identity_keys.is_empty()
                && saved_root_index(base, name).is_none()
            {
                return Some(SavedAccessRejection::KeyedRootMemberWithoutIdentity(
                    root.clone(),
                ));
            }
            saved_access_rejection_program_optional(program, base, suggested_index)
        }
        CheckedExpr::Call { callee, args, .. } => {
            if let (Some(program), SuggestedIndexContext::Enabled { scope }) =
                (program, suggested_index)
                && let Some(declaration) = suggested_index_declaration(program, callee, args, scope)
            {
                return Some(SavedAccessRejection::NoMatchingIndex { declaration });
            }
            if matches!(callee.as_ref(), CheckedExpr::Call { .. }) && index_branch(callee).is_some()
            {
                return Some(SavedAccessRejection::GeneratedIndexBranch);
            }
            match callee.as_ref() {
                CheckedExpr::SavedRoot { .. } => None,
                _ => saved_access_rejection_program_optional(program, callee, suggested_index),
            }
        }
        CheckedExpr::Unary { operand, .. } => {
            saved_access_rejection_program_optional(program, operand, suggested_index)
        }
        CheckedExpr::Binary { left, right, .. } => {
            saved_access_rejection_program_optional(program, left, suggested_index).or_else(|| {
                saved_access_rejection_program_optional(program, right, suggested_index)
            })
        }
        CheckedExpr::Range {
            start, end, step, ..
        } => [start.as_deref(), end.as_deref(), step.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|expr| {
                saved_access_rejection_program_optional(program, expr, suggested_index)
            }),
        CheckedExpr::Interpolation { parts, .. } => parts.iter().find_map(|part| match part {
            super::CheckedInterpolationPart::Expr(expr) => {
                saved_access_rejection_program_optional(program, expr, suggested_index)
            }
            super::CheckedInterpolationPart::Text { .. } => None,
        }),
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {
            None
        }
    }
}

fn saved_root_index<'p>(base: &'p CheckedExpr, name: &str) -> Option<&'p CheckedSavedIndex> {
    let CheckedExpr::SavedRoot { .. } = base else {
        return None;
    };
    base.saved_place()?
        .indexes
        .iter()
        .find(|index| index.name == name)
}

fn index_branch(expr: &CheckedExpr) -> Option<&CheckedSavedPlace> {
    let place = expr.saved_place()?;
    matches!(place.terminal, CheckedSavedTerminal::Index { .. }).then_some(place)
}

fn suggested_index_declaration(
    program: &CheckedProgram,
    callee: &CheckedExpr,
    args: &[CheckedArg],
    scope: &[HashMap<String, MarrowType>],
) -> Option<String> {
    let CheckedExpr::Field { base, name, .. } = callee else {
        return None;
    };
    let CheckedExpr::SavedRoot { .. } = base.as_ref() else {
        return None;
    };
    let place = base.saved_place()?;
    if place.identity_keys.is_empty()
        || place.indexes.iter().any(|index| index.name == *name)
        || place.identity_keys.iter().any(|key| key.name == *name)
        || root_member_named(place, name)
    {
        return None;
    }
    let mut keys = Vec::with_capacity(args.len() + place.identity_keys.len());
    for arg in args {
        keys.push(suggested_index_arg_key(program, place, arg, scope)?);
    }
    if keys.is_empty() {
        return None;
    }
    for key in &place.identity_keys {
        if !keys.iter().any(|name| name == &key.name) {
            keys.push(key.name.clone());
        }
    }
    Some(format!("index {}({})", name, keys.join(", ")))
}

fn suggested_index_arg_key(
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
    arg: &CheckedArg,
    scope: &[HashMap<String, MarrowType>],
) -> Option<String> {
    if arg.name.is_some() {
        return None;
    }
    let CheckedExpr::Name { segments, .. } = &arg.value else {
        return None;
    };
    let [name] = segments.as_slice() else {
        return None;
    };
    let member = root_plain_field(place, name)?;
    let expected = SavedPlaceResolver::new(program).leaf_type(member.leaf.as_ref()?)?;
    let actual = scoped_name_type(scope, name)?;
    (type_compatible(&expected, actual) == Some(true)).then(|| member.name.clone())
}

fn root_member_named(place: &CheckedSavedPlace, name: &str) -> bool {
    place.root_members.iter().any(|member| member.name == name)
}

fn root_plain_field<'p>(
    place: &'p CheckedSavedPlace,
    name: &str,
) -> Option<&'p CheckedSavedMember> {
    place
        .root_members
        .iter()
        .find(|member| member.name == name && member.is_plain_field())
}

fn scoped_name_type<'a>(
    scope: &'a [HashMap<String, MarrowType>],
    name: &str,
) -> Option<&'a MarrowType> {
    scope.iter().rev().find_map(|frame| frame.get(name))
}

fn checked_range_expr(expr: &CheckedExpr) -> bool {
    matches!(
        expr,
        CheckedExpr::Range { .. }
            | CheckedExpr::Binary {
                op: super::CheckedBinaryOp::RangeExclusive | super::CheckedBinaryOp::RangeInclusive,
                ..
            }
    )
}

fn range_arg_position_in(args: &[CheckedArg]) -> Option<usize> {
    args.iter().position(|arg| checked_range_expr(&arg.value))
}

fn place_has_range_arg(place: &CheckedSavedPlace) -> bool {
    range_arg_position_in(&place.identity_args).is_some()
        || place
            .layers
            .iter()
            .any(|layer| range_arg_position_in(&layer.args).is_some())
        || match &place.terminal {
            CheckedSavedTerminal::Index { args, .. } => range_arg_position_in(args).is_some(),
            CheckedSavedTerminal::Record | CheckedSavedTerminal::Field { .. } => false,
        }
}

fn root_record_addressed(place: &CheckedSavedPlace) -> bool {
    place.identity_keys.is_empty() || !place.identity_args.is_empty()
}

fn layer_record_addressed(layer: &CheckedSavedLayer) -> bool {
    layer.key_params.is_empty() || !layer.args.is_empty()
}

fn checked_key_param_type(key: &CheckedSavedKeyParam) -> Option<MarrowType> {
    key.scalar.map(MarrowType::Primitive)
}

pub(crate) struct IndexBranchInfo<'p> {
    pub(crate) place: &'p CheckedSavedPlace,
    pub(crate) name: &'p str,
    pub(crate) unique: bool,
    pub(crate) arg_count: usize,
    pub(crate) key_count: usize,
}

pub(crate) enum SavedKeyParamTarget<'p> {
    Root(&'p CheckedSavedPlace),
    Index(&'p CheckedSavedPlace),
    Layer(&'p CheckedSavedLayer),
}

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
        store_catalog_id: store_fact.catalog_id.clone(),
        resource_name: resource_fact.name.clone(),
        root_members: members.clone(),
        members,
        indexes: checked_saved_indexes(program, store_id),
        identity_args: Vec::new(),
        identity_keys: checked_key_params(&store.store.identity_keys),
        next_id_shape: store_fact.next_id_shape.clone(),
        layers: Vec::new(),
        terminal: CheckedSavedTerminal::Record,
        span,
    })
}

pub(super) fn checked_activation_root_places(program: &CheckedProgram) -> Vec<CheckedSavedPlace> {
    let proposal_ids = active_proposal_id_map(program);
    let mut places = Vec::new();
    for module in &program.modules {
        for store in &module.stores {
            let Some(mut place) = checked_root_place(program, &store.root, SourceSpan::default())
            else {
                continue;
            };
            overlay_index_ids(&mut place, &proposal_ids, &module.name, &store.root);
            overlay_member_ids(
                &mut place.root_members,
                &proposal_ids,
                &module.name,
                &store.resource,
                &mut Vec::new(),
            );
            place.members = place.root_members.clone();
            if place.store_catalog_id.is_some() {
                places.push(place);
            }
        }
    }
    places
}

fn overlay_index_ids(
    place: &mut CheckedSavedPlace,
    proposal_ids: &HashMap<CatalogKey, String>,
    module: &str,
    root: &str,
) {
    for index in &mut place.indexes {
        if index.catalog_id.is_none()
            && let Some(catalog_id) = proposal_ids.get(&CatalogKey::new(
                CatalogEntryKind::StoreIndex,
                store_index_path(module, root, &index.name),
            ))
        {
            index.catalog_id = Some(catalog_id.clone());
        }
    }
}

fn overlay_member_ids(
    members: &mut [CheckedSavedMember],
    proposal_ids: &HashMap<CatalogKey, String>,
    module: &str,
    resource: &str,
    parent_path: &mut Vec<String>,
) {
    for member in members {
        parent_path.push(member.name.clone());
        if member.catalog_id.is_none()
            && let Some(catalog_id) = proposal_ids.get(&CatalogKey::new(
                CatalogEntryKind::ResourceMember,
                resource_member_path(module, resource, parent_path),
            ))
        {
            member.catalog_id = Some(catalog_id.clone());
        }
        overlay_member_ids(
            &mut member.group_members,
            proposal_ids,
            module,
            resource,
            parent_path,
        );
        parent_path.pop();
    }
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
        id: member.id,
        name: name.to_string(),
        catalog_id: member.catalog_id.clone(),
        args: Vec::new(),
        key_params: member.key_params.clone(),
        leaf: member.leaf.clone(),
        typed_entry: member.typed_entry,
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
                typed_entry: node.entry_type.is_some(),
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
            let store = resolve_store_by_root(program, root)?;
            Some(crate::StoreLeafKind::Identity {
                store_root: root.clone(),
                arity: store.store.identity_keys.len(),
            })
        }
        Type::Named(name) => checked_enum_leaf_kind(program, module, name),
        // A non-named, non-identity leaf decodes to its scalar stored-value envelope.
        other => other.scalar().map(crate::StoreLeafKind::Scalar),
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
