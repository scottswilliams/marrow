//! The phased image verifier (design §E).
//!
//! Phases run in order; each consumes only prior output; every failure is a typed
//! [`VerifyRejection`], never a panic. The compiler emits image bytes but cannot
//! mint a [`VerifiedImage`]: this is the only path from bytes to a checked image,
//! and the sole `VerifiedImage` constructor.
//!
//! Coverage grows one slice at a time. The container framing and every table are
//! decoded in full; the per-function instruction set the interpreter admits is the
//! current subset, and an opcode whose vertical has not landed is a phase-3
//! rejection rather than a silent pass.

use marrow_image::{ExportDemand, image_id};

use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    RetShape, SealedEnumType, SealedExport, SealedField, SealedFunction, SealedIndex, SealedInstr,
    SealedRecordType, SealedRoot, SealedSite, SealedTestEntry, SealedVariant, TestKind,
    VerifiedImage,
};

mod code_tables;
mod context;
mod decode_code;
mod durable;
mod flow;
mod model;
mod presence;
mod spans;
mod tables;

use context::{Ctx, Effects, FnSig};
use flow::durable_op_class;
use presence::{call_targets, check_presence_flow, reject_call_cycles, verify_function};

use code_tables::{decode_consts, decode_exports, decode_functions, decode_spans};
use durable::{
    decode_durable, is_flat_executable_root, member_flat_at_root, resolve_index_projection,
    seal_branches, seal_groups,
};
use model::DecodedImage;
use tables::{
    decode_collections, decode_enums, decode_strings, decode_test_entries, decode_types,
    reject_value_type_cycles, validate_record_field_refs,
};

const MAGIC: &[u8; 4] = b"MWI\0";
const VERSION: u8 = 0x00;
const DIGEST_SLOT_END: usize = 37;

type Reject = VerifyRejection;

fn reject(phase: VerifyPhase, detail: &'static str) -> Reject {
    VerifyRejection::new(phase, detail)
}

/// Verify `bytes` into a sealed [`VerifiedImage`], or reject at the earliest phase
/// whose invariant the image violates.
pub fn verify(bytes: &[u8]) -> Result<VerifiedImage, VerifyRejection> {
    let decoded = decode_container(bytes)?;
    seal(decoded)
}

// ---------------------------------------------------------------------------
// Phase 1 (envelope) + phase 2 (table closure).
// ---------------------------------------------------------------------------

