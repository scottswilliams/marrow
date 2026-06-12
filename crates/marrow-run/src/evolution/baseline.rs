//! Freezing a project's first accepted catalog into its store.
//!
//! The first authorized run over a durable store establishes the project's baseline
//! durable identity: it writes the pending catalog proposal as the store's accepted
//! catalog and stamps the activation context in the same transaction the catalog rows
//! land in. The stamp is built through the same [`metadata_stamp`] the evolution apply
//! and managed-write paths use, so a store this path just wrote passes its own open
//! fence by construction.

use marrow_check::program::CheckedProgram;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::write_plan::WritePlan;

use crate::write_plan::CommitIdAllocation;

use super::window::{StampFacts, metadata_stamp};

/// Commit `proposal` as the store's baseline accepted catalog, stamping the activation
/// context in the same transaction. Returns `Ok(true)` when the baseline was written, and
/// `Ok(false)` (writing nothing) when there is nothing to establish: the program proposes
/// no durable identity, the store already holds an accepted catalog (a project past its
/// baseline never churns the catalog rows or the commit stamp), or the store already holds
/// saved data without a catalog. That last case is a populated-but-unstamped store the
/// caller must refuse rather than silently adopt, so the baseline never stamps over it.
///
/// The catalog rows and commit metadata land under one transaction, so a reader sees
/// either no accepted catalog or the whole baseline. The commit metadata records no
/// activation work: a baseline freezes identity without backfilling, transforming, or
/// retiring any record.
pub fn commit_catalog_baseline(
    store: &TreeStore,
    program: &CheckedProgram,
) -> Result<bool, StoreError> {
    let Some(proposal) = &program.catalog.proposal else {
        return Ok(false);
    };
    if proposal.entries.is_empty()
        || store.read_catalog_snapshot()?.is_some()
        || !store.is_empty()?
    {
        return Ok(false);
    }

    let stamp = metadata_stamp(StampFacts {
        catalog_epoch: proposal.epoch,
        catalog_snapshot: Some(Box::new(proposal.clone())),
        commit_id: CommitIdAllocation::Baseline,
        source_digest: program.source_digest(),
        changed_root_catalog_ids: Vec::new(),
        changed_index_catalog_ids: Vec::new(),
    });

    WritePlan { steps: vec![stamp] }
        .commit(store, false)
        .map(|()| true)
}
