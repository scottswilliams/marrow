//! Per-record checked transform execution.
//!
//! A transform recomputes one saved member per record from the members its body reads
//! via `old.<member>`. The checker proved the body pure, total, and well-typed against
//! the current schema, and discharge proved every read member's stored bytes decode
//! under their current type, so binding those decoded values as `old` and running the
//! body is sound. This module stages one `WriteData` of the encoded result at the
//! target cell per record, inside the apply transaction. It reuses the runtime
//! evaluator (no second interpreter), the canonical stored-value codec (no second
//! decoder), and the shared read-member resolver (no second classifier); it owns only
//! the wiring that binds `old` and encodes the result.
//!
//! Two failure shapes are kept distinct. A wiring resolution that discharge already
//! gated (the transform, its body, its place, or its owning module not resolving) is a
//! discharge/apply divergence: it cannot happen under a witness discharge approved, so
//! it is a loud internal fault, not a silent skip. A genuine per-record body fault (an
//! overflow, a divide-by-zero, an explicit throw, an absent optional read, or a value
//! the target leaf cannot encode) is the developer's pure logic faulting on real data;
//! it aborts apply with a typed [`ApplyError::TransformBodyFaulted`] so the transaction
//! rolls back without conflating a body fault with store-byte corruption.

use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::evolution::{TransformReadMember, transform_read_members};
use marrow_check::{
    CheckedProgram, CheckedRuntimeModule, CheckedRuntimeProgram, CheckedSavedMember,
    CheckedSavedMemberKind, CheckedSavedPlace, EvolveTransform, StoreLeafKind,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_syntax::SourceSpan;

use crate::activation::{Completion, Invocation, invoke};
use crate::env::{Context, TransactionState};
use crate::host::Host;
use crate::store::DataAddress;
use crate::value::{Value, decode_leaf, value_to_leaf};
use crate::write_plan::PlanStep;

use super::apply::{ApplyError, StagedWork, for_each_place_record, store_id};

pub(super) struct TransformStage<'a> {
    pub(super) target_id: &'a CatalogId,
    pub(super) witness_reads: &'a [CatalogId],
    pub(super) program: &'a CheckedProgram,
    pub(super) runtime: &'a CheckedRuntimeProgram,
    pub(super) places: &'a [CheckedSavedPlace],
    pub(super) store: &'a TreeStore,
    pub(super) steps: &'a mut Vec<PlanStep>,
    pub(super) staged: &'a mut StagedWork,
}

pub(super) struct TransformVisit<'a> {
    pub(super) target_id: &'a CatalogId,
    pub(super) witness_reads: &'a [CatalogId],
    pub(super) program: &'a CheckedProgram,
    pub(super) runtime: &'a CheckedRuntimeProgram,
    pub(super) places: &'a [CheckedSavedPlace],
    pub(super) store: &'a TreeStore,
    pub(super) visit: &'a mut dyn FnMut(DataAddress, Vec<u8>) -> Result<(), ApplyError>,
}

/// Stage one `WriteData` of the recomputed value at the transform target cell for every
/// record. For each record the read members' decoded values bind `old`, the pure body
/// runs through the runtime evaluator, and the returned value encodes to the target's
/// leaf type. The body never writes, so the only durable effect is the staged target
/// write; a body that raises, returns no value, or yields a value the target cannot
/// encode aborts apply with a typed body fault.
pub(super) fn stage_transform(ctx: TransformStage<'_>) -> Result<(), ApplyError> {
    let mut count = 0usize;
    let mut visit = |address, value| {
        ctx.steps.push(PlanStep::WriteData { address, value });
        count += 1;
        Ok(())
    };
    visit_transform_writes(TransformVisit {
        target_id: ctx.target_id,
        witness_reads: ctx.witness_reads,
        program: ctx.program,
        runtime: ctx.runtime,
        places: ctx.places,
        store: ctx.store,
        visit: &mut visit,
    })?;
    ctx.staged.records_transformed += count;
    Ok(())
}