fn decode_container(bytes: &[u8]) -> Result<DecodedImage, VerifyRejection> {
    if bytes.len() > marrow_image::bounds::MAX_IMAGE_BYTES {
        return Err(reject(
            VerifyPhase::Envelope,
            "image exceeds the size bound",
        ));
    }
    let mut reader = Reader::new(bytes);
    let magic = reader
        .take(4)
        .ok_or(reject(VerifyPhase::Envelope, "short magic"))?;
    if magic != MAGIC {
        return Err(reject(VerifyPhase::Envelope, "bad magic"));
    }
    let version = reader
        .u8()
        .ok_or(reject(VerifyPhase::Envelope, "short version"))?;
    if version != VERSION {
        return Err(reject(VerifyPhase::Envelope, "unsupported version"));
    }
    let stored_digest = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Envelope, "short digest slot"))?;
    // Recompute the digest over the payload (every byte after the digest slot).
    let payload = &bytes[DIGEST_SLOT_END..];
    if image_id(payload).0.as_slice() != stored_digest {
        return Err(reject(VerifyPhase::Envelope, "digest mismatch"));
    }

    let section_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Envelope, "short section count"))?;
    if section_count != 10 {
        return Err(reject(VerifyPhase::Envelope, "section count must be 10"));
    }
    let mut sections: Vec<(u8, &[u8])> = Vec::with_capacity(10);
    let mut last_id = 0u8;
    for _ in 0..10 {
        let id = reader
            .u8()
            .ok_or(reject(VerifyPhase::Envelope, "short section id"))?;
        if id <= last_id {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must strictly ascend",
            ));
        }
        last_id = id;
        let len = reader
            .u32()
            .ok_or(reject(VerifyPhase::Envelope, "short section length"))?
            as usize;
        let body = reader
            .take(len)
            .ok_or(reject(VerifyPhase::Envelope, "section length past input"))?;
        sections.push((id, body));
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Envelope,
            "trailing bytes after sections",
        ));
    }
    // Section ids strictly ascend and there are exactly 10, so they are exactly 1..10.
    for (index, (id, _)) in sections.iter().enumerate() {
        if *id != (index as u8 + 1) {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must be exactly 1..10",
            ));
        }
    }

    // Phase 2: decode each table. Spans are decoded per function, in FUNCTIONS
    // order, so they are attached to the already-decoded function list.
    let strings = decode_strings(sections[0].1)?;
    let types = decode_types(sections[1].1, strings.len())?;
    let enums = decode_enums(sections[8].1, strings.len(), types.len())?;
    let collections = decode_collections(sections[9].1, types.len(), enums.len())?;
    validate_record_field_refs(&types, enums.len(), collections.len())?;
    reject_value_type_cycles(&types, &enums)?;
    let (roots, sites, site_paths, durable_contract, durable_descriptor) =
        decode_durable(sections[2].1, &strings, &types, &enums)?;
    let consts = decode_consts(sections[3].1, &strings)?;
    let mut functions = decode_functions(
        sections[4].1,
        strings.len(),
        types.len(),
        enums.len(),
        collections.len(),
        roots.len(),
    )?;
    let exports = decode_exports(sections[5].1, functions.len())?;
    decode_spans(sections[6].1, &mut functions)?;
    let test_entries = decode_test_entries(sections[7].1, strings.len(), functions.len())?;

    Ok(DecodedImage {
        image_id: image_id(payload),
        strings,
        types,
        enums,
        collections,
        roots,
        sites,
        site_paths,
        durable_contract,
        durable_descriptor,
        consts,
        functions,
        exports,
        test_entries,
    })
}

// ---------------------------------------------------------------------------
// Phase 3 (per-function structural/type/local-init) + phases 4-6.
// ---------------------------------------------------------------------------

