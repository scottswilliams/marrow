use std::collections::HashSet;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::catalog::SourceCatalogEntry;
use crate::executable::{checked_activation_root_places, for_each_place_record};
use crate::program::CheckedProgram;

use super::{Accumulator, catalog_id, required_catalog_id};
use crate::evolution::{RepairGuidance, RepairReason, Verdict};

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
    // has already dropped its entry and advanced the epoch. A rename keeps the same stable id
    // under a new path, so it is keyed on the still-declared id rather than the old path text:
    // a renamed index is rebuilt under its new path, not dropped.
    let declared_index_ids: HashSet<&str> = catalog_entries_for_drop_discharge(program)
        .iter()
        .filter(|entry| {
            entry.kind == CatalogEntryKind::StoreIndex
                && entry.lifecycle == CatalogLifecycle::Active
        })
        .map(|entry| entry.stable_id.as_str())
        .collect();
    for entry in &program.catalog.accepted_entries {
        if entry.kind != CatalogEntryKind::StoreIndex
            || entry.lifecycle != CatalogLifecycle::Active
            || declared_index_ids.contains(entry.stable_id.as_str())
        {
            continue;
        }
        acc.push_index(catalog_id(&entry.stable_id)?, Verdict::IndexDropped)?;
    }

    for entry in absent_source_entries(program) {
        if entry.kind == CatalogEntryKind::StoreIndex
            || declared.contains(&(entry.kind, entry.path.as_str()))
        {
            continue;
        }
        let entry_id = catalog_id(&entry.stable_id)?;
        match absent_entry_state(program, entry) {
            AbsentEntryState::RetiredThisProposal => {
                if entry.kind == CatalogEntryKind::Store {
                    let populated = dropped_store_record_count(store, entry)?;
                    acc.push(entry_id, Verdict::DestructiveDecisionRequired { populated })?;
                } else if retired_member_is_nested(program, entry) {
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
                            "dropped `{}` is still used by index `{index_name}`; {}",
                            entry.path,
                            retire_via_evolve_block(&entry.path)
                        ),
                    );
                    acc.push(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::RetireRequired { index: index_id },
                        },
                    )?;
                } else if dropped_store_holds_data(store, entry)? {
                    // Dropping a whole resource takes its store with it, so the now-absent
                    // member resolves no owning root and its own scan finds nothing — yet the
                    // store subtree still holds every record. Fence the store entry once,
                    // naming the root; the orphaned member entries stay no-ops below, covered
                    // by this fence. An empty store has nothing to lose and stays a no-op.
                    let diagnostic = dropped_store_diagnostic(entry);
                    acc.diagnostic_with_guidance(
                        entry_id.clone(),
                        diagnostic.message,
                        diagnostic.guidance,
                    );
                    acc.push(
                        entry_id,
                        Verdict::RepairRequired {
                            reason: RepairReason::PopulatedDropRequiresRetire,
                        },
                    )?;
                } else if populated_member_records(program, store, entry)? > 0 {
                    let diagnostic =
                        populated_member_drop_diagnostic(program, store, &source_paths, entry)?;
                    acc.diagnostic_with_guidance(
                        entry_id.clone(),
                        diagnostic.message,
                        diagnostic.guidance,
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

/// The catalog entries to classify for a source drop. A retire reserves its entry in the
/// proposal, so the proposal carries it (its `Reserved` lifecycle is what distinguishes a retire
/// from a bare removal). A bare removal drops its entry from the proposal entirely — the
/// projection no longer records it, exactly as a reseed would not — so it is recovered from the
/// accepted snapshot here, where its still-`Active` lifecycle drives the populated-drop fence. An
/// entry the proposal still carries is read from the proposal so the retire state is seen.
fn absent_source_entries(program: &CheckedProgram) -> impl Iterator<Item = &CatalogEntry> {
    let proposal_ids: HashSet<&str> = catalog_entries_for_drop_discharge(program)
        .iter()
        .map(|entry| entry.stable_id.as_str())
        .collect();
    let dropped_from_proposal = program
        .catalog
        .accepted_entries
        .iter()
        .filter(move |entry| !proposal_ids.contains(entry.stable_id.as_str()));
    catalog_entries_for_drop_discharge(program)
        .iter()
        .chain(dropped_from_proposal)
}

struct AbsentRepairDiagnostic {
    message: String,
    guidance: RepairGuidance,
}

/// The single owner of the retire-via-evolve-block remediation prose. It frames the retire as an
/// in-source `evolve` block, points at the scaffold to print the exact block, and names `evolve
/// apply` as the command — never `evolve retire` as if it were a CLI subcommand. Callers append a
/// populated-data clause where one is warranted.
fn retire_via_evolve_block(path: &str) -> String {
    format!(
        "add an in-source evolve block that retires `{path}` (run `marrow evolve preview --scaffold <projectdir>` to print the exact block), then apply it with `marrow evolve apply <projectdir>`"
    )
}

fn dropped_store_diagnostic(entry: &CatalogEntry) -> AbsentRepairDiagnostic {
    AbsentRepairDiagnostic {
        message: format!(
            "dropped store `{}` still holds records; {} and approval, or repair the data first",
            entry.path,
            retire_via_evolve_block(&entry.path)
        ),
        guidance: RepairGuidance::Retire {
            target: entry.path.clone(),
        },
    }
}

fn populated_member_drop_diagnostic(
    program: &CheckedProgram,
    store: &TreeStore,
    source_paths: &[SourceCatalogEntry],
    entry: &CatalogEntry,
) -> Result<AbsentRepairDiagnostic, StoreError> {
    let retire_guidance = format!(
        "{} and approval, or repair the data first",
        retire_via_evolve_block(&entry.path)
    );
    if let Some(target) = plausible_bare_rename_target(program, store, source_paths, entry)? {
        return Ok(AbsentRepairDiagnostic {
            message: format!(
                "dropped `{}` still holds stored data; if this was a rename, add an in-source evolve block that renames `{}` to `{target}` (run `marrow evolve preview --scaffold <projectdir>` to print it), then apply it with `marrow evolve apply <projectdir>`. Otherwise {retire_guidance}",
                entry.path, entry.path
            ),
            guidance: RepairGuidance::RenameOrRetire {
                from: entry.path.clone(),
                to: target,
            },
        });
    }
    Ok(AbsentRepairDiagnostic {
        message: format!(
            "dropped `{}` still holds stored data; {retire_guidance}",
            entry.path
        ),
        guidance: RepairGuidance::Retire {
            target: entry.path.clone(),
        },
    })
}

fn plausible_bare_rename_target(
    program: &CheckedProgram,
    store: &TreeStore,
    source_paths: &[SourceCatalogEntry],
    dropped: &CatalogEntry,
) -> Result<Option<String>, StoreError> {
    if dropped.kind != CatalogEntryKind::ResourceMember {
        return Ok(None);
    }
    let Some(resource_path) = member_resource_path(program, &dropped.path) else {
        return Ok(None);
    };
    let Some(dropped_leaf) = dropped.accepted_leaf_token() else {
        return Ok(None);
    };
    if !populated_drop_side_is_unique(program, store, source_paths, &resource_path, dropped_leaf)? {
        return Ok(None);
    }
    let mut candidates = source_paths
        .iter()
        .filter(|source| source.kind == CatalogEntryKind::ResourceMember)
        .filter(|source| member_belongs_to_resource(&source.path, &resource_path))
        .filter(|source| source_added_member(program, &source.path))
        .filter(|source| {
            source_member_leaf_token(program, &source.path).is_some_and(|leaf| leaf == dropped_leaf)
        })
        .map(|source| source.path.as_str());
    let Some(candidate) = candidates.next() else {
        return Ok(None);
    };
    Ok(candidates.next().is_none().then(|| candidate.to_string()))
}

fn populated_drop_side_is_unique(
    program: &CheckedProgram,
    store: &TreeStore,
    source_paths: &[SourceCatalogEntry],
    resource_path: &str,
    leaf_token: &str,
) -> Result<bool, StoreError> {
    let mut matches = 0;
    for entry in absent_source_entries(program) {
        if entry.kind != CatalogEntryKind::ResourceMember
            || !member_belongs_to_resource(&entry.path, resource_path)
            || entry.accepted_leaf_token() != Some(leaf_token)
            || source_declares_member(source_paths, &entry.path)
            || !matches!(absent_entry_state(program, entry), AbsentEntryState::Active)
        {
            continue;
        }
        if populated_member_records(program, store, entry)? > 0 {
            matches += 1;
            if matches > 1 {
                return Ok(false);
            }
        }
    }
    Ok(matches == 1)
}

fn source_added_member(program: &CheckedProgram, path: &str) -> bool {
    !program.catalog.accepted_entries.iter().any(|entry| {
        entry.kind == CatalogEntryKind::ResourceMember
            && entry.lifecycle == CatalogLifecycle::Active
            && entry.path == path
    })
}

fn source_declares_member(source_paths: &[SourceCatalogEntry], path: &str) -> bool {
    source_paths
        .iter()
        .any(|source| source.kind == CatalogEntryKind::ResourceMember && source.path == path)
}

fn source_member_leaf_token<'a>(program: &'a CheckedProgram, path: &str) -> Option<&'a str> {
    let stable_id = catalog_entries_for_drop_discharge(program)
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember
                && entry.lifecycle == CatalogLifecycle::Active
                && entry.path == path
        })
        .map(|entry| entry.stable_id.as_str())?;
    program
        .catalog
        .declared_member_structs
        .get(stable_id)
        .map(String::as_str)
        .and_then(marrow_catalog::structural_signature_leaf_token)
}

