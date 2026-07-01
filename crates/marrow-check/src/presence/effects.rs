use super::calls::wrapper_arg;
use super::keys::binding_key;
use super::scope::NameScope;
use super::target::{ReadPlace, ReadTarget, ReadTargetValue, read_target_with_scope};
use super::util::extend_unique;
use super::writes::expr_calls_saved_writer;
use crate::facts::{ReadKind, SavedPlaceEffect};
use crate::{
    CheckedBinaryOp, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedForBinding,
    CheckedProgram, CheckedSavedPlace, CheckedSavedTerminal, CheckedUnaryOp,
};

pub(super) fn condition_narrowings(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Vec<ReadTarget> {
    condition_effects(program, expr, scope).narrowings
}

pub(super) fn negated_exists_narrowings(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Vec<ReadTarget> {
    let CheckedExpr::Unary {
        op: CheckedUnaryOp::Not,
        operand,
        ..
    } = expr
    else {
        return Vec::new();
    };
    if expr_calls_saved_writer(program, operand) {
        return Vec::new();
    }
    exists_target(program, operand, scope).into_iter().collect()
}

struct ConditionEffects {
    narrowings: Vec<ReadTarget>,
    writes_saved: bool,
}

fn condition_effects(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> ConditionEffects {
    match expr {
        CheckedExpr::Call { target, .. }
            if *target == CheckedCallTarget::Builtin(CheckedBuiltinCall::Exists) =>
        {
            let writes_saved = expr_calls_saved_writer(program, expr);
            ConditionEffects {
                narrowings: if writes_saved {
                    Vec::new()
                } else {
                    exists_target(program, expr, scope).into_iter().collect()
                },
                writes_saved,
            }
        }
        CheckedExpr::Binary {
            op: CheckedBinaryOp::And,
            left,
            right,
            ..
        } => {
            let left = condition_effects(program, left, scope);
            let right = condition_effects(program, right, scope);
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
            writes_saved: expr_calls_saved_writer(program, expr),
        },
    }
}

fn exists_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    let CheckedExpr::Call { target, args, .. } = expr else {
        return None;
    };
    if *target != CheckedCallTarget::Builtin(CheckedBuiltinCall::Exists) {
        return None;
    }
    args.first()
        .and_then(|arg| read_target_with_scope(program, &arg.value, scope))
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
    let path = loop_key_path(iterable, two_name_loop)?;
    let mut target = record_root_traversal_target(program, path, scope)
        .or_else(|| index_record_traversal_target(program, path))?;
    let key = binding_key(&binding.first, scope)?;
    target.keys.push(key.text);
    target.key_types.push(key.ty);
    extend_unique(&mut target.key_bindings, key.bindings);
    target.value = ReadTargetValue::Value;
    Some(target)
}

/// The narrowing a loop over a whole-record store root proves: streaming a store
/// yields present record identities, so a re-read of the record at the loop key is
/// proven present. A keyed child layer or positional sequence streams keys, not
/// records, and the positional/keyed read it indexes is uniformly `T?` under the one
/// rule, so it is never narrowed here.
fn record_root_traversal_target(
    program: &CheckedProgram,
    path: &CheckedExpr,
    scope: &NameScope,
) -> Option<ReadTarget> {
    let place = path.saved_place()?;
    if !place.layers.is_empty() || !matches!(place.terminal, CheckedSavedTerminal::Record) {
        return None;
    }
    read_target_with_scope(program, path, scope)
        .filter(|target| matches!(target.place, ReadPlace::Saved { .. }))
}

/// The iterable whose streamed key a `for` loop's first name binds, after a
/// `reversed(...)` wrapper is peeled: `keys(x)` and a bare single name stream a key,
/// `entries(x)` and a bare two-name loop stream the first name a key, and `values(x)`
/// streams no key. The first name binds an entry key exactly when this is `Some`, so
/// only then does iterating the collection prove that entry's read present.
fn loop_key_path(iterable: &CheckedExpr, two_name_loop: bool) -> Option<&CheckedExpr> {
    if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Reversed) {
        return loop_key_path(arg, two_name_loop);
    }
    if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Keys) {
        return (!two_name_loop).then_some(arg);
    }
    if wrapper_arg(iterable, CheckedBuiltinCall::Values).is_some() {
        return None;
    }
    if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Entries) {
        return two_name_loop.then_some(arg);
    }
    Some(iterable)
}

fn index_record_traversal_target(
    program: &CheckedProgram,
    path: &CheckedExpr,
) -> Option<ReadTarget> {
    let place = path.saved_place()?;
    if !index_traversal_yields_identity(place) {
        return None;
    }
    let store = program.facts.store_by_root(&place.root)?;
    Some(ReadTarget {
        place: ReadPlace::Saved {
            root: place.root.clone(),
            members: Vec::new(),
            effect: SavedPlaceEffect {
                resource: store.resource,
                members: Vec::new(),
            },
        },
        keys: Vec::new(),
        key_types: Vec::new(),
        key_bindings: Vec::new(),
        read: ReadKind::Direct,
        value: ReadTargetValue::Value,
    })
}

