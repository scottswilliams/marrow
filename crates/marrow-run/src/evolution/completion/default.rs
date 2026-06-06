use marrow_check::evolution::default_value_for_bound_member;
use marrow_check::{
    CatalogEntryKind, CheckedProgram, CheckedRuntimeProgram, CheckedSavedPlace, StoreLeafKind,
};
use marrow_store::cell::CatalogId;
use marrow_store::tree::TreeStore;

use crate::value::decode_leaf;

use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;

use super::super::apply::{ApplyError, for_each_place_record, store_id};
use super::super::backfill::{fold_default_cell, locations, visit_member_cell_paths};
use super::super::evidence::{ACTIVATION_DEFAULT_DIGEST, EvidenceDigest};
use super::{catalog_id, incomplete};

pub(super) struct DefaultCompletion {
    pub(super) catalog_id: CatalogId,
    pub(super) proposal_new: bool,
    pub(super) target_records: u64,
    pub(super) cell_digest: EvidenceDigest,
}

pub(super) fn verify_default_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    places: &[CheckedSavedPlace],
) -> Result<Vec<DefaultCompletion>, ApplyError> {
    let runtime = program.runtime();
    let mut completed = Vec::new();
    for default in &program.catalog.evolve_defaults {
        let target = catalog_id(&default.catalog_id)?;
        let proposal_new = !accepted_resource_member(program, &target);
        let value = default_value_for_bound_member(program, &default.catalog_id, &default.value)
            .ok_or(ApplyError::Drift)?
            .map_err(|_| ApplyError::Drift)?;
        let mut target_leaf = None;
        let mut target_records = 0u64;
        let mut cell_digest = EvidenceDigest::new(ACTIVATION_DEFAULT_DIGEST);
        cell_digest.catalog_id(&target);
        for (place, location) in locations(places, &target) {
            let sid = store_id(place)?;
            let leaf = location.leaf.clone().ok_or(ApplyError::Drift)?;
            if let Some(existing) = &target_leaf {
                if existing != &leaf {
                    return Err(ApplyError::Drift);
                }
            } else {
                target_leaf = Some(leaf.clone());
            }
            let steps = location.steps;
            let cell = DefaultCell {
                runtime: &runtime,
                proposal_new,
                expected: &value.encoded,
                leaf: &leaf,
            };
            for_each_place_record(store, place, &mut |identity| {
                visit_member_cell_paths(store, &sid, identity, &steps, &mut |path| {
                    target_records += 1;
                    verify_default_cell(&cell, store, &sid, identity, path, &mut cell_digest)
                })
            })?;
        }
        if target_leaf.is_none() {
            return Err(ApplyError::Drift);
        }
        completed.push(DefaultCompletion {
            catalog_id: target,
            proposal_new,
            target_records,
            cell_digest,
        });
    }
    Ok(completed)
}

/// The per-cell completion contract for one default: a proposal-new default must hold the
/// exact encoded constant at every target cell, while an accepted optional default must
/// hold any byte string that decodes under the member's current leaf type.
struct DefaultCell<'a> {
    runtime: &'a CheckedRuntimeProgram,
    proposal_new: bool,
    expected: &'a [u8],
    leaf: &'a StoreLeafKind,
}

/// Verify one stored default cell and fold it into the completion digest, or report the
/// activation incomplete when the cell is missing or does not satisfy its contract. The
/// folded fields match the staging recipe exactly, so a completed activation reproduces
/// the digest its stamp recorded.
fn verify_default_cell(
    cell: &DefaultCell<'_>,
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    digest: &mut EvidenceDigest,
) -> Result<(), marrow_store::StoreError> {
    let current = store.read_data_value(store_id, identity, path)?;
    if cell.proposal_new {
        if current.as_deref() != Some(cell.expected) {
            return Err(incomplete());
        }
    } else if !stored_default_cell_valid(cell.runtime, cell.leaf, &current) {
        return Err(incomplete());
    }
    let Some(bytes) = current.as_deref() else {
        return Err(incomplete());
    };
    fold_default_cell(digest, store_id, identity, path, bytes);
    Ok(())
}

fn stored_default_cell_valid(
    runtime: &CheckedRuntimeProgram,
    leaf: &StoreLeafKind,
    current: &Option<Vec<u8>>,
) -> bool {
    let Some(bytes) = current.as_deref() else {
        return false;
    };
    decode_leaf(runtime, bytes, leaf).is_some()
}

fn accepted_resource_member(program: &CheckedProgram, catalog_id: &CatalogId) -> bool {
    program.catalog.accepted_entries.iter().any(|entry| {
        entry.kind == CatalogEntryKind::ResourceMember && entry.stable_id == catalog_id.as_str()
    })
}
