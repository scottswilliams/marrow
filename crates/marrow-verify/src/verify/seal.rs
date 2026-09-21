//! Phases 3 to 5 over the decoded image, sealing it into a `VerifiedImage`.

use super::context::{CallGraph, Ctx, Effects, EntryKind, FnSig};
use super::durable::{
    is_flat_executable_root, member_flat_at_root, seal_branches, seal_groups, seal_root_indexes,
};
use super::model::{DecodedImage, DecodedRoot};
use super::presence::{EntryFamilies, check_presence_flow, verify_function};
use super::reject;
use crate::reject::{Duplicate, RejectionKind as Kind, VerifyPhase, VerifyRejection};
use crate::sealed::{
    SealedExport, SealedFunction, SealedIndex, SealedInstr, SealedRecordType, SealedRoot,
    SealedTestEntry, VerifiedImage,
};
use marrow_image::{ImageType, TypeId};
use std::rc::Rc;

pub(super) fn seal(decoded: DecodedImage) -> Result<VerifiedImage, VerifyRejection> {
    let DecodedImage {
        durable_graph,
        image_id,
        strings,
        types,
        enums,
        collections,
        roots: decoded_roots,
        sites,
        site_paths,
        durable_contract,
        semantic_nodes,
        consts,
        functions: decoded_functions,
        exports: decoded_exports,
        test_entries: decoded_test_entries,
    } = decoded;
    let roots = seal_roots(&decoded_roots, &strings, &types);
    // Each index seals against the one root that declared it, so its projection resolves
    // to that occurrence's record and key positions and no other's.
    let mut indexes: Vec<SealedIndex> = Vec::new();
    for (root_index, root) in decoded_roots.iter().enumerate() {
        indexes.extend(seal_root_indexes(root_index as u16, root)?);
    }
    let signatures: Vec<FnSig> = decoded_functions
        .iter()
        .map(|function| FnSig {
            params: function.params.clone(),
            ret: function.ret,
        })
        .collect();
    let ctx = Ctx {
        types: &types,
        enums: &enums,
        collections: &collections,
        roots: &roots,
        sites: &sites,
        indexes: &indexes,
        signatures: &signatures,
    };
    let mut functions = Vec::with_capacity(decoded_functions.len());
    let mut non_fallthrough_entries = Vec::with_capacity(decoded_functions.len());
    for function in &decoded_functions {
        let (verified, entries) = verify_function(function, &ctx, &consts, &strings)?;
        functions.push(verified);
        non_fallthrough_entries.push(entries);
    }

    let calls = CallGraph::new(&functions)?;
    let effects = Effects::compute(&functions, &site_paths, &calls);
    let entry_kinds = entry_kinds(functions.len(), &decoded_exports, &decoded_test_entries)?;
    for (index, function) in functions.iter().enumerate() {
        effects.check_transaction_flow(index, function, &calls, entry_kinds[index])?;
    }

    let entry_families = EntryFamilies::new(&sites, &site_paths);
    for (function, entries) in functions.iter().zip(non_fallthrough_entries) {
        check_presence_flow(function, &ctx, &entries, &effects, &entry_families)?;
    }

    let exports = decoded_exports
        .iter()
        .map(|(id, func)| {
            let demand = effects.demands.get(usize::from(*func));
            SealedExport {
                id: *id,
                func: *func,
                mutating: effects.mutates_closure[*func as usize],
                demand_id: demand.demand_set_id(),
                reachable_sites: effects.reachable_sites(*func),
            }
        })
        .collect();
    for (_, func) in &decoded_exports {
        functions[*func as usize].mutating = effects.mutates_closure[*func as usize];
    }

    let test_entries = check_test_entries(
        &decoded_test_entries,
        &strings,
        &functions,
        &entry_kinds,
        &effects,
        &calls,
    )?;

    Ok(VerifiedImage {
        durable_graph,
        image_id,
        types,
        enums,
        collections,
        roots,
        indexes,
        sites,
        durable_contract,
        semantic_nodes,
        consts,
        functions,
        exports,
        test_entries,
        function_demands: effects.demands,
    })
}

