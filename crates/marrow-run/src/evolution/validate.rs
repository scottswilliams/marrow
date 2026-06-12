//! Exact preview-witness validation before any evolution write is staged.

use marrow_check::CheckedProgram;
use marrow_check::evolution::{EvolutionWitness, preview};
use marrow_store::tree::TreeStore;

use super::apply::ApplyError;

/// Re-run preview over the live source/catalog/store and require byte-for-byte witness
/// equality. The commit id is checked explicitly so the concurrency pin is visible even
/// though it also participates in witness equality.
pub(super) fn validate_witness(
    witness: &EvolutionWitness,
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), ApplyError> {
    assert_commit_pin(witness, store)?;
    assert_accepted_catalog_pin(witness, store)?;
    let current = preview(program, store).map_err(ApplyError::Store)?.0;
    if current != *witness {
        return Err(ApplyError::Drift);
    }
    Ok(())
}

/// Confirm the store's published accepted-catalog snapshot is the one the witness was
/// built against. The witness discharged its obligations over `accepted_catalog`; if the
/// store's published rows drifted from that digest, staging against the witness would
/// write a shape the store no longer accepts. A store with no published snapshot predates
/// its baseline (a fresh store the first apply adopts), so there is nothing to pin yet.
pub(super) fn assert_accepted_catalog_pin(
    witness: &EvolutionWitness,
    store: &TreeStore,
) -> Result<(), ApplyError> {
    let Some(snapshot) = store.read_catalog_snapshot()? else {
        return Ok(());
    };
    if snapshot.digest == witness.accepted_catalog.digest {
        return Ok(());
    }
    let found = marrow_check::evolution::CatalogFingerprint {
        epoch: snapshot.epoch,
        digest: snapshot.digest,
    };
    if witness.proposal_catalog.is_none()
        && witness.store_catalog.as_ref() == Some(&found)
        && found.epoch < witness.accepted_catalog.epoch
    {
        return Ok(());
    }
    Err(ApplyError::CatalogDrift {
        pinned: witness.accepted_catalog.digest.clone(),
        found: Some(found.digest),
    })
}

pub(super) fn assert_commit_pin(
    witness: &EvolutionWitness,
    store: &TreeStore,
) -> Result<(), ApplyError> {
    let found = store.read_commit_metadata()?.map(|commit| commit.commit_id);
    if found != witness.store_commit_id {
        return Err(ApplyError::StoreCommitDrift {
            pinned: witness.store_commit_id,
            found,
        });
    }
    Ok(())
}
