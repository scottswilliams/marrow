use marrow_schema::stdlib::Capability;

use super::target::saved_place;
use super::util::push_unique;
use crate::facts::{CheckedFacts, DirectEffectFacts, HostEffect};
use crate::{
    CheckedBody, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedInterpolationPart,
    CheckedSavedPlace, CheckedSavedTerminal, CheckedStmt,
};

pub(crate) fn direct_effects_for_block(
    facts: &CheckedFacts,
    body: &CheckedBody,
) -> DirectEffectFacts {
    let mut effects = DirectEffectFacts::default();
    collect_block_effects(facts, body, &mut effects);
    effects
}

fn collect_block_effects(
    facts: &CheckedFacts,
    body: &CheckedBody,
    effects: &mut DirectEffectFacts,
) {
    for statement in body.statements() {
        collect_statement_effects(facts, statement, effects);
    }
}

fn collect_statement_effects(
    facts: &CheckedFacts,
    statement: &CheckedStmt,
    effects: &mut DirectEffectFacts,
) {
    match statement {
        CheckedStmt::Const { value, .. } | CheckedStmt::Throw { value, .. } => {
            if matches!(statement, CheckedStmt::Throw { .. }) {
                effects.throws = true;
            }
            collect_expr_reads(facts, value, effects);
        }
        CheckedStmt::Var { value, .. } => {
            if let Some(value) = value {
                collect_expr_reads(facts, value, effects);
            }
        }
        CheckedStmt::Assign { target, value, .. } => {
            collect_saved_write(facts, target, effects);
            collect_saved_path_key_reads(facts, target, effects);
            collect_expr_reads(facts, value, effects);
        }
        CheckedStmt::Delete { path, .. } => {
            collect_saved_write(facts, path, effects);
            collect_saved_path_key_reads(facts, path, effects);
        }
        CheckedStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_expr_reads(facts, value, effects);
            }
        }
        CheckedStmt::Expr { value, .. } => collect_expr_reads(facts, value, effects),
        CheckedStmt::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                collect_expr_reads(facts, condition, effects);
            }
            collect_block_effects(facts, then_block, effects);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    collect_expr_reads(facts, condition, effects);
                }
                collect_block_effects(facts, &else_if.block, effects);
            }
            if let Some(block) = else_block {
                collect_block_effects(facts, block, effects);
            }
        }
        CheckedStmt::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_reads(facts, condition, effects);
            }
            collect_block_effects(facts, body, effects);
        }
        CheckedStmt::For {
            iterable,
            step,
            body,
            ..
        } => {
            collect_expr_reads(facts, iterable, effects);
            if let Some(step) = step {
                collect_expr_reads(facts, step, effects);
            }
            collect_block_effects(facts, body, effects);
        }
        CheckedStmt::Transaction { body, .. } => {
            effects.transactions = true;
            collect_block_effects(facts, body, effects);
        }
        CheckedStmt::Try {
            body,
            catch,
            finally,
            ..
        } => {
            collect_block_effects(facts, body, effects);
            if let Some(catch) = catch {
                collect_block_effects(facts, &catch.block, effects);
            }
            if let Some(finally) = finally {
                collect_block_effects(facts, finally, effects);
            }
        }
        CheckedStmt::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                collect_expr_reads(facts, scrutinee, effects);
            }
            for arm in arms {
                collect_block_effects(facts, &arm.block, effects);
            }
        }
        CheckedStmt::Break { .. } | CheckedStmt::Continue { .. } => {}
    }
}

fn collect_expr_reads(facts: &CheckedFacts, expr: &CheckedExpr, effects: &mut DirectEffectFacts) {
    if let Some(place) = expr.saved_place() {
        push_saved_read(facts, place, effects);
        collect_saved_path_key_reads(facts, expr, effects);
        return;
    }
    if let Some(effect) = host_effect(expr) {
        push_unique(&mut effects.host_calls, effect);
    }
    match expr {
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            if matches!(
                target,
                CheckedCallTarget::Builtin(CheckedBuiltinCall::Append)
            ) {
                if let Some((target, rest)) = args.split_first() {
                    collect_saved_write(facts, &target.value, effects);
                    collect_saved_path_key_reads(facts, &target.value, effects);
                    for arg in rest {
                        collect_expr_reads(facts, &arg.value, effects);
                    }
                }
                return;
            }
            if matches!(target, CheckedCallTarget::Function(_)) {
                effects.calls_user_function = true;
            }
            collect_expr_reads(facts, callee, effects);
            for arg in args {
                collect_expr_reads(facts, &arg.value, effects);
            }
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            collect_expr_reads(facts, base, effects);
        }
        CheckedExpr::Unary { operand, .. } => collect_expr_reads(facts, operand, effects),
        CheckedExpr::Binary { left, right, .. } => {
            collect_expr_reads(facts, left, effects);
            collect_expr_reads(facts, right, effects);
        }
        CheckedExpr::Interpolation { parts, .. } => {
            for part in parts {
                if let CheckedInterpolationPart::Expr(expr) = part {
                    collect_expr_reads(facts, expr, effects);
                }
            }
        }
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {}
    }
}

fn collect_saved_path_key_reads(
    facts: &CheckedFacts,
    expr: &CheckedExpr,
    effects: &mut DirectEffectFacts,
) {
    match expr {
        CheckedExpr::Call { callee, args, .. } => {
            collect_saved_path_key_reads(facts, callee, effects);
            for arg in args {
                collect_expr_reads(facts, &arg.value, effects);
            }
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            collect_saved_path_key_reads(facts, base, effects);
        }
        CheckedExpr::SavedRoot { .. }
        | CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::Unary { .. }
        | CheckedExpr::Binary { .. }
        | CheckedExpr::Interpolation { .. } => {}
    }
}

fn collect_saved_write(facts: &CheckedFacts, expr: &CheckedExpr, effects: &mut DirectEffectFacts) {
    if let Some(place) = expr.saved_place() {
        push_saved_write(facts, place, effects);
    }
}

fn push_saved_read(
    facts: &CheckedFacts,
    place: &CheckedSavedPlace,
    effects: &mut DirectEffectFacts,
) {
    if let Some(effect) = saved_effect(facts, place) {
        push_unique(&mut effects.saved_reads, effect);
    }
}

fn push_saved_write(
    facts: &CheckedFacts,
    place: &CheckedSavedPlace,
    effects: &mut DirectEffectFacts,
) {
    if let Some(effect) = saved_effect(facts, place) {
        push_unique(&mut effects.saved_writes, effect);
    }
}

fn saved_effect(
    facts: &CheckedFacts,
    place: &CheckedSavedPlace,
) -> Option<crate::SavedPlaceEffect> {
    saved_place(facts, &place.root, &effect_members(place))
}

fn effect_members(place: &CheckedSavedPlace) -> Vec<String> {
    let mut members: Vec<String> = place
        .layers
        .iter()
        .map(|layer| layer.name.clone())
        .collect();
    if let CheckedSavedTerminal::Field { name, .. } = &place.terminal {
        members.push(name.clone());
    }
    members
}

fn host_effect(expr: &CheckedExpr) -> Option<HostEffect> {
    let CheckedExpr::Call { target, .. } = expr else {
        return None;
    };
    match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Print | CheckedBuiltinCall::Write) => {
            Some(HostEffect::Output)
        }
        CheckedCallTarget::Std(target) => match target.capability {
            Capability::Pure => None,
            capability => Some(HostEffect::Capability(capability)),
        },
        _ => None,
    }
}
