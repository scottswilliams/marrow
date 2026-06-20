//! Rebuild every declared index from the data a store already holds.
//!
//! Restore replays a store's data cells but never its index cells: a generated
//! index is derived from data, so a faithful restore rebuilds it rather than
//! trusting bytes that could disagree with the records. This re-derives each
//! declared index the same way an evolution rebuild does — the managed-write index
//! owner stages each entry, so a rebuilt index is byte-identical to one the runtime
//! maintained — and executes the writes against the store inside the caller's open
//! transaction, leaving the commit to the caller.

use marrow_check::{
    CheckedFacts, CheckedProgram, CheckedSavedPlace, checked_activation_root_places,
};
use marrow_store::tree::TreeStore;

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
    for place in indexed_places(program) {
        stage_place_indexes(&place, store, &program.facts)?;
    }
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

/// Rebuild every index on `place`: clear the index subtree, then write the entry each
/// record contributes. Restore replays committed data only, so no same-apply data overlay
/// is needed here.
fn stage_place_indexes(
    place: &CheckedSavedPlace,
    store: &TreeStore,
    facts: &CheckedFacts,
) -> Result<(), ApplyError> {
    for index in &place.indexes {
        stage_index_subtree_rebuild(index, place, store, facts)?;
    }
    Ok(())
}
