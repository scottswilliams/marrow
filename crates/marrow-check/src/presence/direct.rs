use super::util::push_unique;
use crate::executable::{
    accepted_saved_place, checked_saved_index_read, checked_saved_place_effect,
};
use crate::facts::{
    CheckedFacts, DirectEffectFacts, HostEffect, ResourceMemberId, StoreId, StoreIndexId,
    StoreIndexKeySource,
};
use crate::{
    CheckedBody, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedInterpolationPart,
    CheckedSavedTerminal, CheckedStmt,
};

pub(crate) fn direct_effects_for_block(
    facts: &CheckedFacts,
    body: &CheckedBody,
) -> DirectEffectFacts {
    let mut effects = DirectEffectFacts::default();
    collect_block_effects(facts, body, &mut effects);
    effects
}

pub(crate) fn direct_effects_for_expr(
    facts: &CheckedFacts,
    expr: &CheckedExpr,
) -> DirectEffectFacts {
    let mut effects = DirectEffectFacts::default();
    collect_expr_reads(facts, expr, &mut effects);
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
        CheckedStmt::CompoundAssign { target, value, .. } => {
            collect_saved_write(facts, target, effects);
            collect_expr_reads(facts, target, effects);
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
        CheckedStmt::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            collect_expr_reads(facts, value, effects);
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
            if saved_non_index_path(iterable) {
                effects.unindexed_collection_reads = true;
            }
            if let Some(step) = step {
                collect_expr_reads(facts, step, effects);
            }
            collect_block_effects(facts, body, effects);
        }
        CheckedStmt::Transaction { body, .. } => {
            effects.transactions = true;
            collect_block_effects(facts, body, effects);
        }
        CheckedStmt::Try { body, catch, .. } => {
            collect_block_effects(facts, body, effects);
            if let Some(catch) = catch {
                collect_block_effects(facts, &catch.block, effects);
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
    if unindexed_collection_lookup(expr) {
        effects.unindexed_collection_reads = true;
    }
    if let Some(place) = accepted_saved_place(expr) {
        push_unique(&mut effects.store_reads, place.store_id);
        if let Some(effect) = checked_saved_place_effect(facts, place) {
            push_unique(&mut effects.saved_reads, effect);
        }
        if let Some(index) = checked_saved_index_read(place) {
            push_unique(&mut effects.saved_index_reads, index);
        }
        collect_saved_path_key_reads(facts, expr, effects);
        return;
    }
    if expr.saved_place().is_some() {
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
                CheckedCallTarget::Builtin(CheckedBuiltinCall::NextId)
            ) {
                if let Some(target) = args.first()
                    && let Some(place) = target.value.saved_place()
                {
                    push_unique(&mut effects.store_writes, place.store_id);
                }
                for arg in args {
                    collect_expr_reads(facts, &arg.value, effects);
                }
                return;
            }
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
            if let CheckedCallTarget::Function(function_ref) = target {
                push_unique(&mut effects.user_function_calls, *function_ref);
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
        CheckedExpr::Range {
            start, end, step, ..
        } => {
            for part in [start.as_deref(), end.as_deref(), step.as_deref()]
                .into_iter()
                .flatten()
            {
                collect_expr_reads(facts, part, effects);
            }
        }
        CheckedExpr::Interpolation { parts, .. } => {
            for part in parts {
                if let CheckedInterpolationPart::Expr(expr) = part {
                    collect_expr_reads(facts, expr, effects);
                }
            }
        }
        CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::SavedRoot { .. }
        | CheckedExpr::Absent { .. } => {}
    }
}

pub(super) fn unindexed_collection_lookup(expression: &CheckedExpr) -> bool {
    let CheckedExpr::Call { target, args, .. } = expression else {
        return false;
    };
    let CheckedCallTarget::Builtin(builtin) = target else {
        return false;
    };
    unindexed_collection_builtin(*builtin)
        && args
            .first()
            .is_some_and(|arg| saved_non_index_path(&arg.value))
}

fn unindexed_collection_builtin(builtin: CheckedBuiltinCall) -> bool {
    matches!(
        builtin,
        CheckedBuiltinCall::Exists
            | CheckedBuiltinCall::Count
            | CheckedBuiltinCall::Keys
            | CheckedBuiltinCall::Values
            | CheckedBuiltinCall::Entries
            | CheckedBuiltinCall::Reversed
            | CheckedBuiltinCall::Next
            | CheckedBuiltinCall::Prev
    )
}

pub(super) fn saved_non_index_path(expression: &CheckedExpr) -> bool {
    expression
        .saved_place()
        .is_some_and(|place| !matches!(place.terminal, CheckedSavedTerminal::Index { .. }))
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
        | CheckedExpr::Absent { .. }
        | CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::Unary { .. }
        | CheckedExpr::Binary { .. }
        | CheckedExpr::Range { .. }
        | CheckedExpr::Interpolation { .. } => {}
    }
}

fn collect_saved_write(facts: &CheckedFacts, expr: &CheckedExpr, effects: &mut DirectEffectFacts) {
    if let Some(place) = accepted_saved_place(expr) {
        push_unique(&mut effects.store_writes, place.store_id);
        if let Some(index) = checked_saved_index_read(place) {
            push_unique(&mut effects.saved_index_writes, index);
        }
        if let Some(effect) = checked_saved_place_effect(facts, place) {
            for index in indexes_touched_by_write(facts, place.store_id, &effect) {
                push_unique(&mut effects.saved_index_writes, index);
            }
            push_unique(&mut effects.saved_writes, effect);
        }
    }
}

fn indexes_touched_by_write(
    facts: &CheckedFacts,
    store_id: StoreId,
    effect: &crate::SavedPlaceEffect,
) -> Vec<StoreIndexId> {
    facts
        .store_indexes()
        .iter()
        .filter(|index| index.store == store_id)
        .filter(|index| index_touched_by_write(facts, index, effect))
        .map(|index| index.id)
        .collect()
}

fn index_touched_by_write(
    facts: &CheckedFacts,
    index: &crate::StoreIndexFact,
    effect: &crate::SavedPlaceEffect,
) -> bool {
    if effect.members.is_empty() {
        return true;
    }
    index.keys.iter().any(|key| match key.source {
        StoreIndexKeySource::IdentityKey => false,
        StoreIndexKeySource::ResourceMember(member) => {
            member_paths_overlap(facts, member, &effect.members)
        }
    })
}

fn member_paths_overlap(
    facts: &CheckedFacts,
    indexed_member: ResourceMemberId,
    written_members: &[ResourceMemberId],
) -> bool {
    let indexed_path = member_path(facts, indexed_member);
    path_has_prefix(&indexed_path, written_members)
        || path_has_prefix(written_members, &indexed_path)
}

fn member_path(facts: &CheckedFacts, member_id: ResourceMemberId) -> Vec<ResourceMemberId> {
    let mut path = Vec::new();
    let mut current = Some(member_id);
    while let Some(id) = current {
        let Some(member) = facts.resource_members().get(id.0 as usize) else {
            break;
        };
        path.push(id);
        current = member.parent;
    }
    path.reverse();
    path
}

fn path_has_prefix(path: &[ResourceMemberId], prefix: &[ResourceMemberId]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(left, right)| left == right)
}

fn host_effect(expr: &CheckedExpr) -> Option<HostEffect> {
    let CheckedExpr::Call { target, .. } = expr else {
        return None;
    };
    match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Print) => Some(HostEffect::Output),
        CheckedCallTarget::Std(target) => target.requires_capability.map(HostEffect::Capability),
        _ => None,
    }
}
