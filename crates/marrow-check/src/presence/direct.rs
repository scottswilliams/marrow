use std::collections::HashMap;

use marrow_schema::stdlib::Capability;
use marrow_syntax::{Block, Expression, InterpolationPart, Statement};

use super::calls::append_call_args;
use super::keys::saved_path_parts;
use super::scope::NameScope;
use super::target::saved_place;
use crate::expand_alias;
use crate::facts::{CheckedFacts, DirectEffectFacts, HostEffect};

pub(crate) fn direct_effects_for_block(
    facts: &CheckedFacts,
    aliases: &HashMap<String, Vec<String>>,
    block: &Block,
) -> DirectEffectFacts {
    let mut effects = DirectEffectFacts::default();
    collect_block_effects(facts, aliases, block, &mut effects);
    effects
}

fn collect_block_effects(
    facts: &CheckedFacts,
    aliases: &HashMap<String, Vec<String>>,
    block: &Block,
    effects: &mut DirectEffectFacts,
) {
    for statement in &block.statements {
        collect_statement_effects(facts, aliases, statement, effects);
    }
}

fn collect_statement_effects(
    facts: &CheckedFacts,
    aliases: &HashMap<String, Vec<String>>,
    statement: &Statement,
    effects: &mut DirectEffectFacts,
) {
    match statement {
        Statement::Const { value, .. } | Statement::Throw { value, .. } => {
            if matches!(statement, Statement::Throw { .. }) {
                effects.throws = true;
            }
            collect_expr_reads(facts, aliases, value, effects);
        }
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                collect_expr_reads(facts, aliases, value, effects);
            }
        }
        Statement::Assign { target, value, .. } => {
            collect_saved_write(facts, target, effects);
            collect_saved_path_key_reads(facts, aliases, target, effects);
            collect_expr_reads(facts, aliases, value, effects);
        }
        Statement::Delete { path, .. } => {
            collect_saved_write(facts, path, effects);
            collect_saved_path_key_reads(facts, aliases, path, effects);
        }
        Statement::Merge { target, value, .. } => {
            collect_expr_reads(facts, aliases, target, effects);
            collect_expr_reads(facts, aliases, value, effects);
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                collect_expr_reads(facts, aliases, value, effects);
            }
        }
        Statement::Expr { value, .. } => collect_expr_reads(facts, aliases, value, effects),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                collect_expr_reads(facts, aliases, condition, effects);
            }
            collect_block_effects(facts, aliases, then_block, effects);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    collect_expr_reads(facts, aliases, condition, effects);
                }
                collect_block_effects(facts, aliases, &else_if.block, effects);
            }
            if let Some(block) = else_block {
                collect_block_effects(facts, aliases, block, effects);
            }
        }
        Statement::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_reads(facts, aliases, condition, effects);
            }
            collect_block_effects(facts, aliases, body, effects);
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => {
            collect_expr_reads(facts, aliases, iterable, effects);
            if let Some(step) = step {
                collect_expr_reads(facts, aliases, step, effects);
            }
            collect_block_effects(facts, aliases, body, effects);
        }
        Statement::Transaction { body, .. } => {
            effects.transactions = true;
            collect_block_effects(facts, aliases, body, effects);
        }
        Statement::Lock { path, body, .. } => {
            if let Some(path) = path {
                collect_expr_reads(facts, aliases, path, effects);
            }
            collect_block_effects(facts, aliases, body, effects);
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            collect_block_effects(facts, aliases, body, effects);
            if let Some(catch) = catch {
                collect_block_effects(facts, aliases, &catch.block, effects);
            }
            if let Some(finally) = finally {
                collect_block_effects(facts, aliases, finally, effects);
            }
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                collect_expr_reads(facts, aliases, scrutinee, effects);
            }
            for arm in arms {
                collect_block_effects(facts, aliases, &arm.block, effects);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } => {}
    }
}

fn collect_expr_reads(
    facts: &CheckedFacts,
    aliases: &HashMap<String, Vec<String>>,
    expr: &Expression,
    effects: &mut DirectEffectFacts,
) {
    let scope = NameScope::default();
    if let Some(path) = saved_path_parts(expr, &scope) {
        if let Some(effect) = saved_place(facts, &path.root, &path.members) {
            push_unique(&mut effects.saved_reads, effect);
        }
        collect_saved_path_key_reads(facts, aliases, expr, effects);
        return;
    }
    if let Some(effect) = host_effect(expr, aliases) {
        push_unique(&mut effects.host_calls, effect);
    }
    match expr {
        Expression::Call { callee, args, .. } => {
            if let Some((target, rest)) = append_call_args(callee, args) {
                collect_saved_write(facts, &target.value, effects);
                collect_saved_path_key_reads(facts, aliases, &target.value, effects);
                for arg in rest {
                    collect_expr_reads(facts, aliases, &arg.value, effects);
                }
                return;
            }
            collect_expr_reads(facts, aliases, callee, effects);
            for arg in args {
                collect_expr_reads(facts, aliases, &arg.value, effects);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            collect_expr_reads(facts, aliases, base, effects);
        }
        Expression::Unary { operand, .. } => collect_expr_reads(facts, aliases, operand, effects),
        Expression::Binary { left, right, .. } => {
            collect_expr_reads(facts, aliases, left, effects);
            collect_expr_reads(facts, aliases, right, effects);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    collect_expr_reads(facts, aliases, expr, effects);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn collect_saved_path_key_reads(
    facts: &CheckedFacts,
    aliases: &HashMap<String, Vec<String>>,
    expr: &Expression,
    effects: &mut DirectEffectFacts,
) {
    match expr {
        Expression::Call { callee, args, .. } => {
            collect_saved_path_key_reads(facts, aliases, callee, effects);
            for arg in args {
                collect_expr_reads(facts, aliases, &arg.value, effects);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            collect_saved_path_key_reads(facts, aliases, base, effects);
        }
        Expression::SavedRoot { .. }
        | Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::Unary { .. }
        | Expression::Binary { .. }
        | Expression::Interpolation { .. } => {}
    }
}

fn collect_saved_write(facts: &CheckedFacts, expr: &Expression, effects: &mut DirectEffectFacts) {
    let scope = NameScope::default();
    if let Some(path) = saved_path_parts(expr, &scope)
        && let Some(effect) = saved_place(facts, &path.root, &path.members)
    {
        push_unique(&mut effects.saved_writes, effect);
    }
}

fn host_effect(expr: &Expression, aliases: &HashMap<String, Vec<String>>) -> Option<HostEffect> {
    match expr {
        Expression::Call { callee, .. } => match callee.as_ref() {
            Expression::Name { segments, .. } => {
                let expanded = expand_alias(segments, aliases);
                match expanded.as_slice() {
                    [name] if name == "print" || name == "write" => Some(HostEffect::Output),
                    [std, module, op] if std == "std" => marrow_schema::stdlib::lookup(module, op)
                        .and_then(|entry| match entry.capability {
                            Capability::Pure => None,
                            capability => Some(HostEffect::Capability(capability)),
                        }),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}

fn push_unique<T>(items: &mut Vec<T>, item: T)
where
    T: PartialEq,
{
    if !items.contains(&item) {
        items.push(item);
    }
}
