use marrow_check::evolution::EvolutionWitness;
use marrow_store::tree::{CommitMetadata, TreeStore};

use super::super::apply::ApplyError;
use super::super::window::current_engine_profile;

pub(super) fn verify_proposal_identity(
    witness: &EvolutionWitness,
    store: &TreeStore,
    commit: &CommitMetadata,
) -> Result<(), ApplyError> {
    let Some(proposal) = &witness.proposal_catalog else {
        return Err(ApplyError::Drift);
    };
    let profile = current_engine_profile();
    if commit.catalog_epoch != proposal.epoch
        || commit.layout_epoch != profile.layout_epoch()
        || commit.engine_profile_digest != profile.digest_bytes()
        || commit.source_digest.is_empty()
        || commit.source_digest != witness.source_digest
        || commit.activation_evolution_digest.is_empty()
        || commit.activation_evolution_digest != witness.evolution_digest
        || commit.activation_proposal_catalog_digest.as_deref() != Some(proposal.digest.as_str())
        || commit.changed_root_catalog_ids != witness.changed_root_catalog_ids
        || commit.changed_index_catalog_ids != witness.changed_index_catalog_ids
    {
        return Err(ApplyError::Drift);
    }
    if store.read_layout_epoch()? != Some(profile.layout_epoch())
        || store.read_engine_profile_digest()? != Some(profile.digest_bytes())
    {
        return Err(ApplyError::Drift);
    }
    Ok(())
}
