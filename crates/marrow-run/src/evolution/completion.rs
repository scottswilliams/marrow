//! Crash-resume verification for a stamped activation.
//!
//! The accepted-catalog file is the last activation step. If a crash leaves the store
//! stamped at the proposal epoch while the file still names the prior epoch, resume may
//! publish the current generated proposal only after proving the stamped data and index
//! effects are still visible and match the exact recomputed witness.

mod default;
mod index;
mod proposal;
mod receipt;
mod retire;
mod transform;

use marrow_check::evolution::preview;
use marrow_check::{CatalogEntryKind, CatalogLifecycle, CheckedProgram};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{CommitMetadata, TreeStore};

use super::apply::{ApplyError, Approval};
use default::verify_default_completion;
use index::verify_index_completion;
use proposal::verify_proposal_identity;
use receipt::verify_default_receipt;
use retire::verify_retire_completion;
use transform::verify_transform_completion;

/// Prove a store-stamped activation is complete before crash resume publishes the
/// accepted-catalog file. Any missing receipt field, changed witness fingerprint, or
/// absent data/index effect is drift and must fail closed.
pub fn verify_activation_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    commit: &CommitMetadata,
    approval: Option<&Approval>,
) -> Result<(), ApplyError> {
    let (witness, _diagnostics) = preview(program, store)?;
    verify_proposal_identity(&witness, commit)?;

    let places = marrow_check::checked_activation_root_places(program);
    let defaults = verify_default_completion(program, store, &places)?;
    let records_transformed = verify_transform_completion(program, store, &places, &witness)?;
    verify_retire_completion(program, store, commit, &places, approval)?;
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

fn retired_ids(program: &CheckedProgram, kind: CatalogEntryKind) -> Vec<CatalogId> {
    program
        .catalog
        .proposal
        .as_ref()
        .into_iter()
        .flat_map(|proposal| proposal.entries.iter())
        .filter(|entry| {
            entry.kind == kind
                && retired_this_proposal(program, entry.stable_id.as_str(), entry.lifecycle, kind)
        })
        .filter_map(|entry| CatalogId::new(entry.stable_id.clone()).ok())
        .collect()
}

fn retired_this_proposal(
    program: &CheckedProgram,
    stable_id: &str,
    lifecycle: CatalogLifecycle,
    kind: CatalogEntryKind,
) -> bool {
    lifecycle == CatalogLifecycle::Reserved
        && program.catalog.proposal.is_some()
        && program.catalog.accepted_entries.iter().any(|accepted| {
            accepted.kind == kind
                && accepted.stable_id == stable_id
                && accepted.lifecycle == CatalogLifecycle::Active
        })
}

fn incomplete() -> StoreError {
    StoreError::Corruption {
        message: "activation completion evidence is missing a committed effect".to_string(),
    }
}