pub(super) fn index_traversal_yields_identity(place: &CheckedSavedPlace) -> bool {
    let CheckedSavedTerminal::Index {
        name, args, unique, ..
    } = &place.terminal
    else {
        return false;
    };
    if place.identity_keys.is_empty() {
        return false;
    }
    let Some(index) = place
        .indexes
        .iter()
        .find(|index| index.name == name.as_str())
    else {
        return false;
    };
    if *unique {
        return args.len() == index.keys.len();
    }
    // A non-unique index branch always streams the store identity, for any
    // partial prefix down to the full one.
    true
}

pub(super) fn invalidate_saved_narrowings(narrowed: &mut Vec<ReadTarget>) {
    let invalidated = saved_targets(narrowed);
    narrowed.retain(|target| !invalidated.contains(target));
}

pub(super) fn targets_invalidated_by_key_bindings(
    targets: &[ReadTarget],
    bindings: &[u32],
) -> Vec<ReadTarget> {
    if bindings.is_empty() {
        return Vec::new();
    }
    targets
        .iter()
        .filter(|target| {
            target
                .key_bindings
                .iter()
                .any(|binding| bindings.contains(binding))
        })
        .cloned()
        .collect()
}

pub(super) fn targets_invalidated_by_written_target(
    targets: &[ReadTarget],
    written: &ReadTarget,
) -> Vec<ReadTarget> {
    targets
        .iter()
        .filter(|target| written_target_invalidates(written, target))
        .cloned()
        .collect()
}

pub(super) fn saved_targets(targets: &[ReadTarget]) -> Vec<ReadTarget> {
    targets
        .iter()
        .filter(|target| {
            matches!(
                target.place,
                ReadPlace::Saved { .. } | ReadPlace::StoreIndex { .. }
            )
        })
        .cloned()
        .collect()
}

fn written_target_invalidates(written: &ReadTarget, target: &ReadTarget) -> bool {
    match (&written.place, &target.place) {
        (
            ReadPlace::Saved {
                root: written_root,
                members: written_members,
                ..
            },
            ReadPlace::Saved {
                root: target_root,
                members: target_members,
                ..
            },
        ) => {
            written_root == target_root
                && saved_keys_may_overlap(written_root, written, target)
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
            ReadPlace::StoreIndex { id: written_id, .. },
            ReadPlace::StoreIndex { id: target_id, .. },
        ) => written_id == target_id && written.keys == target.keys,
        (
            ReadPlace::TransformOld {
                resource: written_resource,
                member: written_member,
            },
            ReadPlace::TransformOld {
                resource: target_resource,
                member: target_member,
            },
        ) => written_resource == target_resource && written_member == target_member,
        (ReadPlace::Local { binding: written }, ReadPlace::Local { binding: target }) => {
            written == target
        }
        (ReadPlace::StoreIndex { .. }, ReadPlace::Saved { .. }) => false,
        (ReadPlace::TransformOld { .. }, _) | (_, ReadPlace::TransformOld { .. }) => false,
        (ReadPlace::Local { .. }, _) | (_, ReadPlace::Local { .. }) => false,
    }
}

fn saved_keys_may_overlap(root: &str, written: &ReadTarget, target: &ReadTarget) -> bool {
    if identity_splice_key(root, written) || identity_splice_key(root, target) {
        return true;
    }
    written
        .keys
        .iter()
        .zip(&target.keys)
        .enumerate()
        .all(|(index, (written_key, target_key))| {
            saved_key_may_alias(
                written_key,
                written.key_types.get(index),
                target_key,
                target.key_types.get(index),
            )
        })
}

fn saved_key_may_alias(
    written_key: &str,
    written_type: Option<&Option<crate::MarrowType>>,
    target_key: &str,
    target_type: Option<&Option<crate::MarrowType>>,
) -> bool {
    if written_key == target_key {
        return true;
    }
    !distinct_identity_key_types(written_type, target_type)
}

fn distinct_identity_key_types(
    left: Option<&Option<crate::MarrowType>>,
    right: Option<&Option<crate::MarrowType>>,
) -> bool {
    let Some(left) = identity_type_root(left) else {
        return false;
    };
    let Some(right) = identity_type_root(right) else {
        return false;
    };
    left != right
}

fn identity_type_root(ty: Option<&Option<crate::MarrowType>>) -> Option<&str> {
    match ty {
        Some(Some(crate::MarrowType::Identity(root))) => Some(root.as_str()),
        _ => None,
    }
}

fn identity_splice_key(root: &str, target: &ReadTarget) -> bool {
    target.keys.len() == 1
        && matches!(
            target.key_types.first(),
            Some(Some(crate::MarrowType::Identity(identity_root))) if identity_root == root
        )
}

fn slice_prefix<T: PartialEq>(prefix: &[T], full: &[T]) -> bool {
    prefix.len() <= full.len() && prefix.iter().zip(full).all(|(left, right)| left == right)
}

fn related_prefix<T: PartialEq>(left: &[T], right: &[T]) -> bool {
    slice_prefix(left, right) || slice_prefix(right, left)
}
