use std::collections::HashSet;

use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogLifecycle};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::executable::checked_saved_root_place;
use crate::program::CheckedProgram;

use super::{Accumulator, catalog_id, required_catalog_id};
use crate::evolution::{RepairReason, Verdict};

/// Classify the accepted catalog entries current source no longer declares. A retire
/// intent reserves the proposal entry's spelling: dropping populated data is a
/// destructive decision that names the exact catalog id and count. A source-dropped
/// index deletes its derived cell subtree on apply. A member source merely stopped
/// declaring, with no retire and no dependent, is a dependency-free sparse-field drop:
/// a legal no-op whose data lingers. A dropped member an active index still reads
/// cannot be silently dropped; it needs an explicit retire intent that also removes or
/// rebinds the index.
pub(super) fn classify_absent_source_entries(
    program: &CheckedProgram,
    store: &TreeStore,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    let source_paths = crate::catalog::source_catalog_entries(program);
    let declared: HashSet<(CatalogEntryKind, &str)> = source_paths
        .iter()
        .map(|entry| (entry.kind, entry.path.as_str()))
        .collect();

    for entry in catalog_entries_for_drop_discharge(program) {
        if declared.contains(&(entry.kind, entry.path.as_str())) {
            continue;
        }
        let entry_id = catalog_id(&entry.stable_id)?;
        let is_index = entry.kind == CatalogEntryKind::StoreIndex;
        match absent_entry_state(program, entry) {
            AbsentEntryState::RetiredThisProposal if is_index => {
                acc.record(entry_id, Verdict::IndexDropped, true)?;
            }
            AbsentEntryState::RetiredThisProposal => {
                if retired_member_is_nested(program, entry) {
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "retiring `{}` drops a member nested under a group or keyed layer, which apply does not yet support; retire a top-level member instead",
                            entry.path
                        ),
                    );
                    acc.record(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::NestedRetireUnsupported,
                        },
                        is_index,
                    )?;
                } else {
                    let populated = populated_member_records(program, store, entry)?;
                    acc.record(
                        entry_id,
                        Verdict::DestructiveDecisionRequired { populated },
                        is_index,
                    )?;
                }
            }
            AbsentEntryState::Reserved => {}
            AbsentEntryState::Active | AbsentEntryState::Deprecated => {
                if is_index {
                    acc.record(entry_id, Verdict::IndexDropped, true)?;
                } else if let Some((index_name, index_id)) = index_depends_on(program, entry)? {
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "dropped `{}` is still used by index `{index_name}`; retire it with an evolve intent",
                            entry.path
                        ),
                    );
                    acc.record(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::RetireRequired { index: index_id },
                        },
                        false,
                    )?;
                } else {
                    acc.record(entry_id, Verdict::NoOp, false)?;
                }
            }
        }
    }
    Ok(())
}

/// The catalog entries discharge must consider for a source drop: the proposal
/// entries when source proposed a change, else the accepted entries. The proposal
/// already carries consumed retire reservations and the lingering still-active
/// entries, so it supersedes the accepted snapshot; when source proposed nothing,
/// the accepted entries are the snapshot to diff against.
fn catalog_entries_for_drop_discharge(program: &CheckedProgram) -> &[CatalogEntry] {
    match &program.catalog.proposal {
        Some(proposal) => &proposal.entries,
        None => &program.catalog.accepted_entries,
    }
}

#[derive(Clone, Copy)]
enum AbsentEntryState {
    Active,
    Deprecated,
    Reserved,
    RetiredThisProposal,
}

fn absent_entry_state(program: &CheckedProgram, entry: &CatalogEntry) -> AbsentEntryState {
    if entry.lifecycle == CatalogLifecycle::Reserved
        && program.catalog.proposal.is_some()
        && program.catalog.accepted_entries.iter().any(|accepted| {
            accepted.stable_id == entry.stable_id && accepted.lifecycle == CatalogLifecycle::Active
        })
    {
        return AbsentEntryState::RetiredThisProposal;
    }
    match entry.lifecycle {
        CatalogLifecycle::Active => AbsentEntryState::Active,
        CatalogLifecycle::Deprecated => AbsentEntryState::Deprecated,
        CatalogLifecycle::Reserved => AbsentEntryState::Reserved,
    }
}

/// Count records that carry a value for the dropped member identified by `entry`.
/// Only a resource-member entry holds per-record data; a store, index, or enum entry
/// has none to count. The records are streamed, never materialized.
fn populated_member_records(
    program: &CheckedProgram,
    store: &TreeStore,
    entry: &CatalogEntry,
) -> Result<usize, StoreError> {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return Ok(0);
    }
    let Some((store_id, member_id)) = dropped_member_addresses(program, entry)? else {
        return Ok(0);
    };
    let path = [DataPathSegment::Member(member_id)];
    let mut populated = 0;
    store.for_each_record(
        &store_id,
        owning_root_arity(program, entry),
        &mut |identity| {
            if store.data_subtree_exists(&store_id, identity, &path)? {
                populated += 1;
            }
            Ok(())
        },
    )?;
    Ok(populated)
}