pub(super) fn visit_transform_writes(ctx: TransformVisit<'_>) -> Result<(), ApplyError> {
    let transform = ctx
        .program
        .catalog
        .evolve_transforms
        .iter()
        .find(|transform| transform.catalog_id.as_deref() == Some(ctx.target_id.as_str()))
        .ok_or_else(|| diverged("no bound transform for the target the witness names"))?;
    let body = transform
        .runtime_body()
        .ok_or_else(|| diverged("the transform body did not lower"))?;
    let targets = locate_targets(ctx.places, ctx.target_id);
    if targets.is_empty() {
        return Err(diverged(
            "the transform target is not a top-level field of any place",
        ));
    }
    let module = owning_module(ctx.program, ctx.runtime, transform)
        .ok_or_else(|| diverged("the transform owning module is not in the runtime program"))?;

    // A body fault is a per-record `ApplyError`, but the shared record scan threads only
    // `StoreError`. The fault is captured here and the scan stops staging further writes
    // once it is set; apply returns it before any write commits, so the staged steps are
    // discarded and the store is untouched.
    let mut body_fault: Option<ApplyError> = None;
    for (place, target_leaf) in targets {
        let reads = transform_read_members(place, &transform.reads);
        if reads.len() != ctx.witness_reads.len()
            || reads
                .iter()
                .zip(ctx.witness_reads)
                .any(|(read, expected)| read.catalog_id != *expected)
        {
            return Err(diverged(
                "the transform read members no longer match the witness proof",
            ));
        }
        let sid = store_id(place)?;
        let target_path = [DataPathSegment::Member(ctx.target_id.clone())];
        for_each_place_record(ctx.store, place, &mut |identity| {
            if body_fault.is_some() {
                return Ok(());
            }
            let old = bind_old(ctx.runtime, ctx.store, &sid, identity, &reads)?;
            match recompute(ctx.runtime, ctx.store, module, body, old, &target_leaf) {
                Ok(bytes) => {
                    let address = DataAddress::from_resolved_parts(
                        sid.clone(),
                        identity.to_vec(),
                        target_path.to_vec(),
                    );
                    if let Err(error) = (ctx.visit)(address, bytes) {
                        body_fault = Some(error);
                    }
                }
                Err(reason) => {
                    body_fault = Some(ApplyError::TransformBodyFaulted {
                        target: ctx.target_id.clone(),
                        reason,
                    });
                }
            }
            Ok(())
        })?;
        if body_fault.is_some() {
            break;
        }
    }
    if let Some(fault) = body_fault {
        return Err(fault);
    }
    Ok(())
}

/// Run the body over one record's `old` binding and encode the result to the target
/// leaf, returning the bytes to stage or the reason the body faulted on this record.
fn recompute(
    runtime: &CheckedRuntimeProgram,
    store: &TreeStore,
    module: &CheckedRuntimeModule,
    body: &marrow_check::CheckedBody,
    old: Value,
    target_leaf: &StoreLeafKind,
) -> Result<Vec<u8>, String> {
    let value = run_transform(runtime, store, module, body, old)?;
    encode_target(value, target_leaf)
}

/// Build the `old` resource value for one record: each read member's stored bytes
/// decoded to its runtime value, keyed by the member name the body reads. A member with
/// no cell in this record is simply absent from the resource, so `old.<member>` raises
/// the ordinary absent fault the checker already accounts for. Bytes that fail to decode
/// are a store-corruption fault: discharge proved every read member decodes, so a
/// failure here means the store changed under the witness, which the apply drift guard
/// has already ruled out.
fn bind_old(
    runtime: &CheckedRuntimeProgram,
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    reads: &[TransformReadMember],
) -> Result<Value, StoreError> {
    let mut fields = Vec::new();
    for read in reads {
        let path = [DataPathSegment::Member(read.catalog_id.clone())];
        let Some(bytes) = store.read_data_value(store_id, identity, &path)? else {
            continue;
        };
        let Some(value) = decode_leaf(runtime, &bytes, &read.leaf) else {
            return Err(StoreError::Corruption {
                message: format!(
                    "transform input `{}` did not decode under its current type",
                    read.name
                ),
            });
        };
        fields.push((read.name.clone(), value));
    }
    Ok(Value::Resource(fields))
}

