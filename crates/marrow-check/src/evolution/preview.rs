//! Read-only evolution preview: run the discharge and assemble the witness.
//!
//! Preview is the analysis entry a future `check --data` or activation gate calls.
//! It runs the discharge, then composes the witness from the discharge result and
//! the store's metadata fingerprints. It never mutates the store. The returned
//! diagnostics name the exact obligations that block activation; the witness reports
//! activatability through every obligation's verdict.

use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::discharge::{RepairDiagnostic, discharge};
use super::witness::{CatalogFingerprint, EvolutionWitness};
use crate::program::CheckedProgram;

/// Discharge every obligation against `store` and assemble the evolution witness.
/// Strictly read-only. The witness composes the source and catalog fingerprints
/// with the store's engine profile, layout epoch, and latest commit id; the
/// diagnostics are the discharge's fail-closed messages.
pub fn preview(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(EvolutionWitness, Vec<RepairDiagnostic>), StoreError> {
    let discharge = discharge(program, store)?;

    let commit = store.read_commit_metadata()?;
    let store_source_digest = commit.as_ref().map(|commit| commit.source_digest.clone());
    let engine_profile_digest = commit.as_ref().map(|commit| commit.engine_profile_digest);
    let layout_epoch = commit.as_ref().map(|commit| commit.layout_epoch);
    let (source_digest, evolution_digest) = crate::catalog::source_and_evolution_digests(program);
    let witness =
        EvolutionWitness {
            source_digest,
            evolution_digest,
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
            engine_profile_digest,
            layout_epoch,
            store_commit_id: commit.map(|commit| commit.commit_id),
            changed_root_catalog_ids: discharge.changed_root_catalog_ids,
            changed_index_catalog_ids: discharge.changed_index_catalog_ids,
            verdicts: discharge.verdicts,
            counts: discharge.counts,
        };

    Ok((witness, discharge.diagnostics))
}

#[cfg(test)]
mod tests {
    use marrow_store::tree::{CommitMetadata, TreeStore};

    use super::*;

    #[test]
    fn preview_keeps_an_empty_stamped_source_digest() {
        let store = TreeStore::memory();
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 1,
                catalog_epoch: 7,
                layout_epoch: 0,
                source_digest: String::new(),
                engine_profile_digest: [0; 8],
                changed_root_catalog_ids: Vec::new(),
                changed_index_catalog_ids: Vec::new(),
            })
            .expect("write commit metadata");

        let (witness, diagnostics) = preview(&CheckedProgram::default(), &store).expect("preview");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert_eq!(witness.store_source_digest, Some(String::new()));
        assert_eq!(witness.store_commit_id, Some(1));
    }
}
