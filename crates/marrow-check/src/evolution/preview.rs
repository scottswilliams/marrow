//! Read-only evolution preview: run the discharge and assemble the witness.
//!
//! Preview is the analysis entry a future `check --data` or activation gate calls.
//! It runs the discharge, then composes the witness from the discharge result and
//! the store's metadata fingerprints. It never mutates the store. The returned
//! diagnostics name the exact obligations that block activation; the witness reports
//! activatability through every obligation's verdict.

use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::discharge::discharge;
use super::witness::{CatalogFingerprint, EvolutionWitness};
use crate::program::CheckedProgram;

/// Discharge every obligation against `store` and assemble the evolution witness.
/// Strictly read-only. The witness composes the source and catalog fingerprints
/// with the store's engine profile, layout epoch, and latest commit id; the
/// diagnostics are the discharge's fail-closed messages.
pub fn preview(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(EvolutionWitness, Vec<String>), StoreError> {
    let discharge = discharge(program, store)?;

    let commit = store.read_commit_metadata()?;
    // An empty stamped digest predates digest stamping; treat it as unstamped so the
    // apply fence adopts the store rather than comparing against a blank.
    let store_source_digest = commit
        .as_ref()
        .map(|commit| commit.source_digest.clone())
        .filter(|digest| !digest.is_empty());
    let witness =
        EvolutionWitness {
            source_digest: crate::catalog::analyzed_source_digest(program),
            evolution_digest: crate::catalog::evolution_digest(program),
            accepted_catalog: CatalogFingerprint {
                epoch: program.catalog.accepted_epoch.unwrap_or(0),
                digest: program.catalog.accepted_digest.clone().unwrap_or_default(),
            },
            proposal_catalog: program.catalog.proposal.as_ref().map(|proposal| {
                CatalogFingerprint {
                    epoch: proposal.epoch,
                    digest: proposal.digest.clone(),
                }
            }),
            store_source_digest,
            engine_profile_digest: store.read_engine_profile_digest()?,
            layout_epoch: store.read_layout_epoch()?,
            store_commit_id: commit.map(|commit| commit.commit_id),
            changed_root_catalog_ids: discharge.changed_root_catalog_ids,
            changed_index_catalog_ids: discharge.changed_index_catalog_ids,
            verdicts: discharge.verdicts,
            counts: discharge.counts,
        };

    Ok((witness, discharge.diagnostics))
}
