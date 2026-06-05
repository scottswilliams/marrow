use marrow_check::evolution::default_value_for_bound_member;
use marrow_check::{
    CatalogEntryKind, CheckedProgram, CheckedRuntimeProgram, CheckedSavedPlace, StoreLeafKind,
};
use marrow_store::cell::CatalogId;
use marrow_store::tree::TreeStore;

use crate::value::decode_leaf;

use super::super::apply::{ApplyError, for_each_place_record, store_id};
use super::super::backfill::{locations, visit_member_cell_paths};
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
            for_each_place_record(store, place, &mut |identity| {
                visit_member_cell_paths(store, &sid, identity, &steps, &mut |path| {
                    let current = store.read_data_value(&sid, identity, path)?;
                    target_records += 1;
                    if proposal_new {
                        if current.as_deref() != Some(value.encoded.as_slice()) {
                            return Err(incomplete());
                        }
                    } else if !stored_default_cell_valid(&runtime, &leaf, &current) {
                        return Err(incomplete());
                    }
                    let Some(bytes) = current.as_deref() else {
                        return Err(incomplete());
                    };
                    cell_digest.catalog_id(&sid);
                    cell_digest.saved_keys(identity);
                    cell_digest.data_path(path);
                    cell_digest.bytes(bytes);
                    Ok(())
                })?;
                Ok(())
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
