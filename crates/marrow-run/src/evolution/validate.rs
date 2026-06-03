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
    let current = preview(program, store).map_err(ApplyError::Store)?.0;
    if current != *witness {
        return Err(ApplyError::Drift);
    }
    Ok(())
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