fn seal(decoded: DecodedImage) -> Result<VerifiedImage, VerifyRejection> {
    let types: Vec<SealedRecordType> = decoded
        .types
        .iter()
        .map(|record| SealedRecordType {
            fields: record
                .fields
                .iter()
                .map(|field| SealedField {
                    name: decoded.strings[field.name as usize].clone(),
                    ty: field.ty,
                    required: field.required,
                })
                .collect(),
        })
        .collect();
    let enums: Vec<SealedEnumType> = decoded
        .enums
        .iter()
        .map(|enum_def| SealedEnumType {
            name: decoded.strings[enum_def.name as usize].clone(),
            variants: enum_def
                .variants
                .iter()
                .map(|variant| SealedVariant {
                    name: decoded.strings[variant.name as usize].clone(),
                    category: variant.category,
                    payload: variant.payload.clone(),
                })
                .collect(),
        })
        .collect();
    let roots: Vec<SealedRoot> = decoded
        .roots
        .iter()
        .map(|root| {
            let flat = is_flat_executable_root(root);
            // A flat-executable root's branches are all scalar-field keyed
            // branches, each carrying its own nested branches; seal the whole tree in
            // declaration order so a BranchEntry branch path indexes it level by level. A
            // non-flat root parks every branch site, so it needs no sealed branch list.
            let branches = if flat {
                seal_branches(&root.members, &decoded.strings)
            } else {
                Vec::new()
            };
            let groups = if flat {
                seal_groups(root, &types)
            } else {
                Vec::new()
            };
            SealedRoot {
                name: decoded.strings[root.name as usize].clone(),
                keys: root.keys.iter().map(|(scalar, _)| *scalar).collect(),
                record: root.record,
                // A root's members are extra-free when every direct member keeps it flat:
                // a field (scalar or widened composite), a root-level unkeyed group of
                // storable-value fields, or a simple branch. A nested/composite branch, or
                // a group nested below the root, is an extra that parks the root's
                // operations; a widened field no longer parks (it is framed inline). This
                // is a member-shape predicate independent of keyed-ness — a keyless
                // singleton parks separately.
                has_extras: !root.members.iter().all(member_flat_at_root),
                branches,
                groups,
            }
        })
        .collect();
    // The managed indexes seal from the decoded roots, each carrying the index of the
    // root it belongs to. Their projections were re-resolved against the decoded graph
    // in `decode_indexes`, so the sealed set trusts no image-side incidence summary. Each
    // ledger-id projection component also resolves to a record/key position here — the
    // form the path kernel maintains — against the same decoded root.
    let mut indexes: Vec<SealedIndex> = Vec::new();
    for (root_index, root) in decoded.roots.iter().enumerate() {
        for index in &root.indexes {
            let projection = resolve_index_projection(root, &index.components)?;
            indexes.push(SealedIndex {
                id: index.id,
                root: root_index as u16,
                unique: index.unique,
                components: index.components.clone(),
                projection,
            });
        }
    }
    let sites: Vec<SealedSite> = decoded.sites.clone();
    // Function signatures feed the per-function `Call` type check (phase 3).
    let signatures: Vec<FnSig> = decoded
        .functions
        .iter()
        .map(|function| FnSig {
            params: function.params.clone(),
            ret: function.ret,
        })
        .collect();
    let collections = decoded.collections.clone();
    let ctx = Ctx {
        types: &types,
        enums: &enums,
        collections: &collections,
        roots: &roots,
        sites: &sites,
        indexes: &indexes,
        signatures: &signatures,
    };
    let mut functions = Vec::with_capacity(decoded.functions.len());
    for function in &decoded.functions {
        functions.push(verify_function(function, &ctx, &decoded)?);
    }

    // Phase 4: the call graph over the recorded direct calls must be acyclic
    // (recursion is not admitted).
    reject_call_cycles(&functions)?;

    // Phase 4/5: closure-informed effect and transaction-flow validation. An export
    // entry that mutates in closure is the owner of a transaction.
    let effects = Effects::compute(&functions, &decoded.site_paths);
    let export_entries: Vec<bool> = {
        let mut entries = vec![false; functions.len()];
        for (_, func) in &decoded.exports {
            entries[*func as usize] = true;
        }
        entries
    };
    let test_entry_mask: Vec<bool> = {
        let mut entries = vec![false; functions.len()];
        for (_, func) in &decoded.test_entries {
            entries[*func as usize] = true;
        }
        entries
    };
    for (index, function) in functions.iter().enumerate() {
        effects.check_transaction_flow(
            index,
            function,
            export_entries[index],
            test_entry_mask[index],
        )?;
    }

    // Phase 5 (presence): every present-entry sparse set is dominated by a presence
    // fact on its key slot, rechecked independently of the compiler.
    for function in &functions {
        check_presence_flow(function, &ctx)?;
    }

    let exports = decoded
        .exports
        .iter()
        .map(|(id, func)| {
            let demand = effects.demand(*func);
            let demand_id = demand.demand_set_id();
            SealedExport {
                id: *id,
                func: *func,
                mutating: effects.mutates_closure[*func as usize],
                demand,
                demand_id,
                reachable_sites: effects.reachable_sites(*func),
            }
        })
        .collect();

    // Record each export's effect class on its entry function too, for tools.
    for (_, func) in &decoded.exports {
        functions[*func as usize].mutating = effects.mutates_closure[*func as usize];
    }

    let test_entries = check_test_entries(&decoded, &functions, &export_entries, &effects)?;

    // Per-function demand from the same effects owner, so a test-body driver can open
    // the session one export call requires without a second demand model.
    let function_demands: Vec<ExportDemand> = (0..functions.len() as u16)
        .map(|f| effects.demand(f))
        .collect();

    Ok(VerifiedImage {
        image_id: decoded.image_id,
        types,
        enums,
        collections,
        roots,
        indexes,
        sites,
        durable_contract: decoded.durable_contract,
        durable_descriptor: decoded.durable_descriptor,
        consts: decoded.consts,
        functions,
        exports,
        test_entries,
        function_demands,
    })
}

