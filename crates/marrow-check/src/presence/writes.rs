//! Transitive effect closure over lowered direct-effect facts.

use crate::{
    CheckedArg, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedFunctionRef,
    CheckedInterpolationPart, CheckedProgram,
};

use super::util::extend_unique;
use crate::executable::accepted_saved_place;
use crate::facts::{DirectEffectFacts, EffectClosureFacts, StoreId};

pub(crate) fn effect_closure(
    program: &CheckedProgram,
    function_ref: CheckedFunctionRef,
) -> Option<EffectClosureFacts> {
    program.facts.function_for_ref(function_ref)?;
    let mut closure = EffectClosureFacts::default();
    collect_function_closure(program, function_ref, &mut closure, &mut Vec::new());
    set_write_reachability(&mut closure);
    Some(closure)
}

pub(crate) fn effect_closure_for_direct(
    program: &CheckedProgram,
    direct: &DirectEffectFacts,
) -> EffectClosureFacts {
    let mut closure = EffectClosureFacts::default();
    extend_closure(&mut closure, direct);
    let mut visited = Vec::new();
    for callee in &direct.user_function_calls {
        collect_function_closure(program, *callee, &mut closure, &mut visited);
    }
    set_write_reachability(&mut closure);
    closure
}

/// The stores a user-function call actually writes a saved record or index entry to. A
/// `nextId` allocation reports a `stores_written` store effect but commits no record, so
/// that coarse set cannot be used: only a record write (`saved_writes`, keyed by the
/// written resource) or an index write (`saved_index_writes`, keyed by its index) counts.
/// Returning the precise written stores lets a caller advance just those allocation
/// cohorts, so a helper that allocates from one store but writes another never suppresses
/// a collision on the store it left untouched.
pub(super) fn call_written_stores(
    program: &CheckedProgram,
    target: &CheckedCallTarget,
) -> Vec<StoreId> {
    let CheckedCallTarget::Function(function_ref) = target else {
        return Vec::new();
    };
    let Some(closure) = effect_closure(program, *function_ref) else {
        return Vec::new();
    };
    let mut stores: Vec<StoreId> = Vec::new();
    for write in &closure.saved_writes {
        for store in program.facts.stores() {
            if store.resource == write.resource && !stores.contains(&store.id) {
                stores.push(store.id);
            }
        }
    }
    for index in &closure.saved_index_writes {
        let store = program.facts.store_index(*index).store;
        if !stores.contains(&store) {
            stores.push(store);
        }
    }
    stores
}

fn call_writes_saved_data(
    program: &CheckedProgram,
    target: &CheckedCallTarget,
    args: &[CheckedArg],
) -> bool {
    match target {
        // `append` writes saved data only when its target is a saved layer; an append to
        // a purely local sequence mutates no node, so it must not expire saved narrowings.
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Append) => args
            .first()
            .is_some_and(|arg| accepted_saved_place(&arg.value).is_some()),
        CheckedCallTarget::Function(function_ref) => effect_closure(program, *function_ref)
            .is_some_and(|closure| closure.write_effects_reachable),
        CheckedCallTarget::SavedIndexLookup
        | CheckedCallTarget::SavedLayerRead
        | CheckedCallTarget::SavedResourceRead
        | CheckedCallTarget::IdentityConstructor(_)
        | CheckedCallTarget::ErrorConstructor
        | CheckedCallTarget::Builtin(_)
        | CheckedCallTarget::Std(_)
        | CheckedCallTarget::ResourceConstructor(_)
        | CheckedCallTarget::LocalCollection { .. } => false,
    }
}

fn collect_function_closure(
    program: &CheckedProgram,
    function_ref: CheckedFunctionRef,
    closure: &mut EffectClosureFacts,
    visited: &mut Vec<CheckedFunctionRef>,
) {
    if visited.contains(&function_ref) {
        return;
    }
    let Some(function) = program.facts.function_for_ref(function_ref) else {
        return;
    };
    visited.push(function_ref);
    let direct = function.direct_effects.clone();
    extend_closure(closure, &direct);
    for callee in direct.user_function_calls {
        collect_function_closure(program, callee, closure, visited);
    }
}

pub(super) fn expr_calls_saved_writer(program: &CheckedProgram, expr: &CheckedExpr) -> bool {
    match expr {
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            call_writes_saved_data(program, target, args)
                || expr_calls_saved_writer(program, callee)
                || args
                    .iter()
                    .any(|arg| expr_calls_saved_writer(program, &arg.value))
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            expr_calls_saved_writer(program, base)
        }
        CheckedExpr::Unary { operand, .. } => expr_calls_saved_writer(program, operand),
        CheckedExpr::Binary { left, right, .. } => {
            expr_calls_saved_writer(program, left) || expr_calls_saved_writer(program, right)
        }
        CheckedExpr::Range {
            start, end, step, ..
        } => [start.as_deref(), end.as_deref(), step.as_deref()]
            .into_iter()
            .flatten()
            .any(|part| expr_calls_saved_writer(program, part)),
        CheckedExpr::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            CheckedInterpolationPart::Text { .. } => false,
            CheckedInterpolationPart::Expr(expr) => expr_calls_saved_writer(program, expr),
        }),
        CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::SavedRoot { .. }
        | CheckedExpr::Absent { .. } => false,
    }
}

fn extend_closure(closure: &mut EffectClosureFacts, direct: &DirectEffectFacts) {
    extend_unique(&mut closure.saved_reads, direct.saved_reads.clone());
    extend_unique(&mut closure.stores_read, direct.store_reads.clone());
    extend_unique(
        &mut closure.saved_index_reads,
        direct.saved_index_reads.clone(),
    );
    extend_unique(&mut closure.saved_writes, direct.saved_writes.clone());
    extend_unique(&mut closure.stores_written, direct.store_writes.clone());
    extend_unique(
        &mut closure.saved_index_writes,
        direct.saved_index_writes.clone(),
    );
    extend_unique(
        &mut closure.indexes_touched,
        direct.saved_index_reads.clone(),
    );
    extend_unique(
        &mut closure.indexes_touched,
        direct.saved_index_writes.clone(),
    );
    closure.transactions |= direct.transactions;
    extend_unique(&mut closure.host_calls, direct.host_calls.clone());
    closure.unindexed_collection_reads |= direct.unindexed_collection_reads;
    closure.throws |= direct.throws;
}

fn set_write_reachability(closure: &mut EffectClosureFacts) {
    closure.write_effects_reachable = !closure.saved_writes.is_empty()
        || !closure.stores_written.is_empty()
        || !closure.saved_index_writes.is_empty();
}
