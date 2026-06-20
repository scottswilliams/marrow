use crate::tooling::ToolingError;
use crate::{
    CheckedProgram, CheckedSavedKeyParam, CheckedSavedMember, CheckedSavedMemberKind, ScalarType,
    StoreLeafKind,
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
