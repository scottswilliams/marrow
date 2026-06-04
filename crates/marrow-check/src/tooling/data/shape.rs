use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;

use crate::{CheckedSavedMember, CheckedSavedMemberKind, ScalarType};

use super::{DataQuerySegment, KeyMismatch};
use crate::tooling::ToolingError;

pub(crate) fn tooling_catalog_id(
    raw: &Option<String>,
    context: &'static str,
) -> Result<CatalogId, StoreError> {
    let Some(raw) = raw.as_deref() else {
        return Err(StoreError::Corruption {
            message: format!("checked {context} catalog id is missing"),
        });
    };
    CatalogId::new(raw.to_string()).map_err(|_| StoreError::Corruption {
        message: format!("checked {context} catalog id is malformed"),
    })
}

pub(crate) fn key_mismatch(expected: Option<ScalarType>, key: &SavedKey) -> Option<KeyMismatch> {
    let expected = expected?;
    let found = key.scalar_type();
    (expected != found).then_some(KeyMismatch { expected, found })
}

#[derive(Clone, Copy)]
pub(crate) enum QueryMemberKind {
    Field,
    Layer,
    SourceText,
}

impl QueryMemberKind {
    pub(crate) fn matches(self, member: &CheckedSavedMember) -> bool {
        match self {
            Self::Field => {
                member.key_params.is_empty()
                    && matches!(member.kind, CheckedSavedMemberKind::Field { .. })
            }
            Self::Layer => {
                !member.key_params.is_empty()
                    || matches!(member.kind, CheckedSavedMemberKind::Group)
            }
            Self::SourceText => true,
        }
    }

    pub(crate) fn unknown_message(self, name: &str) -> String {
        match self {
            Self::Field => format!("unknown saved field `{name}`"),
            Self::Layer => format!("unknown saved layer `{name}`"),
            Self::SourceText => format!("unknown saved member `{name}`"),
        }
    }
}

pub(crate) fn query_segment_for_member(member: &CheckedSavedMember) -> DataQuerySegment {
    if member.key_params.is_empty() && matches!(member.kind, CheckedSavedMemberKind::Field { .. }) {
        DataQuerySegment::Field(member.name.clone())
    } else {
        DataQuerySegment::Layer(member.name.clone())
    }
}

pub(crate) fn declared_members_below_path<'a>(
    members: &'a [CheckedSavedMember],
    path: &[DataPathSegment],
) -> Option<&'a [CheckedSavedMember]> {
    if path.is_empty() {
        return Some(members);
    }
    let DataPathSegment::Member(catalog) = &path[0] else {
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

pub(crate) fn cursor_names_value_path(
    members: &[CheckedSavedMember],
    data_path: &[DataPathSegment],
) -> Result<bool, ToolingError> {
    let Some(DataPathSegment::Member(catalog)) = data_path.first() else {
        return Ok(false);
    };
    let Some(member) = checked_member_by_catalog(members, catalog)? else {
        return Ok(false);
    };
    let Some(rest) = rest_after_member_keys(member, &data_path[1..]) else {
        return Ok(false);
    };
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => Ok(rest.is_empty()),
        CheckedSavedMemberKind::Group => {
            if rest.is_empty() {
                return Ok(false);
            }
            cursor_names_value_path(&member.group_members, rest)
        }
    }
}

pub(crate) fn validate_member_value_path(
    members: &[CheckedSavedMember],
    path: &[DataPathSegment],
) -> Result<(), &'static str> {
    let Some(DataPathSegment::Member(catalog)) = path.first() else {
        return Err("a saved path shape the schema does not declare");
    };
    let Some(member) = members
        .iter()
        .find(|member| member.catalog_id.as_deref() == Some(catalog.as_str()))
    else {
        return Err("a saved member the schema no longer declares");
    };
    let Some(rest) = rest_after_member_keys(member, &path[1..]) else {
        return Err("a saved member key shape the schema does not declare");
    };
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => {
            if rest.is_empty() {
                Ok(())
            } else {
                Err("a saved field path the schema does not declare")
            }
        }
        CheckedSavedMemberKind::Group => {
            if rest.is_empty() {
                Err("a saved group path the schema does not declare")
            } else {
                validate_member_value_path(&member.group_members, rest)
            }
        }
    }
}

pub(crate) fn checked_member_by_catalog<'a>(
    members: &'a [CheckedSavedMember],
    catalog: &CatalogId,
) -> Result<Option<&'a CheckedSavedMember>, ToolingError> {
    for member in members {
        let member_catalog = tooling_catalog_id(&member.catalog_id, "resource member")?;
        if &member_catalog == catalog {
            return Ok(Some(member));
        }
    }
    Ok(None)
}

pub(crate) fn path_can_match(path: &[DataPathSegment], filter: &[DataPathSegment]) -> bool {
    path.starts_with(filter) || filter.starts_with(path)
}

fn rest_after_member_keys<'a>(
    member: &CheckedSavedMember,
    mut rest: &'a [DataPathSegment],
) -> Option<&'a [DataPathSegment]> {
    for _ in &member.key_params {
        let Some(DataPathSegment::Key(_)) = rest.first() else {
            return None;
        };
        rest = &rest[1..];
    }
    Some(rest)
}
