use std::collections::{HashMap, HashSet};

use marrow_catalog::{CatalogEntry, CatalogEntryKind};
use marrow_store::StoreError;

use crate::evolution::witness::{RepairReason, Verdict};
use crate::executable::CheckedSavedPlace;
use crate::program::CheckedProgram;

use super::{Accumulator, required_catalog_id};

/// Stable ids of catalog entries the proposal changes (new, retired, moved, or a store-index
/// declaration shape edit), each tagged with its kind so the accumulator partitions an index
/// from a data root without re-classifying it.
pub(super) fn proposal_changed_catalog_ids(
    program: &CheckedProgram,
) -> Vec<(String, CatalogEntryKind)> {
    let Some(proposal) = &program.catalog.proposal else {
        return Vec::new();
    };
    let accepted: HashMap<(CatalogEntryKind, &str), &CatalogEntry> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| ((entry.kind, entry.path.as_str()), entry))
        .collect();
    proposal
        .entries
        .iter()
        .filter(
            |entry| match accepted.get(&(entry.kind, entry.path.as_str())) {
                Some(prior) => {
                    prior.stable_id != entry.stable_id
                        || prior.lifecycle != entry.lifecycle
                        || (entry.kind == CatalogEntryKind::StoreIndex
                            && prior.accepted_index_shape != entry.accepted_index_shape)
                }
                None => true,
            },
        )
        .map(|entry| (entry.stable_id.clone(), entry.kind))
        .collect()
}

/// Stable ids of catalog entries a rename moved this cycle, of any kind — a store root, a
/// resource member, or an enum member — detected by a proposal entry whose alias set gained
/// a path the accepted entry lacked. A rename moves catalog identity only — the cells stay
/// under the same stable id — but moving a populated store, member, or enum-value spelling
/// re-addresses stored data, so this set drives both the member-rename `CatalogOnly` verdict
/// and the re-address count the run-time auto-apply set excludes.
pub(super) fn renamed_catalog_ids(program: &CheckedProgram) -> HashSet<String> {
    let Some(proposal) = &program.catalog.proposal else {
        return HashSet::new();
    };
    let accepted_aliases: HashMap<&str, &[String]> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| (entry.stable_id.as_str(), entry.aliases.as_slice()))
        .collect();
    proposal
        .entries
        .iter()
        .filter(|entry| {
            let accepted = accepted_aliases
                .get(entry.stable_id.as_str())
                .copied()
                .unwrap_or(&[]);
            entry.aliases.iter().any(|alias| !accepted.contains(alias))
        })
        .map(|entry| entry.stable_id.clone())
        .collect()
}

/// Enum ids — the leaf-carried identity a [`StoreLeafKind::Enum`] holds — that gained a
/// renamed member this cycle. An enum-member rename re-addresses the stored spelling of every
/// record holding that enum, so a place whose leaf names one of these enums re-addresses its
/// records. The owning enum of a renamed `EnumMember` is resolved through the member facts,
/// the single owner of the member-id-to-enum-id mapping.
pub(super) fn enum_ids_with_renamed_member(
    program: &CheckedProgram,
    renamed: &HashSet<String>,
) -> HashSet<crate::facts::EnumId> {
    program
        .facts
        .enum_members()
        .iter()
        .filter_map(|member| {
            let catalog_id = member.catalog_id.as_ref()?;
            renamed.contains(catalog_id).then_some(member.enum_id)
        })
        .collect()
}

/// Enum ids whose stored values an identity-preserving rename carries forward this cycle:
/// either the enum type itself was renamed (its own entry is in `renamed`) or one of its
/// members was. Such an enum's stored value stays valid even though its member paths moved,
/// so the orphaned-value check must not fail it closed.
pub(super) fn enum_ids_rename_covered(
    program: &CheckedProgram,
    renamed: &HashSet<String>,
    renamed_member_enum_ids: &HashSet<crate::facts::EnumId>,
) -> HashSet<crate::facts::EnumId> {
    let mut covered = renamed_member_enum_ids.clone();
    covered.extend(program.facts.enums().iter().filter_map(|enum_fact| {
        let catalog_id = enum_fact.catalog_id.as_ref()?;
        renamed.contains(catalog_id).then_some(enum_fact.id)
    }));
    covered
}

/// Accepted identity-aware leaf token for each resource member, keyed by raw catalog id:
/// `Some(token)` when the entry was a leaf, `None` when it was a non-leaf. A member absent
/// from the map is brand-new. Discharge compares this against the declared token to catch a
/// leaf type change the new decoder might otherwise reinterpret silently.
pub(super) fn accepted_member_leaves(program: &CheckedProgram) -> HashMap<String, Option<String>> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
        .map(|entry| {
            (
                entry.stable_id.clone(),
                entry.accepted_leaf_token().map(str::to_string),
            )
        })
        .collect()
}

/// Accepted structural signature for each resource member that records one, keyed by raw
/// catalog id. A member with no recorded signature carries no baseline, so the backstop never
/// fires against it; the proposal freezes the current signature forward so a later change has
/// one. The backstop fails closed only against a recorded baseline the current source diverges
/// from.
pub(super) fn accepted_member_structs(program: &CheckedProgram) -> HashMap<String, String> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::ResourceMember)
        .filter_map(|entry| {
            entry
                .accepted_struct
                .clone()
                .map(|signature| (entry.stable_id.clone(), signature))
        })
        .collect()
}

/// Accepted identity-key shape for each store that records one, keyed by raw catalog id. A
/// store with no recorded shape is absent: there is no baseline, and the proposal freezes the
/// current shape forward so the next cycle has one.
pub(super) fn accepted_store_key_shapes(program: &CheckedProgram) -> HashMap<String, String> {
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::Store)
        .filter_map(|entry| {
            entry
                .accepted_key_shape
                .clone()
                .map(|shape| (entry.stable_id.clone(), shape))
        })
        .collect()
}

/// Fail closed when a store's declared identity-key shape no longer matches the shape its
/// records were keyed under, returning whether such a re-key was detected. Identity keys live
/// in the saved path itself, so a record under the old key bytes is unreachable under the new
/// shape. v0.1 has no graceful store-key migration, so this is `RepairRequired` rather than a
/// silent activation that would orphan every record.
pub(super) fn classify_store_key_shape(
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
    accepted_key_shapes: &HashMap<String, String>,
    acc: &mut Accumulator,
) -> Result<bool, StoreError> {
    let Some(store_catalog_id) = place.store_catalog_id.as_deref() else {
        return Ok(false);
    };
    let Some(accepted) = accepted_key_shapes.get(store_catalog_id) else {
        return Ok(false);
    };
    let Some(declared) = program
        .catalog
        .declared_store_key_shapes
        .get(store_catalog_id)
    else {
        return Ok(false);
    };
    if accepted == declared {
        return Ok(false);
    }
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    acc.diagnostic(
        store_id.clone(),
        format!(
            "store `^{}` changed its identity key shape from `{accepted}` to `{declared}`; v0.1 does not support migrating an identity key shape over saved data, so this fails closed. Existing records are keyed by the old shape and cannot be addressed by the new one — model a new store and migrate with maintenance code instead",
            place.root
        ),
    );
    acc.push(
        store_id,
        Verdict::RepairRequired {
            reason: RepairReason::StoreKeyShapeChange,
        },
    )?;
    Ok(true)
}
