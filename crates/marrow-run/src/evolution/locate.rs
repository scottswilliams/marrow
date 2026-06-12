//! Run-side member-tree location for evolution apply.
//!
//! Locate a catalog id within a checked place's member tree, recording the keyed layers
//! and plain members needed to address it, and expose the place store id after validating
//! the checked place shape.

use marrow_check::{
    CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, checked_place_store_id,
};
use marrow_store::cell::CatalogId;

use super::apply::ApplyError;

/// The store catalog id of a place, validated once.
pub(super) fn store_id(place: &CheckedSavedPlace) -> Result<CatalogId, ApplyError> {
    Ok(checked_place_store_id(place)?)
}

pub(super) fn locate_member(
    place: &CheckedSavedPlace,
    catalog_id: &CatalogId,
) -> Option<MemberLocation> {
    let mut steps = Vec::new();
    locate_in(&place.root_members, &mut steps, catalog_id)?;
    Some(MemberLocation { steps })
}

pub(super) struct MemberLocation {
    pub(super) steps: Vec<PathStep>,
}

pub(super) enum PathStep {
    Member(CatalogId),
    Layer(CatalogId),
}

fn locate_in(
    members: &[CheckedSavedMember],
    steps: &mut Vec<PathStep>,
    target: &CatalogId,
) -> Option<()> {
    for member in members {
        let Some(raw_id) = &member.catalog_id else {
            continue;
        };
        let Ok(member_id) = CatalogId::new(raw_id.clone()) else {
            continue;
        };
        let keyed = !member.key_params.is_empty();
        let step = if keyed {
            PathStep::Layer(member_id.clone())
        } else {
            PathStep::Member(member_id.clone())
        };
        steps.push(step);
        if member_id == *target {
            return Some(());
        }
        if matches!(member.kind, CheckedSavedMemberKind::Group)
            && let Some(leaf) = locate_in(&member.group_members, steps, target)
        {
            return Some(leaf);
        }
        steps.pop();
    }
    None
}