/// Run the transform body with `old` bound, returning the value it computes or a reason
/// the body faulted. The body is checker-proven pure, so it makes no saved reads against
/// the store the evaluator carries and stages no writes. A body that throws, faults, or
/// returns no value is a per-record body fault: the body is total and value-returning by
/// type, so reaching one of these means the developer's logic faulted over real data.
fn run_transform(
    runtime: &CheckedRuntimeProgram,
    store: &TreeStore,
    module: &CheckedRuntimeModule,
    body: &marrow_check::CheckedBody,
    old: Value,
) -> Result<Value, String> {
    let output = Rc::new(RefCell::new(String::new()));
    let host = Host::new();
    let ctx = Context {
        program: runtime,
        store,
        host: &host,
        transaction: Rc::new(RefCell::new(TransactionState::default())),
    };
    let completion = invoke(Invocation {
        ctx,
        output,
        module: Some(module),
        param_names: &["old"],
        body,
        span: body.span(),
        args: &[old],
        writeback: &[],
        traversed_layers: &[],
        hook: None,
        depth: 1,
    })
    .map_err(|error| error.message)?;
    match completion.0 {
        Completion::Returned(Some(value)) => Ok(value),
        Completion::Returned(None) => Err("the transform body returned no value".to_string()),
        Completion::Threw { .. } | Completion::Faulted { .. } => {
            Err("the transform body raised an error".to_string())
        }
    }
}

/// Encode a transform result to the target member's leaf bytes, or a reason the value
/// could not be held by the target leaf. The checker proved the body's result types as
/// the target, so a failure here means the value lost its type through dynamic
/// evaluation: a body fault, not store corruption.
fn encode_target(value: Value, leaf: &StoreLeafKind) -> Result<Vec<u8>, String> {
    let leaf_value =
        value_to_leaf(value, leaf, SourceSpan::default()).map_err(|error| error.message)?;
    leaf_value.bytes().map_err(|error| error.to_string())
}

/// Every place and target leaf for a transform target catalog id, when the target is a
/// top-level plain field of the resource shape used by one or more stores.
fn locate_targets<'a>(
    places: &'a [CheckedSavedPlace],
    target_id: &CatalogId,
) -> Vec<(&'a CheckedSavedPlace, StoreLeafKind)> {
    places
        .iter()
        .filter_map(|place| {
            place
                .root_members
                .iter()
                .find(|member| member.catalog_id.as_deref() == Some(target_id.as_str()))
                .and_then(target_leaf)
                .map(|leaf| (place, leaf))
        })
        .collect()
}

/// The leaf kind of a transform target member, when it is a plain scalar/enum/identity
/// field. A group target has no leaf to encode to.
fn target_leaf(member: &CheckedSavedMember) -> Option<StoreLeafKind> {
    matches!(member.kind, CheckedSavedMemberKind::Field { .. })
        .then(|| member.leaf.clone())
        .flatten()
}

/// The runtime module that owns a transform, resolved by the resource the transform
/// reshapes. The owning module name is derived once in the checker
/// ([`CheckedProgram::owning_module_name`]); the runtime never re-splits a path to find
/// it.
fn owning_module<'a>(
    program: &CheckedProgram,
    runtime: &'a CheckedRuntimeProgram,
    transform: &EvolveTransform,
) -> Option<&'a CheckedRuntimeModule> {
    let name = program.owning_module_name(&transform.resource)?;
    runtime.modules().iter().find(|module| module.name == name)
}

/// A discharge/apply divergence: apply reached a transform staging step discharge had
/// already gated as resolvable. It cannot happen under a witness whose discharge passed,
/// so it is a loud internal fault rather than a silent skip.
fn diverged(detail: &str) -> ApplyError {
    ApplyError::Store(StoreError::Corruption {
        message: format!("evolution apply transform staging diverged from discharge: {detail}"),
    })
}
