use std::path::Path;

use marrow_syntax::{Argument, Expression, ParsedSource, Severity, SourceSpan};

use crate::tooling::ToolingError;
use crate::{
    CheckedProgram, CheckedSavedKeyParam, CheckedSavedMember, CheckedSavedMemberKind, ScalarType,
    StoreLeafKind,
};
use crate::{analysis::debug_expression_scope_before, checks::saved_root_args_address_record};
use crate::{
    executable::{SavedPlaceResolver, lower_expr_for_file},
    infer::infer_type,
};

use super::DataPathSegment;
use super::path::{DataPathStep, walk_data_path_steps};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDataPathSegment {
    Root(String),
    Member(String),
    KeySlot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDataChild {
    pub name: String,
    pub kind: DeclaredDataChildKind,
    pub key_params: Vec<DeclaredDataKeyParam>,
    pub leaf: Option<StoreLeafKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredDataChildKind {
    Field { required: bool },
    Layer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDataKeyParam {
    pub name: String,
    pub scalar: Option<ScalarType>,
}

pub fn declared_data_children(
    program: &CheckedProgram,
    segments: &[DataPathSegment],
) -> Result<Vec<DeclaredDataChild>, ToolingError> {
    let steps: Vec<DataPathStep<'_>> = segments.iter().map(DataPathStep::from_data).collect();
    declared_data_children_steps(program, &steps)
}

pub fn declared_source_data_children(
    program: &CheckedProgram,
    segments: &[SourceDataPathSegment],
) -> Result<Vec<DeclaredDataChild>, ToolingError> {
    let steps: Vec<DataPathStep<'_>> = segments.iter().map(source_data_path_step).collect();
    declared_data_children_steps(program, &steps)
}

pub fn declared_source_receiver_data_children(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    receiver: &str,
    scope_span: SourceSpan,
) -> Vec<DeclaredDataChild> {
    declared_source_receiver_data_children_impl(program, file, parsed, receiver, scope_span)
        .unwrap_or_default()
}

fn declared_source_receiver_data_children_impl(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    receiver: &str,
    scope_span: SourceSpan,
) -> Option<Vec<DeclaredDataChild>> {
    let module = program
        .modules
        .iter()
        .find(|module| module.source_file == file)?;
    let (expr, syntax_diagnostics) = marrow_syntax::parse_expression(receiver);
    if syntax_diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic.severity, Severity::Error))
    {
        return None;
    }
    let expr = expr?;
    let (root, args) = source_receiver_root_args(&expr)?;
    let store = crate::resolve::resolve_store_by_root(program, root)?;
    let scope = debug_expression_scope_before(program, file, parsed, scope_span);
    let aliases = crate::build_alias_map(&module.imports);
    let mut diagnostics = Vec::new();
    let arg_types = args
        .iter()
        .map(|arg| {
            infer_type(
                program,
                &arg.value,
                &scope,
                &aliases,
                file,
                &mut diagnostics,
            )
        })
        .collect::<Vec<_>>();
    if diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic.severity, Severity::Error))
    {
        return None;
    }
    if !saved_root_args_address_record(store.store, args, &arg_types) {
        return None;
    }

    infer_type(program, &expr, &scope, &aliases, file, &mut diagnostics);
    if diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic.severity, Severity::Error))
    {
        return None;
    }
    let checked = lower_expr_for_file(program, file, &expr, &scope)?;
    let place = checked.saved_place()?;
    let members = SavedPlaceResolver::new(program).record_replacement_members(place)?;
    Some(declared_data_child_vec(members))
}

fn source_receiver_root_args(expr: &Expression) -> Option<(&str, &[Argument])> {
    match expr {
        Expression::SavedRoot { name, .. } => Some((name, &[])),
        Expression::Call { callee, args, .. } => {
            if let Expression::SavedRoot { name, .. } = callee.as_ref() {
                Some((name, args))
            } else {
                source_receiver_root_args(callee)
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            source_receiver_root_args(base)
        }
        _ => None,
    }
}

fn declared_data_children_steps(
    program: &CheckedProgram,
    segments: &[DataPathStep<'_>],
) -> Result<Vec<DeclaredDataChild>, ToolingError> {
    let walk = walk_data_path_steps(program, segments)?;
    Ok(walk
        .child_members
        .as_deref()
        .map(declared_data_child_vec)
        .unwrap_or_default())
}

fn source_data_path_step(segment: &SourceDataPathSegment) -> DataPathStep<'_> {
    match segment {
        SourceDataPathSegment::Root(name) => DataPathStep::source_root(name),
        SourceDataPathSegment::Member(name) => DataPathStep::source_member(name),
        SourceDataPathSegment::KeySlot => DataPathStep::key_slot(),
    }
}

fn declared_data_child_vec(members: &[CheckedSavedMember]) -> Vec<DeclaredDataChild> {
    members.iter().map(DeclaredDataChild::from_member).collect()
}

impl DeclaredDataChild {
    fn from_member(member: &CheckedSavedMember) -> Self {
        let kind = match &member.kind {
            CheckedSavedMemberKind::Field { required } => DeclaredDataChildKind::Field {
                required: *required,
            },
            CheckedSavedMemberKind::Group => DeclaredDataChildKind::Layer,
        };
        Self {
            name: member.name.clone(),
            kind,
            key_params: declared_key_params(&member.key_params),
            leaf: member.leaf.clone(),
        }
    }
}

fn declared_key_params(params: &[CheckedSavedKeyParam]) -> Vec<DeclaredDataKeyParam> {
    params
        .iter()
        .map(|param| DeclaredDataKeyParam {
            name: param.name.clone(),
            scalar: param.scalar,
        })
        .collect()
}
