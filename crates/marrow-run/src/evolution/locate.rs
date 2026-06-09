//! Member-tree location and per-record iteration for evolution apply.
//!
//! Locate a catalog id within a place's checked member tree, recording the path of keyed
//! layers and plain members to reach it, and iterate every stored record of a place.

use marrow_check::{CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, StoreLeafKind};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

use super::apply::ApplyError;

/// The store catalog id of a place, validated once.
pub(super) fn store_id(place: &CheckedSavedPlace) -> Result<CatalogId, ApplyError> {
    let Some(raw) = &place.store_catalog_id else {
        return Err(ApplyError::Store(StoreError::Corruption {
            message: "evolution apply saw a missing store catalog id".to_string(),
        }));
    };
    CatalogId::new(raw.clone()).map_err(|_| {
        ApplyError::Store(StoreError::Corruption {
            message: "evolution apply saw an invalid store catalog id".to_string(),
        })
    })
}

pub(super) fn locate_member(
    place: &CheckedSavedPlace,
    catalog_id: &CatalogId,
) -> Option<MemberLocation> {
    let mut steps = Vec::new();
    let leaf = locate_in(&place.root_members, &mut steps, catalog_id)?;
    Some(MemberLocation { steps, leaf })
}

pub(super) struct MemberLocation {
    pub(super) steps: Vec<PathStep>,
    pub(super) leaf: Option<StoreLeafKind>,
}

pub(super) enum PathStep {
    Member(CatalogId),
    Layer(CatalogId),
}

fn locate_in(
    members: &[CheckedSavedMember],
    steps: &mut Vec<PathStep>,
    target: &CatalogId,
) -> Option<Option<StoreLeafKind>> {
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
            return Some(member.leaf.clone());
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

pub(super) fn for_each_place_record(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), ApplyError> {
    let store_id = store_id(place)?;
    store.for_each_record(&store_id, place.identity_keys.len(), visit)?;
    Ok(())
}