/// The store and member catalog ids for a dropped resource-member entry. The store id
/// comes from the owning resource's store; the member id is the entry's own stable id,
/// since a dropped member's cells were written under that id.
fn dropped_member_addresses(
    program: &CheckedProgram,
    entry: &CatalogEntry,
) -> Result<Option<(CatalogId, CatalogId)>, StoreError> {
    let Some(root) = owning_root(program, entry) else {
        return Ok(None);
    };
    let Some(place) = checked_saved_root_place(program, &root, Default::default()) else {
        return Ok(None);
    };
    let store_id = required_catalog_id(&place.store_catalog_id)?;
    let member_id = catalog_id(&entry.stable_id)?;
    Ok(Some((store_id, member_id)))
}

/// The store root whose resource owns the dropped member, found by matching the
/// member path's resource prefix against a source store's resource. A member path is
/// `module::Resource::field...`; its resource prefix is the source resource path.
fn owning_root(program: &CheckedProgram, entry: &CatalogEntry) -> Option<String> {
    let resource_prefix = entry.path.rsplit_once("::").map(|(head, _)| head)?;
    program.modules.iter().find_map(|module| {
        module.stores.iter().find_map(|store| {
            let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
            (resource_path == resource_prefix).then(|| store.root.clone())
        })
    })
}

/// Whether a retired resource-member entry names a member nested under an unkeyed group
/// or a keyed layer rather than a top-level member of the record. The member chain is
/// everything after the owning resource path; a top-level member is a single segment,
/// while a nested member carries the group or layer segments before its own. A retired
/// member is gone from current source, so its nesting is read from its catalog path
/// against the owning source resource, not from the live member tree.
fn retired_member_is_nested(program: &CheckedProgram, entry: &CatalogEntry) -> bool {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return false;
    }
    program.modules.iter().any(|module| {
        module.stores.iter().any(|store| {
            let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
            entry
                .path
                .strip_prefix(&resource_path)
                .and_then(|tail| tail.strip_prefix("::"))
                .is_some_and(|member_chain| member_chain.contains("::"))
        })
    })
}

/// The identity arity of the store that owns the dropped member, or `1` when it
/// cannot be resolved (the common single-key store).
fn owning_root_arity(program: &CheckedProgram, entry: &CatalogEntry) -> usize {
    owning_root(program, entry)
        .and_then(|root| checked_saved_root_place(program, &root, Default::default()))
        .map(|place| place.identity_keys.len())
        .unwrap_or(1)
}

/// An active source index that reads the dropped member, as its developer-facing name
/// and its catalog identity. A dropped member an index still needs cannot be silently
/// deprecated. The name is for the diagnostic; the catalog id is the typed identity the
/// verdict carries across into apply. The index is matched on its source-declared key
/// columns, which still name the dropped member, and its stable id is read from the
/// catalog entry for the index path.
fn index_depends_on(
    program: &CheckedProgram,
    entry: &CatalogEntry,
) -> Result<Option<(String, CatalogId)>, StoreError> {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return Ok(None);
    }
    let Some((resource_prefix, member_name)) = entry.path.rsplit_once("::") else {
        return Ok(None);
    };
    let found = program.modules.iter().find_map(|module| {
        module.stores.iter().find_map(|store| {
            let store_resource = crate::catalog::resource_path(&module.name, &store.resource);
            if store_resource != resource_prefix {
                return None;
            }
            store
                .indexes
                .iter()
                .find(|index| index.args.iter().any(|arg| arg == member_name))
                .map(|index| {
                    (
                        index.name.clone(),
                        crate::catalog::store_index_path(&module.name, &store.root, &index.name),
                    )
                })
        })
    });
    let Some((index_name, index_path)) = found else {
        return Ok(None);
    };
    let Some(stable_id) = index_stable_id(program, &index_path) else {
        return Ok(None);
    };
    Ok(Some((index_name, catalog_id(&stable_id)?)))
}

/// The stable id of the store-index catalog entry at `path`, from the proposal when
/// source proposed a change, else the accepted snapshot. Both carry the index entry;
/// the proposal supersedes the accepted snapshot the same way the dropped-entry scan
/// chooses its source.
fn index_stable_id(program: &CheckedProgram, path: &str) -> Option<String> {
    catalog_entries_for_drop_discharge(program)
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::StoreIndex && entry.path == path)
        .map(|entry| entry.stable_id.clone())
}
