use std::collections::HashSet;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::executable::{checked_activation_root_places, for_each_place_record};
use crate::program::CheckedProgram;

use super::{Accumulator, catalog_id, required_catalog_id};
use crate::evolution::{RepairReason, Verdict};

/// Classify the accepted catalog entries current source no longer declares. A retire intent
/// makes dropping populated data a destructive decision naming the exact id and count; a
/// source-dropped index deletes its derived subtree on apply. A member or whole store merely
/// undeclared with no retire intent is a legal no-op only when it holds no records; a populated
/// drop with no retire intent, or a member an active index still reads, fails closed and needs
/// an explicit `evolve retire`, so populated data is never orphaned by a bare source diff. A
/// dropped whole resource takes its store with it, so the store entry — addressed by its own
/// accepted id — owns the single fence for the root, and its orphaned member entries stay
/// no-ops covered by it.
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

    // A source-dropped index is removed from the proposal outright, so it no longer appears in
    // the drop-discharge entries; its derived cells were written under its accepted stable id,
    // so the deletion obligation is read from the accepted snapshot here. The catalog binding
    // has already dropped its entry and advanced the epoch.
    for entry in &program.catalog.accepted_entries {
        if entry.kind != CatalogEntryKind::StoreIndex
            || entry.lifecycle != CatalogLifecycle::Active
            || declared.contains(&(entry.kind, entry.path.as_str()))
        {
            continue;
        }
        acc.push_index(catalog_id(&entry.stable_id)?, Verdict::IndexDropped)?;
    }

    for entry in catalog_entries_for_drop_discharge(program) {
        if entry.kind == CatalogEntryKind::StoreIndex
            || declared.contains(&(entry.kind, entry.path.as_str()))
        {
            continue;
        }
        let entry_id = catalog_id(&entry.stable_id)?;
        match absent_entry_state(program, entry) {
            AbsentEntryState::RetiredThisProposal => {
                if retired_member_is_nested(program, entry) {
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "retiring `{}` drops a member nested under a group or keyed layer, which apply does not yet support; retire a top-level member instead",
                            entry.path
                        ),
                    );
                    acc.push(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::NestedRetireUnsupported,
                        },
                    )?;
                } else {
                    let populated = populated_member_records(program, store, entry)?;
                    acc.push(entry_id, Verdict::DestructiveDecisionRequired { populated })?;
                }
            }
            AbsentEntryState::Reserved => {}
            AbsentEntryState::Active => {
                if let Some((index_name, index_id)) = index_depends_on(program, entry)? {
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "dropped `{}` is still used by index `{index_name}`; retire it with an evolve intent",
                            entry.path
                        ),
                    );
                    acc.push(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::RetireRequired { index: index_id },
                        },
                    )?;
                } else if dropped_store_holds_records(program, store, entry)? {
                    // Dropping a whole resource takes its store with it, so the now-absent
                    // member resolves no owning root and its own scan finds nothing — yet the
                    // store subtree still holds every record. Fence the store entry once,
                    // naming the root; the orphaned member entries stay no-ops below, covered
                    // by this fence. An empty store has nothing to lose and stays a no-op.
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "dropped store `{}` still holds records; retire it with `evolve retire {}` and apply with approval, or repair the data before activation",
                            entry.path, entry.path
                        ),
                    );
                    acc.push(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::PopulatedDropRequiresRetire,
                        },
                    )?;
                } else if populated_member_records(program, store, entry)? > 0 {
                    // A dropped member with no retire intent whose cells are still populated
                    // would orphan that data on a bare activation. Fence it closed until the
                    // developer states the destructive intent with `evolve retire`; an empty
                    // member has nothing to lose and stays a no-op below.
                    acc.diagnostic(
                        entry_id.clone(),
                        format!(
                            "dropped `{}` still holds stored data; retire it with `evolve retire {}` and apply with approval, or repair the data before activation",
                            entry.path, entry.path
                        ),
                    );
                    acc.push(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::PopulatedDropRequiresRetire,
                        },
                    )?;
                } else {
                    acc.push(entry_id, Verdict::NoOp)?;
                }
            }
        }
    }
    Ok(())
}

/// The catalog entries to consider for a source drop: the proposal entries when source proposed
/// a change, else the accepted snapshot. The proposal already carries consumed retire
/// reservations and lingering active entries, so it supersedes the accepted snapshot.
fn catalog_entries_for_drop_discharge(program: &CheckedProgram) -> &[CatalogEntry] {
    match &program.catalog.proposal {
        Some(proposal) => &proposal.entries,
        None => &program.catalog.accepted_entries,
    }
}