/// The test-entry phase (design §E extension): the TEST-ENTRY table names storeless
/// zero-argument entry points, `assert` is legal only inside one, and a test entry
/// is never an export, a mutating/reading closure, or a call target. Returns the
/// sealed entries in the table's ascending-name order.
fn check_test_entries(
    decoded: &DecodedImage,
    functions: &[SealedFunction],
    export_entries: &[bool],
    effects: &Effects,
) -> Result<Vec<SealedTestEntry>, VerifyRejection> {
    let mut is_test_entry = vec![false; functions.len()];
    for (_, func) in &decoded.test_entries {
        // The decoder proved every function index in range. Two names aliasing
        // one function would make the report double-count it; entries are unique
        // by function as well as by name.
        if is_test_entry[*func as usize] {
            return Err(reject(
                VerifyPhase::TestEntry,
                "duplicate test-entry function index",
            ));
        }
        is_test_entry[*func as usize] = true;
    }

    // `assert` may appear only in a test-entry function.
    for (index, function) in functions.iter().enumerate() {
        let has_assert = function
            .instrs()
            .iter()
            .any(|instr| matches!(instr, SealedInstr::Assert));
        if has_assert && !is_test_entry[index] {
            return Err(reject(
                VerifyPhase::TestEntry,
                "an assert instruction sits outside a test entry",
            ));
        }
    }

    // Each test entry is a storeless zero-argument entry point, disjoint from the
    // export table.
    for (_, func) in &decoded.test_entries {
        let function = &functions[*func as usize];
        if export_entries[*func as usize] {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry is also an export",
            ));
        }
        if !function.params.is_empty() {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry takes no parameters",
            ));
        }
        if function.ret != RetShape::Unit {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry must return unit",
            ));
        }
        // A test entry may touch durable data: its demand is recorded in the parallel
        // test-entry table below so an E01 ephemeral test attachment can bound its
        // authority by the test-image union. It is still never an export and carries
        // no wire identity.
    }

    // A test entry is an entry point: no function may call one.
    for function in functions {
        for callee in call_targets(function) {
            if is_test_entry[callee] {
                return Err(reject(
                    VerifyPhase::TestEntry,
                    "a test entry may not be called",
                ));
            }
        }
    }

    // A test body is one of two disjoint kinds: it performs durable operations
    // directly (running in the harness session) or it drives exports, where each
    // export call is its own invocation boundary. Mixing the two — a direct durable
    // op together with a call to a transaction owner — is refused: the owner's commit
    // would consume the harness session out from under the direct op, and no single
    // session can carry both. The compiler reports the same shape at check time; this
    // is the independent artifact-level mirror.
    for (_, func) in &decoded.test_entries {
        let function = &functions[*func as usize];
        let has_direct_durable = function
            .instrs()
            .iter()
            .any(|instr| durable_op_class(instr).is_some());
        let drives_owner = call_targets(function)
            .iter()
            .any(|&callee| effects.has_begin[callee]);
        if has_direct_durable && drives_owner {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test body performs a direct durable operation and also drives a \
                 transaction-owning export",
            ));
        }
    }

    Ok(decoded
        .test_entries
        .iter()
        .map(|(name, func)| {
            let demand = effects.demand(*func);
            let kind = if demand.is_empty() {
                TestKind::Storeless
            } else if functions[*func as usize]
                .instrs()
                .iter()
                .any(|instr| durable_op_class(instr).is_some())
            {
                TestKind::DirectDurable
            } else {
                TestKind::Driver
            };
            SealedTestEntry {
                name: decoded.strings[*name as usize].clone(),
                func: *func,
                demand,
                kind,
            }
        })
        .collect())
}
