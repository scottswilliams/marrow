//! Crash-resume verification for a stamped activation.
//!
//! The accepted-catalog file is the last activation step, so a crash can leave the store
//! stamped at the proposal epoch while the file still names the prior one. Resume may
//! publish the proposal only after proving the stamped data and index effects are still
//! visible and match the exact recomputed witness.

mod default;
mod index;
mod proposal;
mod receipt;
mod retire;
mod transform;
mod verdict;

use marrow_check::CheckedProgram;
use marrow_check::evolution::preview;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{CommitMetadata, TreeStore};

use super::apply::ApplyError;
use default::verify_default_completion;
use index::verify_index_completion;
use proposal::verify_proposal_identity;
use receipt::verify_default_receipt;
use retire::verify_retire_completion;
use transform::verify_transform_completion;
use verdict::verify_no_repair_verdicts;

/// Prove a store-stamped activation is complete before crash resume publishes the
/// accepted-catalog file. Any missing receipt field, changed witness fingerprint, or
/// absent data/index effect is drift and must fail closed.
pub fn verify_activation_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    commit: &CommitMetadata,
) -> Result<(), ApplyError> {
    let (witness, _diagnostics) = preview(program, store)?;
    verify_no_repair_verdicts(&witness)?;
    verify_proposal_identity(&witness, store, commit)?;

    let places = marrow_check::checked_activation_root_places(program);
    let defaults = verify_default_completion(program, store, &places)?;
    let records_transformed = verify_transform_completion(program, store, &places, &witness)?;
    verify_retire_completion(program, store, commit, &places)?;
    let indexes_rebuilt = verify_index_completion(program, store, commit, &places)?;

    verify_default_receipt(&defaults, commit)?;
    if commit.activation_records_transformed != records_transformed as u64
        || commit.activation_indexes_rebuilt != indexes_rebuilt as u64
    {
        return Err(ApplyError::Drift);
    }

    Ok(())
}

fn catalog_id(raw: &str) -> Result<CatalogId, ApplyError> {
    CatalogId::new(raw.to_string()).map_err(|_| {
        ApplyError::Store(StoreError::Corruption {
            message: "activation completion saw an invalid catalog id".to_string(),
        })
    })
}
