//! Rebuild every declared index from the data a store already holds.
//!
//! Restore replays a store's data cells but never its index cells: a generated
//! index is derived from data, so a faithful restore rebuilds it rather than
//! trusting bytes that could disagree with the records. This re-derives each
//! declared index the same way an evolution rebuild does — the managed-write index
//! owner stages each entry, so a rebuilt index is byte-identical to one the runtime
//! maintained — and executes the writes against the store inside the caller's open
//! transaction, leaving the commit to the caller.

use marrow_check::{CheckedProgram, CheckedSavedPlace, checked_activation_root_places};
use marrow_store::tree::TreeStore;

use crate::index_maintenance::EmptyStagedData;
use crate::write_plan::{PlanStep, WritePlan};

use super::apply::ApplyError;
use super::backfill::stage_index_subtree_rebuild;

/// Rebuild every declared index for every saved store in `program` from the records
/// currently in `store`, staging and executing the index writes inside the caller's
/// open transaction. The caller's commit makes the rebuilt indexes durable; this
/// function never begins or commits a transaction of its own.
///
/// Each index subtree is cleared and then repopulated from the live records, so a
/// rebuild is idempotent and a store with stale or partial index data converges on
/// exactly the entries the data implies.
pub fn rebuild_store_indexes(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), ApplyError> {
    let mut steps = Vec::new();
    for place in indexed_places(program) {
        stage_place_indexes(&place, store, &mut steps)?;
    }
    WritePlan { steps }.commit(store, true)?;
    Ok(())
}

/// The saved places `program` declares that carry at least one index. A place with
/// no index contributes no rebuild work.
fn indexed_places(program: &CheckedProgram) -> Vec<CheckedSavedPlace> {
    checked_activation_root_places(program)
        .into_iter()
        .filter(|place| !place.indexes.is_empty())
        .collect()
}

/// Stage a full rebuild of every index on `place`: clear the index subtree, then stage the
/// entry each record contributes. Restore replays committed data only, so each index is
/// derived against an empty staged-data view.
fn stage_place_indexes(
    place: &CheckedSavedPlace,
    store: &TreeStore,
    steps: &mut Vec<PlanStep>,
) -> Result<(), ApplyError> {
    for index in &place.indexes {
        let index_steps = stage_index_subtree_rebuild(index, place, store, &EmptyStagedData)?;
        steps.extend(index_steps);
    }
    Ok(())
}
