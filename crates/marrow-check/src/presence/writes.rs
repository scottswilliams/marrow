//! Transitive effect closure over lowered direct-effect facts.

use crate::{
    CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedFunctionRef,
    CheckedInterpolationPart, CheckedProgram,
};

use super::util::extend_unique;
use crate::facts::{DirectEffectFacts, EffectClosureFacts};

pub(crate) fn effect_closure(
    program: &CheckedProgram,
    function_ref: CheckedFunctionRef,
) -> Option<EffectClosureFacts> {
    program.facts.function_for_ref(function_ref)?;
    let mut closure = EffectClosureFacts::default();
    collect_function_closure(program, function_ref, &mut closure, &mut Vec::new());
    closure.write_effects_reachable = !closure.stores_written.is_empty();
    Some(closure)
}

pub(super) fn call_writes_saved_data(program: &CheckedProgram, target: &CheckedCallTarget) -> bool {
    call_target_writes_saved_data(program, target)
}

fn call_target_writes_saved_data(program: &CheckedProgram, target: &CheckedCallTarget) -> bool {
    match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Append) => true,
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
            call_target_writes_saved_data(program, target)
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
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {
            false
        }
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
    closure.throws |= direct.throws;
}
