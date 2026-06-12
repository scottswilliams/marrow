//! Freezing a project's first accepted catalog into its store.
//!
//! The first authorized run over a durable store establishes the project's baseline
//! durable identity: it writes the pending catalog proposal, or republishes the accepted
//! catalog already bound from `marrow.catalog.json`, as the store's accepted catalog and
//! stamps the activation context in the same transaction the catalog rows land in. The
//! stamp is built through the same [`metadata_stamp`] the evolution apply and
//! managed-write paths use, so a store this path just wrote passes its own open fence by
//! construction.

use marrow_check::program::CheckedProgram;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::write_plan::WritePlan;

use crate::write_plan::CommitIdAllocation;

use super::window::{StampFacts, metadata_stamp};

/// Commit the program's baseline accepted catalog, stamping the activation context in the
/// same transaction. A first run freezes the pending proposal; a fresh checkout whose
/// committed `marrow.catalog.json` already bound the program republishes that accepted
/// snapshot into the empty store. Returns `Ok(true)` when the baseline was written, and
/// `Ok(false)` (writing nothing) when there is nothing to establish: the program has no
/// durable identity, the store already holds an accepted catalog (a project past its
/// baseline never churns the catalog rows or the commit stamp), or the store already
/// holds saved data without a catalog. That last case is a populated-but-unstamped store
/// the caller must refuse rather than silently adopt, so the baseline never stamps over it.
///
/// The catalog rows and commit metadata land under one transaction, so a reader sees
/// either no accepted catalog or the whole baseline. The commit metadata records no
/// activation work: a baseline freezes identity without backfilling, transforming, or
/// retiring any record.
pub fn commit_catalog_baseline(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<bool, StoreError> {
    let Some(snapshot) = baseline_snapshot(program) else {
        return Ok(false);
    };
    if snapshot.entries.is_empty()
        || store.read_catalog_snapshot()?.is_some()
        || !store.is_empty()?
    {
        return Ok(false);
    }

    let stamp = metadata_stamp(StampFacts {
        catalog_epoch: snapshot.epoch,
        catalog_snapshot: Some(Box::new(snapshot)),
        commit_id: CommitIdAllocation::Baseline,
        source_digest: program.source_digest(),
        changed_root_catalog_ids: Vec::new(),
        changed_index_catalog_ids: Vec::new(),
    });

    WritePlan { steps: vec![stamp] }
        .commit(store, false)
        .map(|()| true)
}

fn baseline_snapshot(program: &CheckedProgram) -> Option<marrow_catalog::CatalogMetadata> {
    program.catalog.proposal.clone().or_else(|| {
        let epoch = program.catalog.accepted_epoch?;
        Some(marrow_catalog::CatalogMetadata::new(
            epoch,
            program.catalog.accepted_entries.clone(),
        ))
    })
}