#[derive(Clone, Copy)]
enum AbsentEntryState {
    Active,
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
        CatalogLifecycle::Reserved => AbsentEntryState::Reserved,
    }
}

/// Whether a source-dropped store still holds at least one record, streamed and short-circuited
/// on the first one. A dropped store's cells were written under its own accepted stable id, so
/// the store is addressed by `entry.stable_id`, and the record arity is read from the accepted
/// identity-key shape (a keyless singleton or an unrecorded shape descends one level). Only a
/// store entry holds records; every other kind returns `false`. A store with no records has no
/// data to orphan and stays a free no-op.
fn dropped_store_holds_records(
    program: &CheckedProgram,
    store: &TreeStore,
    entry: &CatalogEntry,
) -> Result<bool, StoreError> {
    if entry.kind != CatalogEntryKind::Store {
        return Ok(false);
    }
    let store_id = catalog_id(&entry.stable_id)?;
    let arity = accepted_store_arity(program, &entry.stable_id);
    let mut found = false;
    store.for_each_record(&store_id, arity, &mut |_identity| {
        found = true;
        Ok(())
    })?;
    Ok(found)
}

/// The record arity of a store from its accepted identity-key shape: the count of comma-joined
/// key types the shape records (`int` is one, `int,string` is two). A keyless singleton renders
/// the empty shape and a store recorded before key shapes were tracked has none; both descend a
/// single identity level, matching [`TreeStore::for_each_record`]'s floor.
fn accepted_store_arity(program: &CheckedProgram, store_stable_id: &str) -> usize {
    program
        .catalog
        .accepted_entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::Store && entry.stable_id == store_stable_id)
        .and_then(|entry| entry.accepted_key_shape.as_deref())
        .filter(|shape| !shape.is_empty())
        .map_or(1, |shape| shape.split(',').count())
}

/// Count records that carry a value for the dropped member, streamed, never materialized. Only
/// a resource-member entry holds per-record data.
fn populated_member_records(
    program: &CheckedProgram,
    store: &TreeStore,
    entry: &CatalogEntry,
) -> Result<usize, StoreError> {
    if entry.kind != CatalogEntryKind::ResourceMember {
        return Ok(0);
    }
    let Some(resource_prefix) = dropped_member_resource_prefix(entry) else {
        return Ok(0);
    };
    let member_id = catalog_id(&entry.stable_id)?;
    let path = [DataPathSegment::Member(member_id)];
    let mut populated = 0;
    for place in places_owning_resource(program, resource_prefix) {
        let store_id = required_catalog_id(&place.store_catalog_id)?;
        for_each_place_record(store, &place, &mut |identity| {
            if store.data_subtree_exists(&store_id, identity, &path)? {
                populated += 1;
            }
            Ok(())
        })?;
    }
    Ok(populated)
}

fn dropped_member_resource_prefix(entry: &CatalogEntry) -> Option<&str> {
    entry.path.rsplit_once("::").map(|(head, _)| head)
}

fn places_owning_resource(
    program: &CheckedProgram,
    resource_prefix: &str,
) -> Vec<crate::CheckedSavedPlace> {
    let roots: HashSet<&str> = program
        .modules
        .iter()
        .flat_map(|module| {
            module.stores.iter().filter_map(|store| {
                let resource_path = crate::catalog::resource_path(&module.name, &store.resource);
                (resource_path == resource_prefix).then_some(store.root.as_str())
            })
        })
        .collect();
    checked_activation_root_places(program)
        .into_iter()
        .filter(|place| roots.contains(place.root.as_str()))
        .collect()
}

/// Whether a retired member is nested under a group or keyed layer rather than top-level. A
/// retired member is gone from current source, so its nesting is read from its catalog path
/// against the owning resource (a top-level member is a single trailing segment).
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

/// An active source index that reads the dropped member, as its name (for the diagnostic) and
/// catalog identity (carried into apply). Matched on the index's source-declared key columns,
/// which still name the dropped member.
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

/// The stable id of the store-index catalog entry at `path`, from the proposal when source
/// proposed a change, else the accepted snapshot, the same source the dropped-entry scan uses.
fn index_stable_id(program: &CheckedProgram, path: &str) -> Option<String> {
    catalog_entries_for_drop_discharge(program)
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::StoreIndex && entry.path == path)
        .map(|entry| entry.stable_id.clone())
}
