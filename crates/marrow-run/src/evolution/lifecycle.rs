use marrow_check::{CatalogEntryKind, CatalogLifecycle, CheckedProgram};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;

use super::apply::ApplyError;

pub(super) fn retired_proposal_ids(
    program: &CheckedProgram,
    kind: CatalogEntryKind,
) -> Result<Vec<CatalogId>, ApplyError> {
    let mut ids = Vec::new();
    let Some(proposal) = &program.catalog.proposal else {
        return Ok(ids);
    };
    for entry in &proposal.entries {
        if entry.kind == kind
            && entry.lifecycle == CatalogLifecycle::Reserved
            && accepted_active_entry(program, entry.stable_id.as_str(), kind)
        {
            ids.push(CatalogId::new(entry.stable_id.clone()).map_err(|_| {
                ApplyError::Store(StoreError::Corruption {
                    message: "evolution proposal contains an invalid retired catalog id"
                        .to_string(),
                })
            })?);
        }
    }
    Ok(ids)
}

fn accepted_active_entry(
    program: &CheckedProgram,
    stable_id: &str,
    kind: CatalogEntryKind,
) -> bool {
    program.catalog.accepted_entries.iter().any(|accepted| {
        accepted.kind == kind
            && accepted.stable_id == stable_id
            && accepted.lifecycle == CatalogLifecycle::Active
    })
}
