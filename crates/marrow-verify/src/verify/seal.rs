//! Phases 3 to 5 over the decoded image, sealing it into a `VerifiedImage`.

use super::context::{CallGraph, Ctx, Effects, EntryKind, FnSig};
use super::durable::seal_root_indexes;
use super::model::DecodedImage;
use super::presence::{EntryFamilies, check_presence_flow, verify_function};
use super::reject;
use crate::reject::{Duplicate, RejectionKind as Kind, VerifyPhase, VerifyRejection};
use crate::sealed::{
    SealedExport, SealedFunction, SealedIndex, SealedInstr, SealedTestEntry, VerifiedImage,
};
use marrow_image::ImageType;
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
        sealed_roots: roots,
        sites,
        site_paths,
        durable_contract,
        semantic_nodes,
        consts,
        functions: decoded_functions,
        exports: decoded_exports,
        test_entries: decoded_test_entries,
    } = decoded;
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
    let entry_kinds = entry_kinds(functions.len(), &decoded_exports, &decoded_test_entries);
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

/// Each function's entry role, from the export and test-entry tables.
fn entry_kinds(
    count: usize,
    exports: &[(marrow_image::ExportId, u16)],
    test_entries: &[(u16, u16)],
) -> Vec<EntryKind> {
    let mut kinds = vec![EntryKind::Internal; count];
    for (_, func) in exports {
        kinds[usize::from(*func)] = EntryKind::Export;
    }
    for (_, func) in test_entries {
        let kind = &mut kinds[usize::from(*func)];
        *kind = match kind {
            EntryKind::Internal | EntryKind::Test => EntryKind::Test,
            EntryKind::Export | EntryKind::ExportedTest => EntryKind::ExportedTest,
        };
    }
    kinds
}

/// Test entries are unique by function, hold every `assert`, are never exports, take no
/// arguments and return unit, are never called, and open no session of their own: a body
/// performs no direct durable operation and calls a mutating function only through that
/// function's own transaction. The checks run in that order over the whole table.
fn check_test_entries(
    test_entries: &[(u16, u16)],
    strings: &[Rc<str>],
    functions: &[SealedFunction],
    kinds: &[EntryKind],
    effects: &Effects,
    calls: &CallGraph,
) -> Result<Vec<SealedTestEntry>, VerifyRejection> {
    let mut named = vec![false; functions.len()];
    for (_, func) in test_entries {
        if named[usize::from(*func)] {
            return Err(reject(
                VerifyPhase::TestEntry,
                Kind::Duplicate(Duplicate::TestEntryFunction),
            ));
        }
        named[usize::from(*func)] = true;
    }
    for (index, function) in functions.iter().enumerate() {
        let has_assert = function
            .instrs()
            .iter()
            .any(|instr| matches!(instr, SealedInstr::Assert));
        if has_assert && !kinds[index].is_test() {
            return Err(reject(VerifyPhase::TestEntry, Kind::AssertOutsideTest));
        }
    }
    for (_, func) in test_entries {
        let function = &functions[usize::from(*func)];
        if kinds[usize::from(*func)] == EntryKind::ExportedTest {
            return Err(reject(VerifyPhase::TestEntry, Kind::TestEntryExported));
        }
        if !function.params.is_empty() || function.ret != ImageType::Unit {
            return Err(reject(VerifyPhase::TestEntry, Kind::TestEntrySignature));
        }
    }
    for index in 0..functions.len() {
        for &callee in calls.callees(index) {
            if kinds[usize::from(callee)].is_test() {
                return Err(reject(VerifyPhase::TestEntry, Kind::TestEntryCalled));
            }
        }
    }
    for (_, func) in test_entries {
        let function = &functions[usize::from(*func)];
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