fn member_resource_path(program: &CheckedProgram, member_path: &str) -> Option<String> {
    program
        .modules
        .iter()
        .flat_map(|module| {
            module
                .resources
                .iter()
                .map(|resource| crate::catalog::resource_path(&module.name, &resource.name))
        })
        .filter(|resource_path| member_belongs_to_resource(member_path, resource_path))
        .max_by_key(String::len)
}

fn member_belongs_to_resource(member_path: &str, resource_path: &str) -> bool {
    member_path
        .strip_prefix(resource_path)
        .and_then(|tail| tail.strip_prefix("::"))
        .is_some()
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

/// Whether a source-dropped store still holds data. A dropped store's cells were written under
/// its own accepted stable id, so no current source shape is needed to prove that activation
/// would orphan them.
fn dropped_store_holds_data(store: &TreeStore, entry: &CatalogEntry) -> Result<bool, StoreError> {
    Ok(dropped_store_record_count(store, entry)? > 0)
}

fn dropped_store_record_count(
    store: &TreeStore,
    entry: &CatalogEntry,
) -> Result<usize, StoreError> {
    if entry.kind != CatalogEntryKind::Store {
        return Ok(0);
    }
    let store_id = catalog_id(&entry.stable_id)?;
    store.data_record_count(&store_id)
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
