use super::calls::wrapper_arg;
use super::keys::{assigned_bindings, binding_key};
use super::scope::NameScope;
use super::target::{ReadPlace, ReadTarget, read_target_with_scope};
use super::util::extend_unique;
use super::writes::expr_calls_saved_writer;
use crate::{
    CheckedArg, CheckedArgMode, CheckedBinaryOp, CheckedExpr, CheckedForBinding,
    CheckedInterpolationPart, CheckedProgram,
};

pub(super) fn condition_narrowings(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Vec<ReadTarget> {
    let mutations = mutating_bindings_in_expr(expr, scope);
    condition_effects_after_mutations(program, expr, scope, &mutations).narrowings
}

struct ConditionEffects {
    narrowings: Vec<ReadTarget>,
    writes_saved: bool,
}

fn condition_effects_after_mutations(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
    mutations: &[u32],
) -> ConditionEffects {
    match expr {
        CheckedExpr::Call { callee, args, .. } if super::calls::is_exists_call(callee) => {
            ConditionEffects {
                narrowings: args
                    .first()
                    .and_then(|arg| read_target_with_scope(program, &arg.value, scope))
                    .filter(|target| {
                        !target
                            .key_bindings
                            .iter()
                            .any(|binding| mutations.contains(binding))
                    })
                    .into_iter()
                    .collect(),
                writes_saved: expr_calls_saved_writer(program, expr, &mut Vec::new()),
            }
        }
        CheckedExpr::Binary {
            op: CheckedBinaryOp::And,
            left,
            right,
            ..
        } => {
            let left = condition_effects_after_mutations(program, left, scope, mutations);
            let right = condition_effects_after_mutations(program, right, scope, mutations);
            let mut narrowings = left.narrowings;
            if right.writes_saved {
                invalidate_saved_narrowings(&mut narrowings);
            }
            extend_unique(&mut narrowings, right.narrowings);
            ConditionEffects {
                narrowings,
                writes_saved: left.writes_saved || right.writes_saved,
            }
        }
        _ => ConditionEffects {
            narrowings: Vec::new(),
            writes_saved: expr_calls_saved_writer(program, expr, &mut Vec::new()),
        },
    }
}

pub(super) fn traversal_narrowing(
    program: &CheckedProgram,
    iterable: &CheckedExpr,
    binding: &CheckedForBinding,
    scope: &NameScope,
) -> Option<ReadTarget> {
    let two_name_loop = binding.second.is_some();
    if binding.second.as_deref() == Some(binding.first.as_str()) {
        return None;
    }
    let path = traversal_key_path(iterable, two_name_loop)?;
    let mut target = read_target_with_scope(program, path, scope)?;
    if !matches!(target.place, ReadPlace::Saved { .. }) {
        return None;
    }
    let key = binding_key(&binding.first, scope)?;
    target.keys.push(key.text);
    extend_unique(&mut target.key_bindings, key.bindings);
    Some(target)
}

fn traversal_key_path(expr: &CheckedExpr, two_name_loop: bool) -> Option<&CheckedExpr> {
    if let Some(arg) = wrapper_arg(expr, "reversed") {
        return traversal_key_path(arg, two_name_loop);
    }
    if wrapper_arg(expr, "values").is_some() {
        return None;
    }
    if let Some(arg) = wrapper_arg(expr, "entries") {
        return two_name_loop.then_some(arg);
    }
    if let Some(arg) = wrapper_arg(expr, "keys") {
        return (!two_name_loop).then_some(arg);
    }
    Some(expr)
}

pub(super) fn invalidate_key_bindings(narrowed: &mut Vec<ReadTarget>, bindings: Vec<u32>) {
    if bindings.is_empty() {
        return;
    }
    narrowed.retain(|target| {
        !target
            .key_bindings
            .iter()
            .any(|binding| bindings.contains(binding))
    });
}

pub(super) fn invalidate_removed_narrowings(
    narrowed: &mut Vec<ReadTarget>,
    before: &[ReadTarget],
    after: &[ReadTarget],
) {
    for target in before {
        if !after.contains(target) {
            narrowed.retain(|current| current != target);
        }
    }
}

pub(super) fn invalidate_written_target(narrowed: &mut Vec<ReadTarget>, written: &ReadTarget) {
    narrowed.retain(|target| !written_target_invalidates(written, target));
}

pub(super) fn invalidate_saved_narrowings(narrowed: &mut Vec<ReadTarget>) {
    narrowed.retain(|target| {
        !matches!(
            target.place,
            ReadPlace::Saved { .. } | ReadPlace::StoreIndex { .. }
        )
    });
}

fn written_target_invalidates(written: &ReadTarget, target: &ReadTarget) -> bool {
    match (&written.place, &target.place) {
        (
            ReadPlace::Saved {
                root: written_root,
                members: written_members,
            },
            ReadPlace::Saved {
                root: target_root,
                members: target_members,
            },
        ) => {
            written_root == target_root
                && related_prefix(&written.keys, &target.keys)
                && related_prefix(written_members, target_members)
        }
        (
            ReadPlace::Saved {
                root: written_root, ..
            },
            ReadPlace::StoreIndex {
                root: target_root, ..
            },
        ) => written_root == target_root,
        (
            ReadPlace::StoreIndex {
                root: written_root,
                index: written_index,
            },
            ReadPlace::StoreIndex {
                root: target_root,
                index: target_index,
            },
        ) => {
            written_root == target_root
                && written_index == target_index
                && written.keys == target.keys
        }
        (ReadPlace::StoreIndex { .. }, ReadPlace::Saved { .. }) => false,
    }
}

fn slice_prefix<T: PartialEq>(prefix: &[T], full: &[T]) -> bool {
    prefix.len() <= full.len() && prefix.iter().zip(full).all(|(left, right)| left == right)
}

fn related_prefix<T: PartialEq>(left: &[T], right: &[T]) -> bool {
    slice_prefix(left, right) || slice_prefix(right, left)
}

pub(super) fn mutating_arg_bindings(args: &[CheckedArg], scope: &NameScope) -> Vec<u32> {
    let mut bindings = Vec::new();
    for arg in args {
        if matches!(arg.mode, Some(CheckedArgMode::Out | CheckedArgMode::InOut)) {
            extend_unique(&mut bindings, assigned_bindings(&arg.value, scope));
        }
    }
    bindings
}

fn mutating_bindings_in_expr(expr: &CheckedExpr, scope: &NameScope) -> Vec<u32> {
    match expr {
        CheckedExpr::Call { callee, args, .. } => {
            let mut bindings = mutating_arg_bindings(args, scope);
            extend_unique(&mut bindings, mutating_bindings_in_expr(callee, scope));
            for arg in args {
                extend_unique(&mut bindings, mutating_bindings_in_expr(&arg.value, scope));
            }
            bindings
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            mutating_bindings_in_expr(base, scope)
        }
        CheckedExpr::Unary { operand, .. } => mutating_bindings_in_expr(operand, scope),
        CheckedExpr::Binary { left, right, .. } => {
            let mut bindings = mutating_bindings_in_expr(left, scope);
            extend_unique(&mut bindings, mutating_bindings_in_expr(right, scope));
            bindings
        }
        CheckedExpr::Interpolation { parts, .. } => {
            let mut bindings = Vec::new();
            for part in parts {
                if let CheckedInterpolationPart::Expr(expr) = part {
                    extend_unique(&mut bindings, mutating_bindings_in_expr(expr, scope));
                }
            }
            bindings
        }
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {
            Vec::new()
        }
    }
}
