use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment as StoreDataPathSegment;

use crate::tooling::ToolingError;
use crate::{
    CheckedProgram, CheckedRuntimeProgram, CheckedSavedMember, CheckedSavedMemberKind,
    CheckedSavedPlace, StoreLeafKind,
};

use super::path_error::DataPathError;
use super::program::{DataProgram, inspection_root_place, inspection_root_place_by_catalog_id};
use super::render::render_data_path_segments;
use super::shape::{
    PathMemberKind, data_path_segment_for_member, key_mismatch, tooling_catalog_id,
};
use super::{DataPathSegment, ResolvedDataPath, SavedDataPathSegment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StorageDataPath {
    pub(crate) store: CatalogId,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) identity_arity: usize,
    pub(crate) identity_key_scalars: Vec<Option<crate::ScalarType>>,
    pub(crate) data_path: Vec<StoreDataPathSegment>,
    pub(crate) data_key_scalars: Vec<Option<crate::ScalarType>>,
    pub(crate) data_key_prefix_len: usize,
}

pub fn resolve_data_path(
    program: &CheckedProgram,
    segments: &[DataPathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    resolve_data_path_in(program, segments)
}

pub fn resolve_runtime_data_path(
    program: &CheckedRuntimeProgram,
    segments: &[DataPathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    resolve_data_path_in(program, segments)
}

pub fn resolve_saved_data_path(
    program: &CheckedProgram,
    segments: &[SavedDataPathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    resolve_saved_data_path_in(program, segments)
}

pub fn resolve_runtime_saved_data_path(
    program: &CheckedRuntimeProgram,
    segments: &[SavedDataPathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    resolve_saved_data_path_in(program, segments)
}

fn resolve_data_path_in(
    program: &(impl DataProgram + ?Sized),
    segments: &[DataPathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    let steps: Vec<DataPathStep<'_>> = segments.iter().map(DataPathStep::from_data).collect();
    resolve_data_path_steps(program, Some(render_data_path_segments(segments)), &steps)
}

fn resolve_saved_data_path_in(
    program: &(impl DataProgram + ?Sized),
    segments: &[SavedDataPathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    let steps: Vec<DataPathStep<'_>> = segments.iter().map(DataPathStep::from_saved_data).collect();
    resolve_data_path_steps(program, None, &steps)
}

pub fn resolve_source_text_data_path(
    program: &CheckedProgram,
    segments: &[crate::PathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    let steps: Vec<DataPathStep<'_>> = segments
        .iter()
        .map(DataPathStep::from_source_text)
        .collect();
    resolve_data_path_steps(program, Some(crate::display_path(segments)), &steps)
}

fn resolve_data_path_steps(
    program: &(impl DataProgram + ?Sized),
    path: Option<String>,
    segments: &[DataPathStep<'_>],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    let (place, rest) = checked_data_path_root(program, segments)?;
    let Some(store) = tooling_catalog_id(&place.store_catalog_id, "store")? else {
        return Ok(None);
    };
    let walk = walk_data_path_rest(program, place, rest)?;

    let mut data_path = Vec::new();
    let mut rendered_segments = vec![DataPathSegment::Root(walk.root.clone())];
    for key in &walk.identity {
        rendered_segments.push(DataPathSegment::Key(key.clone()));
    }
    let identity_key_scalars = walk
        .place
        .identity_keys
        .iter()
        .map(|key| key.scalar)
        .collect();
    let mut data_key_scalars = Vec::new();
    let mut data_key_prefix_len = 0usize;
    for step in &walk.members {
        let member = &step.member;
        rendered_segments.push(data_path_segment_for_member(member));
        let Some(member_id) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
            return Ok(None);
        };
        data_path.push(StoreDataPathSegment::Member(member_id));
        data_key_scalars = member.key_params.iter().map(|param| param.scalar).collect();
        data_key_prefix_len = 0;
        for key in &step.keys {
            data_path.push(StoreDataPathSegment::Key(key.clone()));
            rendered_segments.push(DataPathSegment::Key(key.clone()));
            data_key_prefix_len += 1;
        }

        if step.key_count < member.key_params.len() {
            break;
        }
        match &member.kind {
            CheckedSavedMemberKind::Group => {
                data_key_scalars = Vec::new();
                data_key_prefix_len = 0;
            }
            CheckedSavedMemberKind::Field { .. } => {}
        }
    }

    let path = path.unwrap_or_else(|| render_data_path_segments(&rendered_segments));
    Ok(Some(ResolvedDataPath::new(
        path,
        walk.root.clone(),
        rendered_segments,
        walk.leaf.clone(),
        StorageDataPath {
            store,
            identity: walk.identity,
            identity_arity: walk.place.identity_keys.len(),
            identity_key_scalars,
            data_path,
            data_key_scalars,
            data_key_prefix_len,
        },
    )))
}

#[derive(Clone, Copy)]
pub(crate) enum DataPathStep<'a> {
    Root(DataPathRootStep<'a>),
    Member(DataPathMemberStep<'a>),
    Key(&'a SavedKey),
    KeySlot,
}

#[derive(Clone, Copy)]
pub(crate) enum DataPathRootStep<'a> {
    SourceName(&'a str),
    CatalogId(&'a str),
}

#[derive(Clone, Copy)]
pub(crate) enum DataPathMemberStep<'a> {
    SourceName(&'a str, PathMemberKind),
    CatalogId(&'a str, PathMemberKind),
}

impl<'a> DataPathStep<'a> {
    pub(crate) fn from_data(segment: &'a DataPathSegment) -> Self {
        match segment {
            DataPathSegment::Root(name) => Self::Root(DataPathRootStep::SourceName(name)),
            DataPathSegment::Field(name) => {
                Self::Member(DataPathMemberStep::SourceName(name, PathMemberKind::Field))
            }
            DataPathSegment::Layer(name) => {
                Self::Member(DataPathMemberStep::SourceName(name, PathMemberKind::Layer))
            }
            DataPathSegment::Key(key) => Self::Key(key),
        }
    }

    pub(crate) fn from_saved_data(segment: &'a SavedDataPathSegment) -> Self {
        match segment {
            SavedDataPathSegment::Root { store_catalog_id } => {
                Self::Root(DataPathRootStep::CatalogId(store_catalog_id))
            }
            SavedDataPathSegment::Field { member_catalog_id } => Self::Member(
                DataPathMemberStep::CatalogId(member_catalog_id, PathMemberKind::Field),
            ),
            SavedDataPathSegment::Layer { member_catalog_id } => Self::Member(
                DataPathMemberStep::CatalogId(member_catalog_id, PathMemberKind::Layer),
            ),
            SavedDataPathSegment::Key(key) => Self::Key(key),
        }
    }

    pub(crate) fn from_source_text(segment: &'a crate::PathSegment) -> Self {
        match segment {
            crate::PathSegment::Root(name) => Self::Root(DataPathRootStep::SourceName(name)),
            crate::PathSegment::Field(name) => Self::Member(DataPathMemberStep::SourceName(
                name,
                PathMemberKind::SourceText,
            )),
            crate::PathSegment::RecordKey(key) | crate::PathSegment::IndexKey(key) => {
                Self::Key(key)
            }
        }
    }

    pub(crate) fn source_root(name: &'a str) -> Self {
        Self::Root(DataPathRootStep::SourceName(name))
    }

    pub(crate) fn source_member(name: &'a str) -> Self {
        Self::Member(DataPathMemberStep::SourceName(
            name,
            PathMemberKind::SourceText,
        ))
    }

    pub(crate) fn key_slot() -> Self {
        Self::KeySlot
    }
}

fn data_path_key<'a>(segment: &DataPathStep<'a>) -> Option<&'a SavedKey> {
    match segment {
        DataPathStep::Key(key) => Some(key),
        DataPathStep::Root(_) | DataPathStep::Member(_) | DataPathStep::KeySlot => None,
    }
}

fn data_path_member<'a>(segment: &DataPathStep<'a>) -> Option<DataPathMemberStep<'a>> {
    match segment {
        DataPathStep::Member(member) => Some(*member),
        DataPathStep::Root(_) | DataPathStep::Key(_) | DataPathStep::KeySlot => None,
    }
}

impl DataPathMemberStep<'_> {
    fn resolve(
        &self,
        program: &(impl DataProgram + ?Sized),
        members: &[CheckedSavedMember],
    ) -> Result<CheckedSavedMember, DataPathError> {
        match self {
            Self::SourceName(name, kind) => members
                .iter()
                .find(|member| member.name == **name && kind.matches(member))
                .cloned()
                .ok_or_else(|| DataPathError::UnknownMember {
                    flavor: kind.flavor(),
                    name: (*name).to_string(),
                }),
            Self::CatalogId(member_catalog_id, kind) => {
                if !program.has_accepted_catalog_id(member_catalog_id) {
                    return Err(DataPathError::UnknownMemberCatalogId {
                        flavor: kind.flavor(),
                        member_catalog_id: (*member_catalog_id).to_string(),
                    });
                }
                members
                    .iter()
                    .find(|member| {
                        member.catalog_id.as_deref() == Some(*member_catalog_id)
                            && kind.matches(member)
                    })
                    .cloned()
                    .ok_or_else(|| DataPathError::UnknownMemberCatalogId {
                        flavor: kind.flavor(),
                        member_catalog_id: (*member_catalog_id).to_string(),
                    })
            }
        }
    }
}

pub(crate) struct CheckedDataPathWalk {
    pub(crate) root: String,
    pub(crate) place: CheckedSavedPlace,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) members: Vec<CheckedDataPathMemberStep>,
    pub(crate) child_members: Option<Vec<CheckedSavedMember>>,
    pub(crate) leaf: Option<StoreLeafKind>,
}

pub(crate) struct CheckedDataPathMemberStep {
    pub(crate) member: CheckedSavedMember,
    pub(crate) keys: Vec<SavedKey>,
    pub(crate) key_count: usize,
}

pub(crate) fn walk_data_path_steps(
    program: &(impl DataProgram + ?Sized),
    segments: &[DataPathStep<'_>],
) -> Result<CheckedDataPathWalk, ToolingError> {
    let (place, rest) = checked_data_path_root(program, segments)?;
    walk_data_path_rest(program, place, rest)
}

fn checked_data_path_root<'a>(
    program: &(impl DataProgram + ?Sized),
    segments: &'a [DataPathStep<'a>],
) -> Result<(CheckedSavedPlace, &'a [DataPathStep<'a>]), ToolingError> {
    let Some((DataPathStep::Root(root), rest)) = segments.split_first() else {
        return Err(DataPathError::MissingRoot.into());
    };
    let place = match root {
        DataPathRootStep::SourceName(root) => {
            inspection_root_place(program, root).ok_or_else(|| DataPathError::UnknownRoot {
                root: (*root).to_string(),
            })?
        }
        DataPathRootStep::CatalogId(store_catalog_id) => {
            inspection_root_place_by_catalog_id(program, store_catalog_id).ok_or_else(|| {
                DataPathError::UnknownRootCatalogId {
                    store_catalog_id: (*store_catalog_id).to_string(),
                }
            })?
        }
    };
    Ok((place, rest))
}

fn walk_data_path_rest(
    program: &(impl DataProgram + ?Sized),
    place: CheckedSavedPlace,
    rest: &[DataPathStep<'_>],
) -> Result<CheckedDataPathWalk, ToolingError> {
    let mut identity = Vec::new();
    let mut identity_count = 0usize;
    let mut index = 0usize;
    while let Some(segment) = rest.get(index) {
        if !data_path_step_is_key(segment) {
            break;
        }
        if identity_count == place.identity_keys.len() {
            return Err(DataPathError::TooManyIdentityKeys {
                root: place.root.clone(),
            }
            .into());
        }
        if let Some(key) = data_path_key(segment) {
            if let Some(mismatch) = key_mismatch(place.identity_keys[identity_count].scalar, key) {
                return Err(DataPathError::IdentityKeyType {
                    root: place.root.clone(),
                    expected: mismatch.expected,
                    found: mismatch.found,
                }
                .into());
            }
            identity.push(key.clone());
        }
        identity_count += 1;
        index += 1;
    }
    if identity_count < place.identity_keys.len() {
        if index < rest.len() {
            return Err(DataPathError::MissingIdentityKeys {
                root: place.root.clone(),
                expected: place.identity_keys.len(),
            }
            .into());
        }
        return Ok(CheckedDataPathWalk {
            root: place.root.clone(),
            place,
            identity,
            members: Vec::new(),
            child_members: None,
            leaf: None,
        });
    }

    let mut members = place.root_members.clone();
    let mut child_members = Some(members.clone());
    let mut walked_members = Vec::new();
    let mut leaf = None;
    while let Some(segment) = rest.get(index) {
        let Some(member_step) = data_path_member(segment) else {
            return Err(DataPathError::UnexpectedKey.into());
        };
        let member = member_step.resolve(program, &members)?;
        let member_name = member.name.clone();
        index += 1;

        let mut key_count = 0usize;
        let mut keys = Vec::new();
        while let Some(segment) = rest.get(index) {
            if !data_path_step_is_key(segment) {
                break;
            }
            if key_count == member.key_params.len() {
                return Err(DataPathError::TooManyMemberKeys {
                    member: member_name.clone(),
                }
                .into());
            }
            if let Some(key) = data_path_key(segment) {
                if let Some(mismatch) = key_mismatch(member.key_params[key_count].scalar, key) {
                    return Err(DataPathError::MemberKeyType {
                        member: member_name.clone(),
                        expected: mismatch.expected,
                        found: mismatch.found,
                    }
                    .into());
                }
                keys.push(key.clone());
            }
            key_count += 1;
            index += 1;
        }

        let keys_complete = key_count == member.key_params.len();
        walked_members.push(CheckedDataPathMemberStep {
            member: member.clone(),
            keys,
            key_count,
        });
        if !keys_complete {
            if index < rest.len() {
                return Err(DataPathError::IncompleteMemberKeys {
                    member: member_name,
                }
                .into());
            }
            return Ok(CheckedDataPathWalk {
                root: place.root.clone(),
                place,
                identity,
                members: walked_members,
                child_members: None,
                leaf: None,
            });
        }

        match &member.kind {
            CheckedSavedMemberKind::Group => {
                members = member.group_members.clone();
                child_members = Some(members.clone());
                leaf = None;
            }
            CheckedSavedMemberKind::Field { .. } => {
                leaf = member.leaf.clone();
                child_members = None;
                if index == rest.len() {
                    break;
                }
                members = Vec::new();
            }
        }
    }

    Ok(CheckedDataPathWalk {
        root: place.root.clone(),
        place,
        identity,
        members: walked_members,
        child_members,
        leaf,
    })
}

fn data_path_step_is_key(segment: &DataPathStep<'_>) -> bool {
    matches!(segment, DataPathStep::Key(_) | DataPathStep::KeySlot)
}

pub fn data_path_under_prefix(candidate: &ResolvedDataPath, prefix: &ResolvedDataPath) -> bool {
    candidate.root == prefix.root
        && candidate.storage.store == prefix.storage.store
        && candidate.storage.identity_arity == prefix.storage.identity_arity
        && candidate
            .storage
            .identity
            .starts_with(&prefix.storage.identity)
        && candidate
            .storage
            .data_path
            .starts_with(&prefix.storage.data_path)
}
