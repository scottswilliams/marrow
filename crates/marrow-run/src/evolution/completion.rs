//! Re-proving a stamped activation against a recomputed witness.
//!
//! A committed activation stamps its data, index, and metadata effects in one transaction.
//! This re-derives the witness from the live source and store and proves the recorded
//! activation fields still match the current durable effects. Historical applied-step
//! evidence may be carried by later writes for stale replay suppression, but this verifier
//! recomputes the current data and index effects instead of treating that evidence as a
//! completion proof.

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

/// Prove a store-stamped activation is complete against the current store: every
/// recorded activation field, witness fingerprint, and data/index effect matches the
/// recomputed witness. Per-commit changed root/index ids describe the commit that wrote
/// the metadata, so they are not applied-step evidence and are not carried as proof.
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
    let indexes_rebuilt = verify_index_completion(program, store, &witness, &places)?;

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