/// Each function's entry role, from the export and test-entry tables. A function is at
/// most one of them: a test entry is never an export, and two test names never alias one
/// function (the report would double-count it).
fn entry_kinds(
    count: usize,
    exports: &[(marrow_image::ExportId, u16)],
    test_entries: &[(u16, u16)],
) -> Result<Vec<EntryKind>, VerifyRejection> {
    let mut kinds = vec![EntryKind::Internal; count];
    for (_, func) in exports {
        kinds[usize::from(*func)] = EntryKind::Export;
    }
    for (_, func) in test_entries {
        match kinds[usize::from(*func)] {
            EntryKind::Internal => kinds[usize::from(*func)] = EntryKind::Test,
            EntryKind::Export => {
                return Err(reject(VerifyPhase::TestEntry, Kind::TestEntryExported));
            }
            EntryKind::Test => {
                return Err(reject(
                    VerifyPhase::TestEntry,
                    Kind::Duplicate(Duplicate::TestEntryFunction),
                ));
            }
        }
    }
    Ok(kinds)
}

/// Test entries take no arguments, return unit, are never called, hold every `assert`,
/// and open no session of their own: a body performs no direct durable operation and
/// calls a mutating function only through that function's own transaction.
fn check_test_entries(
    test_entries: &[(u16, u16)],
    strings: &[Rc<str>],
    functions: &[SealedFunction],
    kinds: &[EntryKind],
    effects: &Effects,
    calls: &CallGraph,
) -> Result<Vec<SealedTestEntry>, VerifyRejection> {
    for (index, function) in functions.iter().enumerate() {
        let has_assert = function
            .instrs()
            .iter()
            .any(|instr| matches!(instr, SealedInstr::Assert));
        if has_assert && kinds[index] != EntryKind::Test {
            return Err(reject(VerifyPhase::TestEntry, Kind::AssertOutsideTest));
        }
        for &callee in calls.callees(index) {
            if kinds[usize::from(callee)] == EntryKind::Test {
                return Err(reject(VerifyPhase::TestEntry, Kind::TestEntryCalled));
            }
        }
    }
    for (_, func) in test_entries {
        let function = &functions[usize::from(*func)];
        if !function.params.is_empty() || function.ret != ImageType::Unit {
            return Err(reject(VerifyPhase::TestEntry, Kind::TestEntrySignature));
        }
        if function
            .instrs()
            .iter()
            .any(|instr| instr.operation_class().is_some())
        {
            return Err(reject(VerifyPhase::TestEntry, Kind::TestDirectDurable));
        }
        for &callee in calls.callees(usize::from(*func)) {
            let callee = usize::from(callee);
            if effects.mutates_closure[callee] && !effects.has_begin[callee] {
                return Err(reject(
                    VerifyPhase::TestEntry,
                    Kind::TestCallsUnownedMutation,
                ));
            }
        }
    }
    Ok(test_entries
        .iter()
        .map(|(name, func)| SealedTestEntry {
            name: strings[usize::from(*name)].clone(),
            func: *func,
        })
        .collect())
}

/// A flat-executable root carries its branch tree and groups; a non-flat root parks every
/// branch and group site, so it needs neither list.
fn seal_roots(
    roots: &[DecodedRoot],
    strings: &[Rc<str>],
    types: &[SealedRecordType],
) -> Vec<SealedRoot> {
    roots
        .iter()
        .map(|root| {
            let flat = is_flat_executable_root(root);
            SealedRoot {
                name: strings[root.name as usize].clone(),
                keys: root.keys.iter().map(|(scalar, _)| *scalar).collect(),
                record: TypeId::from_index(root.record),
                has_extras: !root.members.iter().all(member_flat_at_root),
                branches: if flat {
                    seal_branches(&root.members, strings)
                } else {
                    Vec::new()
                },
                groups: if flat {
                    seal_groups(root, types)
                } else {
                    Vec::new()
                },
            }
        })
        .collect()
}
