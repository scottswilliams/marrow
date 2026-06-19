use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment as StoreDataPathSegment;
use marrow_store::value::{scalar_key_matches_type, validate_scalar_key};

use crate::{CheckedSavedMember, CheckedSavedMemberKind, ScalarType};

use super::path_error::MemberFlavor;
use super::{DataPathSegment, KeyMismatch};

/// Resolve a checked store or member catalog id for a store read.
///
/// A `None` id is not corruption: it is durable identity that was never
/// committed — a never-run project, or a member just added in source and not yet
/// applied. That entity simply has no committed data, so callers treat `Ok(None)`
/// as "no data here" (empty roots, zero records, an absent read) rather than a
/// fault. Only a committed but malformed id is genuine `store.corruption`.
pub(crate) fn tooling_catalog_id(
    raw: &Option<String>,
    context: &'static str,
) -> Result<Option<CatalogId>, StoreError> {
    let Some(raw) = raw.as_deref() else {
        return Ok(None);
    };
    CatalogId::new(raw.to_string())
        .map(Some)
        .map_err(|_| StoreError::Corruption {
            message: format!("checked {context} catalog id is malformed"),
        })
}

pub(crate) fn key_mismatch(expected: Option<ScalarType>, key: &SavedKey) -> Option<KeyMismatch> {
    let expected = expected?;
    let found = key.scalar_type();
    (!scalar_key_matches_type(key, expected)).then_some(KeyMismatch { expected, found })
}

pub(crate) fn stored_key_mismatch(
    expected: Option<ScalarType>,
    key: &SavedKey,
) -> Result<Option<KeyMismatch>, StoreError> {
    validate_scalar_key(key).map_err(|error| StoreError::Corruption {
        message: error.to_string(),
    })?;
    Ok(key_mismatch(expected, key))
}

#[derive(Clone, Copy)]
pub(crate) enum PathMemberKind {
    Field,
    Layer,
    SourceText,
}

impl PathMemberKind {
    pub(crate) fn matches(self, member: &CheckedSavedMember) -> bool {
        match self {
            Self::Field => member.is_plain_field(),
            Self::Layer => !member.is_plain_field(),
            Self::SourceText => true,
        }
    }

    pub(crate) fn flavor(self) -> MemberFlavor {
        match self {
            Self::Field => MemberFlavor::Field,
            Self::Layer => MemberFlavor::Layer,
            Self::SourceText => MemberFlavor::Member,
        }
    }
}

pub(crate) fn data_path_segment_for_member(member: &CheckedSavedMember) -> DataPathSegment {
    if member.is_plain_field() {
        DataPathSegment::Field(member.name.clone())
    } else {
        DataPathSegment::Layer(member.name.clone())
    }
}

pub(crate) fn declared_members_below_path<'a>(
    members: &'a [CheckedSavedMember],
    path: &[StoreDataPathSegment],
) -> Option<&'a [CheckedSavedMember]> {
    if path.is_empty() {
        return Some(members);
    }
    let StoreDataPathSegment::Member(catalog) = &path[0] else {
        return None;
    };
    let member = members
        .iter()
        .find(|member| member.catalog_id.as_deref() == Some(catalog.as_str()))?;
    let rest = rest_after_member_keys(member, &path[1..])?;
    match &member.kind {
        CheckedSavedMemberKind::Group => declared_members_below_path(&member.group_members, rest),
        CheckedSavedMemberKind::Field { .. } => None,
    }
}

/// Where a stored data path lands relative to the declared member tree.
///
/// Both the walk cursor (which only cares whether a resume point is a value
/// position) and integrity orphan detection (which renders why an undeclared
/// path is not part of the schema) read this single classification, so the two
/// never drift in how they decide what a declared value path is.
pub(crate) enum DataPathShape {
    /// The path ends exactly on a declared scalar field value.
    Value,
    /// The path ends on a keyed group, which is a valid path-node position even
    /// though it is not a value.
    KeyedGroupNode,
    /// The path ends on a plain (keyless) group: well formed against the schema
    /// but neither a value nor a keyed-group node.
    PlainGroup,
    /// The path does not follow the declared member tree. The reason renders
    /// into the orphan diagnostic.
    Undeclared(&'static str),
}

pub(crate) fn classify_data_path(
    members: &[CheckedSavedMember],
    path: &[StoreDataPathSegment],
) -> DataPathShape {
    let Some(StoreDataPathSegment::Member(catalog)) = path.first() else {
        return DataPathShape::Undeclared("a saved path shape the schema does not declare");
    };
    let Some(member) = members
        .iter()
        .find(|member| member.catalog_id.as_deref() == Some(catalog.as_str()))
    else {
        return DataPathShape::Undeclared("a saved member the schema no longer declares");
    };
    let Some(rest) = rest_after_member_keys(member, &path[1..]) else {
        return DataPathShape::Undeclared("a saved member key shape the schema does not declare");
    };
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => {
            if rest.is_empty() {
                DataPathShape::Value
            } else {
                DataPathShape::Undeclared("a saved field path the schema does not declare")
            }
        }
        CheckedSavedMemberKind::Group => {
            if rest.is_empty() {
                if member.key_params.is_empty() {
                    DataPathShape::PlainGroup
                } else {
                    DataPathShape::KeyedGroupNode
                }
            } else {
                classify_data_path(&member.group_members, rest)
            }
        }
    }
}

pub(crate) fn cursor_names_value_path(
    members: &[CheckedSavedMember],
    data_path: &[StoreDataPathSegment],
) -> bool {
    matches!(classify_data_path(members, data_path), DataPathShape::Value)
}

pub(crate) fn validate_member_value_path(
    members: &[CheckedSavedMember],
    path: &[StoreDataPathSegment],
) -> Result<(), &'static str> {
    match classify_data_path(members, path) {
        DataPathShape::Value => Ok(()),
        DataPathShape::KeyedGroupNode | DataPathShape::PlainGroup => {
            Err("a saved group path the schema does not declare")
        }
        DataPathShape::Undeclared(reason) => Err(reason),
    }
}

pub(crate) fn validate_member_path_node(
    members: &[CheckedSavedMember],
    path: &[StoreDataPathSegment],
) -> Result<(), &'static str> {
    match classify_data_path(members, path) {
        DataPathShape::KeyedGroupNode => Ok(()),
        DataPathShape::Value => Err("a saved field node path the schema does not declare"),
        DataPathShape::PlainGroup => {
            Err("a saved plain group node path the schema does not declare")
        }
        DataPathShape::Undeclared(reason) => Err(reason),
    }
}

pub(crate) fn path_can_match(
    path: &[StoreDataPathSegment],
    filter: &[StoreDataPathSegment],
) -> bool {
    path.starts_with(filter) || filter.starts_with(path)
}

fn rest_after_member_keys<'a>(
    member: &CheckedSavedMember,
    mut rest: &'a [StoreDataPathSegment],
) -> Option<&'a [StoreDataPathSegment]> {
    for _ in &member.key_params {
        let Some(StoreDataPathSegment::Key(_)) = rest.first() else {
            return None;
        };
        rest = &rest[1..];
    }
    Some(rest)
}
