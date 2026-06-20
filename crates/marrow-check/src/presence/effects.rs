use super::calls::wrapper_arg;
use super::keys::binding_key;
use super::scope::NameScope;
use super::target::{ReadPlace, ReadTarget, ReadTargetValue, read_target_with_scope};
use super::util::extend_unique;
use super::writes::expr_calls_saved_writer;
use crate::facts::{PresenceProofRead, SavedPlaceEffect};
use crate::{
    CheckedBinaryOp, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedForBinding,
    CheckedProgram, CheckedSavedPlace, CheckedSavedTerminal, CheckedUnaryOp, MarrowType,
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
    let path = LoopShape::classify(iterable, two_name_loop).key_path?;
    let mut target = read_target_with_scope(program, path, scope)
        .filter(|target| matches!(target.place, ReadPlace::Saved { .. }))
        .or_else(|| index_record_traversal_target(program, path))?;
    let key = binding_key(&binding.first, scope)?;
    target.keys.push(key.text);
    target.key_types.push(key.ty);
    extend_unique(&mut target.key_bindings, key.bindings);
    target.value = ReadTargetValue::Value;
    Some(target)
}

/// The materialized value type a `for` loop binds to each name. A value rides one
/// name per loop shape: `values(x)` and a bare single name over a value-yielding
/// collection bind the first; `entries(x)` and a bare two-name loop bind the
/// second; `keys(x)` and a bare single name over a keyed collection bind a key.
/// The presence walk binds these types so a sparse-field read of a loop-bound
/// entry classifies the same way the type pass's `collection_loop_binding_types`
/// does. A key binding carries a scalar or identity key, which has no sparse
/// fields, so only value-carrying names need a type here.
pub(super) fn loop_value_binding_type(
    program: &CheckedProgram,
    iterable: &CheckedExpr,
    binding: &CheckedForBinding,
    scope: &NameScope,
) -> Option<(Option<MarrowType>, Option<MarrowType>)> {
    let two_name_loop = binding.second.is_some();
    let value = LoopShape::classify(iterable, two_name_loop)
        .value_path
        .and_then(|path| iterable_value_type(program, path, scope));
    if two_name_loop {
        Some((None, value))
    } else {
        Some((value, None))
    }
}

/// How a `for` loop's iterable, after a `reversed(...)` wrapper is peeled,
/// distributes its streamed key and value across the loop's names. The narrowing
/// target needs `key_path` — the iterable whose streamed key the first name binds —
/// and the value-type binding needs `value_path` — the iterable whose per-element
/// value a name binds. Resolving both from one classification keeps the wrapper
/// rules (`keys`/`values`/`entries` and the bare single/two-name forms) in a single
/// place rather than two routers that must be hand-kept in sync.
struct LoopShape<'a> {
    key_path: Option<&'a CheckedExpr>,
    value_path: Option<&'a CheckedExpr>,
}

impl<'a> LoopShape<'a> {
    fn classify(iterable: &'a CheckedExpr, two_name_loop: bool) -> Self {
        if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Reversed) {
            return Self::classify(arg, two_name_loop);
        }
        if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Keys) {
            // `keys(x)` streams its argument's key to the single name and no value.
            return Self {
                key_path: (!two_name_loop).then_some(arg),
                value_path: None,
            };
        }
        if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Values) {
            // `values(x)` streams its argument's value to the single name and no key.
            return Self {
                key_path: None,
                value_path: (!two_name_loop).then_some(arg),
            };
        }
        if let Some(arg) = wrapper_arg(iterable, CheckedBuiltinCall::Entries) {
            // `entries(x)` streams its argument's key to the first name and value to
            // the second.
            return Self {
                key_path: two_name_loop.then_some(arg),
                value_path: two_name_loop.then_some(arg),
            };
        }
        // A bare loop streams the iterable's own key to the first name and, in the
        // two-name form, its value to the second.
        Self {
            key_path: Some(iterable),
            value_path: two_name_loop.then_some(iterable),
        }
    }
}

/// The per-element value type of a collection iterable, saved or local. A saved
/// record root or a non-unique index branch yields its resource; a saved layer
/// yields its leaf or group entry; a bound local sequence or keyed tree yields
/// its element.
fn iterable_value_type(
    program: &CheckedProgram,
    iterable: &CheckedExpr,
    scope: &NameScope,
) -> Option<MarrowType> {
    let resolver = crate::executable::SavedPlaceResolver::new(program);
    if let Some(place) = iterable.saved_place() {
        if place.layers.is_empty() && matches!(place.terminal, CheckedSavedTerminal::Record) {
            return Some(resolver.record_root_element_type(place));
        }
        if resolver.non_unique_index_branch_yields_identity(iterable) {
            return Some(resolver.record_root_element_type(place));
        }
        return resolver.value_type(iterable);
    }
    let CheckedExpr::Name { segments, .. } = iterable else {
        return None;
    };
    let [bound] = segments.as_slice() else {
        return None;
    };
    match scope.lookup_type(bound)? {
        MarrowType::Sequence(element) => Some(element.as_ref().clone()),
        MarrowType::LocalTree { value, .. } => Some(value.as_ref().clone()),
        _ => None,
    }
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
        read: PresenceProofRead::Direct,
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
        (ReadPlace::StoreIndex { .. }, ReadPlace::Saved { .. }) => false,
        (ReadPlace::TransformOld { .. }, _) | (_, ReadPlace::TransformOld { .. }) => false,
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
