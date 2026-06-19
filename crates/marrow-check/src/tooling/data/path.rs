use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment as StoreDataPathSegment;

use crate::tooling::ToolingError;
use crate::{CheckedProgram, CheckedSavedMemberKind, StoreLeafKind, checked_saved_root_place};

use super::path_error::DataPathError;
use super::render::render_data_path_segments;
use super::shape::{
    PathMemberKind, data_path_segment_for_member, key_mismatch, tooling_catalog_id,
};
use super::{DataPathSegment, ResolvedDataPath};

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
    let steps: Vec<DataPathStep<'_>> = segments.iter().map(DataPathStep::from_data).collect();
    resolve_data_path_steps(program, render_data_path_segments(segments), &steps)
}

pub fn resolve_source_text_data_path(
    program: &CheckedProgram,
    segments: &[crate::PathSegment],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    let steps: Vec<DataPathStep<'_>> = segments
        .iter()
        .map(DataPathStep::from_source_text)
        .collect();
    resolve_data_path_steps(program, crate::display_path(segments), &steps)
}

fn resolve_data_path_steps(
    program: &CheckedProgram,
    path: String,
    segments: &[DataPathStep<'_>],
) -> Result<Option<ResolvedDataPath>, ToolingError> {
    let Some((DataPathStep::Root(root), rest)) = segments.split_first() else {
        return Err(DataPathError::MissingRoot.into());
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| DataPathError::UnknownRoot {
            root: (*root).to_string(),
        })?;
    let Some(store) = tooling_catalog_id(&place.store_catalog_id, "store")? else {
        return Ok(None);
    };
    let mut identity = Vec::new();
    let mut rendered_segments = vec![DataPathSegment::Root((*root).to_string())];
    let mut index = 0usize;
    while let Some(segment) = rest.get(index) {
        let Some(key) = data_path_key(segment) else {
            break;
        };
        if identity.len() == place.identity_keys.len() {
            return Err(DataPathError::TooManyIdentityKeys {
                root: (*root).to_string(),
            }
            .into());
        }
        if let Some(mismatch) = key_mismatch(place.identity_keys[identity.len()].scalar, key) {
            return Err(DataPathError::IdentityKeyType {
                root: (*root).to_string(),
                expected: mismatch.expected,
                found: mismatch.found,
            }
            .into());
        }
        identity.push(key.clone());
        rendered_segments.push(DataPathSegment::Key(key.clone()));
        index += 1;
    }
    if index < rest.len() && identity.len() != place.identity_keys.len() {
        return Err(DataPathError::MissingIdentityKeys {
            root: (*root).to_string(),
            expected: place.identity_keys.len(),
        }
        .into());
    }

    let mut data_path = Vec::new();
    let identity_key_scalars = place.identity_keys.iter().map(|key| key.scalar).collect();
    let mut data_key_scalars = Vec::new();
    let mut data_key_prefix_len = 0usize;
    let mut members = place.root_members.as_slice();
    let mut leaf: Option<StoreLeafKind> = None;
    while let Some(segment) = rest.get(index) {
        let Some((name, kind)) = data_path_member(segment) else {
            return Err(DataPathError::UnexpectedKey.into());
        };
        let member = members
            .iter()
            .find(|member| member.name == *name && kind.matches(member))
            .ok_or_else(|| DataPathError::UnknownMember {
                flavor: kind.flavor(),
                name: name.to_string(),
            })?;
        rendered_segments.push(data_path_segment_for_member(member));
        let Some(member_id) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
            return Ok(None);
        };
        data_path.push(StoreDataPathSegment::Member(member_id));
        data_key_scalars = member.key_params.iter().map(|param| param.scalar).collect();
        data_key_prefix_len = 0;
        leaf = None;
        index += 1;

        let mut key_count = 0usize;
        while let Some(key) = rest.get(index).and_then(data_path_key) {
            if key_count == member.key_params.len() {
                return Err(DataPathError::TooManyMemberKeys {
                    member: name.to_string(),
                }
                .into());
            }
            if let Some(mismatch) = key_mismatch(member.key_params[key_count].scalar, key) {
                return Err(DataPathError::MemberKeyType {
                    member: name.to_string(),
                    expected: mismatch.expected,
                    found: mismatch.found,
                }
                .into());
            }
            data_path.push(StoreDataPathSegment::Key(key.clone()));
            rendered_segments.push(DataPathSegment::Key(key.clone()));
            key_count += 1;
            data_key_prefix_len = key_count;
            index += 1;
        }

        if key_count < member.key_params.len() {
            if index < rest.len() {
                return Err(DataPathError::IncompleteMemberKeys {
                    member: name.to_string(),
                }
                .into());
            }
            break;
        }
        members = match &member.kind {
            CheckedSavedMemberKind::Group => {
                leaf = None;
                data_key_scalars = Vec::new();
                data_key_prefix_len = 0;
                member.group_members.as_slice()
            }
            CheckedSavedMemberKind::Field { .. } => {
                leaf = member.leaf.clone();
                &[]
            }
        };
    }

    Ok(Some(ResolvedDataPath::new(
        path,
        (*root).to_string(),
        rendered_segments,
        leaf,
        StorageDataPath {
            store,
            identity,
            identity_arity: place.identity_keys.len(),
            identity_key_scalars,
            data_path,
            data_key_scalars,
            data_key_prefix_len,
        },
    )))
}

#[derive(Clone, Copy)]
enum DataPathStep<'a> {
    Root(&'a str),
    Member(&'a str, PathMemberKind),
    Key(&'a SavedKey),
}

impl<'a> DataPathStep<'a> {
    fn from_data(segment: &'a DataPathSegment) -> Self {
        match segment {
            DataPathSegment::Root(name) => Self::Root(name),
            DataPathSegment::Field(name) => Self::Member(name, PathMemberKind::Field),
            DataPathSegment::Layer(name) => Self::Member(name, PathMemberKind::Layer),
            DataPathSegment::Key(key) => Self::Key(key),
        }
    }

    fn from_source_text(segment: &'a crate::PathSegment) -> Self {
        match segment {
            crate::PathSegment::Root(name) => Self::Root(name),
            crate::PathSegment::Field(name) => Self::Member(name, PathMemberKind::SourceText),
            crate::PathSegment::RecordKey(key) | crate::PathSegment::IndexKey(key) => {
                Self::Key(key)
            }
        }
    }
}

fn data_path_key<'a>(segment: &DataPathStep<'a>) -> Option<&'a SavedKey> {
    match segment {
        DataPathStep::Key(key) => Some(key),
        DataPathStep::Root(_) | DataPathStep::Member(_, _) => None,
    }
}

fn data_path_member<'a>(segment: &DataPathStep<'a>) -> Option<(&'a str, PathMemberKind)> {
    match segment {
        DataPathStep::Member(name, kind) => Some((name, *kind)),
        DataPathStep::Root(_) | DataPathStep::Key(_) => None,
    }
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
