use marrow_syntax::{Argument, Block, Expression, ForBinding, InterpolationPart, Statement};

use super::calls::{callee_name, is_append_call, wrapper_arg};
use super::keys::{assigned_bindings, binding_key};
use super::scope::NameScope;
use super::target::{ReadPlace, ReadTarget, read_target_with_scope};
use super::util::extend_unique;
use crate::CheckedProgram;

pub(super) fn condition_narrowings(
    program: &CheckedProgram,
    expr: &Expression,
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
    expr: &Expression,
    scope: &NameScope,
    mutations: &[u32],
) -> ConditionEffects {
    match expr {
        Expression::Call { callee, args, .. } if super::calls::is_exists_call(callee) => {
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
        Expression::Binary {
            op: marrow_syntax::BinaryOp::And,
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
    iterable: &Expression,
    binding: &ForBinding,
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

fn traversal_key_path(expr: &Expression, two_name_loop: bool) -> Option<&Expression> {
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

pub(super) fn mutating_arg_bindings(args: &[Argument], scope: &NameScope) -> Vec<u32> {
    let mut bindings = Vec::new();
    for arg in args {
        if matches!(
            arg.mode,
            Some(marrow_syntax::ArgMode::Out | marrow_syntax::ArgMode::InOut)
        ) {
            extend_unique(&mut bindings, assigned_bindings(&arg.value, scope));
        }
    }
    bindings
}

fn mutating_bindings_in_expr(expr: &Expression, scope: &NameScope) -> Vec<u32> {
    match expr {
        Expression::Call { callee, args, .. } => {
            let mut bindings = mutating_arg_bindings(args, scope);
            extend_unique(&mut bindings, mutating_bindings_in_expr(callee, scope));
            for arg in args {
                extend_unique(&mut bindings, mutating_bindings_in_expr(&arg.value, scope));
            }
            bindings
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            mutating_bindings_in_expr(base, scope)
        }
        Expression::Unary { operand, .. } => mutating_bindings_in_expr(operand, scope),
        Expression::Binary { left, right, .. } => {
            let mut bindings = mutating_bindings_in_expr(left, scope);
            extend_unique(&mut bindings, mutating_bindings_in_expr(right, scope));
            bindings
        }
        Expression::Interpolation { parts, .. } => {
            let mut bindings = Vec::new();
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    extend_unique(&mut bindings, mutating_bindings_in_expr(expr, scope));
                }
            }
            bindings
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {
            Vec::new()
        }
    }
}

pub(super) fn call_writes_saved_data(program: &CheckedProgram, callee: &Expression) -> bool {
    let Expression::Name { segments, .. } = callee else {
        return false;
    };
    let Some(leaf) = segments.last() else {
        return false;
    };
    function_name_writes_saved_data(program, leaf, &mut Vec::new())
}

fn function_name_writes_saved_data(
    program: &CheckedProgram,
    name: &str,
    visiting: &mut Vec<String>,
) -> bool {
    if visiting.iter().any(|item| item == name) {
        return false;
    }
    visiting.push(name.to_string());
    let writes =
        program.facts.functions().iter().any(|function| {
            function.name == name && !function.direct_effects.saved_writes.is_empty()
        }) || program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .filter(|function| function.name == name)
            .any(|function| block_calls_saved_writer(program, &function.body, visiting));
    visiting.pop();
    writes
}

fn block_calls_saved_writer(
    program: &CheckedProgram,
    block: &Block,
    visiting: &mut Vec<String>,
) -> bool {
    block
        .statements
        .iter()
        .any(|statement| statement_calls_saved_writer(program, statement, visiting))
}

fn statement_calls_saved_writer(
    program: &CheckedProgram,
    statement: &Statement,
    visiting: &mut Vec<String>,
) -> bool {
    match statement {
        Statement::Const { value, .. }
        | Statement::Var {
            value: Some(value), ..
        }
        | Statement::Throw { value, .. }
        | Statement::Expr { value, .. } => expr_calls_saved_writer(program, value, visiting),
        Statement::Var { value: None, .. } => false,
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            expr_calls_saved_writer(program, target, visiting)
                || expr_calls_saved_writer(program, value, visiting)
        }
        Statement::Delete { path, .. } => expr_calls_saved_writer(program, path, visiting),
        Statement::Return { value, .. } => value
            .as_ref()
            .is_some_and(|value| expr_calls_saved_writer(program, value, visiting)),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => statement_if_calls_saved_writer(
            program, condition, then_block, else_ifs, else_block, visiting,
        ),
        Statement::While {
            condition, body, ..
        } => {
            condition
                .as_ref()
                .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
                || block_calls_saved_writer(program, body, visiting)
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => statement_for_calls_saved_writer(program, iterable, step.as_ref(), body, visiting),
        Statement::Transaction { body, .. } | Statement::Lock { body, .. } => {
            block_calls_saved_writer(program, body, visiting)
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => statement_try_calls_saved_writer(
            program,
            body,
            catch.as_ref(),
            finally.as_ref(),
            visiting,
        ),
        Statement::Match {
            scrutinee, arms, ..
        } => statement_match_calls_saved_writer(program, scrutinee.as_ref(), arms, visiting),
        Statement::Break { .. } | Statement::Continue { .. } => false,
    }
}

fn statement_if_calls_saved_writer(
    program: &CheckedProgram,
    condition: &Option<Expression>,
    then_block: &Block,
    else_ifs: &[marrow_syntax::ElseIf],
    else_block: &Option<Block>,
    visiting: &mut Vec<String>,
) -> bool {
    condition
        .as_ref()
        .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
        || block_calls_saved_writer(program, then_block, visiting)
        || else_ifs.iter().any(|else_if| {
            else_if
                .condition
                .as_ref()
                .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
                || block_calls_saved_writer(program, &else_if.block, visiting)
        })
        || else_block
            .as_ref()
            .is_some_and(|block| block_calls_saved_writer(program, block, visiting))
}

fn statement_for_calls_saved_writer(
    program: &CheckedProgram,
    iterable: &Expression,
    step: Option<&Expression>,
    body: &Block,
    visiting: &mut Vec<String>,
) -> bool {
    expr_calls_saved_writer(program, iterable, visiting)
        || step
            .as_ref()
            .is_some_and(|step| expr_calls_saved_writer(program, step, visiting))
        || block_calls_saved_writer(program, body, visiting)
}

fn statement_try_calls_saved_writer(
    program: &CheckedProgram,
    body: &Block,
    catch: Option<&marrow_syntax::CatchClause>,
    finally: Option<&Block>,
    visiting: &mut Vec<String>,
) -> bool {
    block_calls_saved_writer(program, body, visiting)
        || catch
            .as_ref()
            .is_some_and(|catch| block_calls_saved_writer(program, &catch.block, visiting))
        || finally
            .as_ref()
            .is_some_and(|finally| block_calls_saved_writer(program, finally, visiting))
}

fn statement_match_calls_saved_writer(
    program: &CheckedProgram,
    scrutinee: Option<&Expression>,
    arms: &[marrow_syntax::MatchArm],
    visiting: &mut Vec<String>,
) -> bool {
    scrutinee
        .as_ref()
        .is_some_and(|scrutinee| expr_calls_saved_writer(program, scrutinee, visiting))
        || arms
            .iter()
            .any(|arm| block_calls_saved_writer(program, &arm.block, visiting))
}

fn expr_calls_saved_writer(
    program: &CheckedProgram,
    expr: &Expression,
    visiting: &mut Vec<String>,
) -> bool {
    match expr {
        Expression::Call { callee, args, .. } => {
            is_append_call(callee)
                || callee_name(callee)
                    .is_some_and(|name| function_name_writes_saved_data(program, name, visiting))
                || expr_calls_saved_writer(program, callee, visiting)
                || args
                    .iter()
                    .any(|arg| expr_calls_saved_writer(program, &arg.value, visiting))
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            expr_calls_saved_writer(program, base, visiting)
        }
        Expression::Unary { operand, .. } => expr_calls_saved_writer(program, operand, visiting),
        Expression::Binary { left, right, .. } => {
            expr_calls_saved_writer(program, left, visiting)
                || expr_calls_saved_writer(program, right, visiting)
        }
        Expression::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            InterpolationPart::Text { .. } => false,
            InterpolationPart::Expr(expr) => expr_calls_saved_writer(program, expr, visiting),
        }),
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {
            false
        }
    }
}
