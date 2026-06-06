use marrow_check::evolution::EvolutionWitness;

use super::super::admission::has_repair_verdict;
use super::super::apply::ApplyError;

pub(super) fn verify_no_repair_verdicts(witness: &EvolutionWitness) -> Result<(), ApplyError> {
    if has_repair_verdict(witness) {
        return Err(ApplyError::NotActivatable);
    }
    Ok(())
}
