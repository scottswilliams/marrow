//! Local-collection runtime: read, write, delete, count, and ordered
//! key/value/entry materialization over the in-memory `Sequence`/`LocalTree`
//! kernel, mirroring the saved iteration contract.
//!
//! This module is the one boundary that turns evaluated lookup arguments into a
//! typed address: a runtime int becomes a validated [`Position`] and a key tuple a
//! [`CollectionKey`]. A position below the 1-based range addresses no node and never
//! becomes a `Position`, so the by-convention guards that once threaded raw integers
//! are now the newtype construction itself.

use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use super::{Direction, absent_read};
use crate::env::Env;
use crate::error::{RUN_ABSENT, RuntimeError, assign_error, overflow, type_error, unsupported};
use crate::expr::eval_expr;
use crate::value::{CollectionKey, Position, Value, saved_key_to_value, value_to_key};

pub(crate) fn eval_local_collection_read(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // A position below the 1-based sequence range addresses no node, so a read
    // resolves it to the empty optional. A write of that same position still faults
    // (`resolve_local_collection_target` surfaces it there), so an unreachable node
    // is never persisted.
    let target = match resolve_local_collection_target(name, args, span, env) {
        Ok(Some(target)) => target,
        Ok(None) => return Ok(None),
        Err(error) if error.code() == RUN_ABSENT && error.is_catchable() => {
            return Ok(Some(Value::Absent));
        }
        Err(error) => return Err(error),
    };
    target.read(env).map(Some)
}

pub(crate) fn eval_local_collection_write(
    name: &str,
    args: &[ExecArg],
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    let Some(target) = resolve_local_collection_target(name, args, span, env)? else {
        return Ok(false);
    };
    let value = eval_expr(value, env)?;
    target.write(value, env)?;
    Ok(true)
}

pub(crate) fn eval_local_collection_write_value(
    name: &str,
    args: &[ExecArg],
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    let Some(target) = resolve_local_collection_target(name, args, span, env)? else {
        return Ok(false);
    };
    target.write(value, env)?;
    Ok(true)
}

pub(crate) enum LocalCollectionTarget {
    Sequence {
        name: String,
        position: Position,
        span: SourceSpan,
    },
    Tree {
        name: String,
        key: CollectionKey,
        span: SourceSpan,
    },
}

pub(crate) fn resolve_local_collection_target(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<LocalCollectionTarget>, RuntimeError> {
    match env.lookup(name) {
        Some(Value::Sequence(_)) => Ok(Some(LocalCollectionTarget::Sequence {
            name: name.to_string(),
            position: eval_local_sequence_position(args, span, env)?,
            span,
        })),
        Some(Value::LocalTree(_)) => {
            let key = eval_local_keys(args, span, env)?;
            reject_non_positive_sequence_key(&key, span)?;
            Ok(Some(LocalCollectionTarget::Tree {
                name: name.to_string(),
                key,
                span,
            }))
        }
        _ => Ok(None),
    }
}

impl LocalCollectionTarget {
    pub(crate) fn read(&self, env: &Env<'_>) -> Result<Value, RuntimeError> {
        match self {
            Self::Sequence {
                name,
                position,
                span,
            } => {
                let Some(Value::Sequence(items)) = env.lookup(name) else {
                    return Err(unsupported("reading this local collection", *span));
                };
                Ok(items.get(*position).cloned().unwrap_or(Value::Absent))
            }
            Self::Tree { name, key, span } => {
                let Some(Value::LocalTree(tree)) = env.lookup(name) else {
                    return Err(unsupported("reading this local collection", *span));
                };
                Ok(tree.get(key).cloned().unwrap_or(Value::Absent))
            }
        }
    }

    fn span(&self) -> SourceSpan {
        match self {
            Self::Sequence { span, .. } | Self::Tree { span, .. } => *span,
        }
    }

    pub(crate) fn write(self, value: Value, env: &mut Env<'_>) -> Result<(), RuntimeError> {
        // A local sequence or tree element is managed through the collection
        // builtins, so it has no point-clear: assigning the empty optional is the
        // one rule, resolved before the write or expressed with `delete`.
        if matches!(value, Value::Absent) {
            return Err(type_error(
                "a local collection element has no point-clear; assign a value or use `delete`",
                self.span(),
            ));
        }
        match self {
            Self::Sequence {
                name,
                position,
                span,
            } => {
                let Value::Sequence(items) = mutable_local_collection(&name, span, env)? else {
                    return Err(unsupported("writing this local collection", span));
                };
                items.set(position, value);
            }
            Self::Tree { name, key, span } => {
                let Value::LocalTree(tree) = mutable_local_collection(&name, span, env)? else {
                    return Err(unsupported("writing this local collection", span));
                };
                tree.insert(key, value);
            }
        }
        Ok(())
    }
}

/// A mutable borrow of `name`'s collection binding, surfacing an unbound or immutable
/// binding as the same assignment fault a whole-value reassignment would. Mutating in
/// place is what keeps a local-collection write proportional to one node rather than
/// cloning the whole collection.
fn mutable_local_collection<'v>(
    name: &str,
    span: SourceSpan,
    env: &'v mut Env<'_>,
) -> Result<&'v mut Value, RuntimeError> {
    env.lookup_mut(name)
        .map_err(|error| assign_error(name, error, span))
}

/// The catchable absent fault a write or read to a non-positive sequence position
/// raises, naming the position rather than the collection. A local sequence is
/// identical to a saved sequence, so this matches the saved guard's message.
fn non_positive_sequence_position(span: SourceSpan) -> RuntimeError {
    absent_read("a sequence position below 1 is absent".into(), span)
}

/// Reject a write to a single int-keyed tree at a position below 1. A single
/// int-keyed layer is a 1-based sequence, so a zero or negative position addresses no
/// node and must persist nothing; the lone int is validated through [`Position`], the
/// single owner of the 1-based rule. A composite or non-int key carries meaning in its
/// own right and is left alone.
fn reject_non_positive_sequence_key(
    key: &CollectionKey,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if let Some(position) = key.lone_int()
        && Position::new(position).is_none()
    {
        return Err(non_positive_sequence_position(span));
    }
    Ok(())
}

/// Delete an entry from a local collection by position or key. A delete names a node
/// to remove, so a hole, an out-of-range position, or a non-positive position
/// addresses no node and is a tolerant no-op, the same as deleting any absent saved
/// position. A sequence delete leaves a hole; append never reuses it.
pub(crate) fn eval_local_collection_delete(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    match env.lookup(name) {
        Some(Value::Sequence(_)) => {
            let position = match eval_local_sequence_position(args, span, env) {
                Ok(position) => Some(position),
                // A non-positive position addresses no node, so its delete is a no-op.
                Err(error) if error.code() == RUN_ABSENT && error.is_catchable() => None,
                Err(error) => return Err(error),
            };
            if let (Some(position), Value::Sequence(items)) =
                (position, mutable_local_collection(name, span, env)?)
            {
                items.remove(position);
            }
            Ok(())
        }
        Some(Value::LocalTree(_)) => {
            let key = eval_local_keys(args, span, env)?;
            if let Value::LocalTree(tree) = mutable_local_collection(name, span, env)? {
                tree.remove(&key);
            }
            Ok(())
        }
        _ => Err(unsupported("deleting from this local value", span)),
    }
}

/// Count a local collection by borrowing it: reading the cardinality needs only
/// `len`, so a bound collection is never deep-cloned just to be measured. A bare
/// name is resolved against the environment in place; any other expression is
/// evaluated to a temporary value, which carries its own ownership.
pub(crate) fn local_collection_count(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if let Some(name) = local_collection_name(arg)
        && let Some(value @ (Value::Sequence(_) | Value::LocalTree(_))) = env.lookup(name)
    {
        return local_collection_len(value, span);
    }
    local_collection_len(&eval_expr(arg, env)?, span)
}

fn local_collection_len(value: &Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let len = match value {
        Value::Sequence(items) => items.len(),
        Value::LocalTree(tree) => tree.len(),
        _ => return Err(unsupported("counting this value", span)),
    };
    i64::try_from(len)
        .map(Value::Int)
        .map_err(|_| overflow(span))
}

/// The single unqualified binding name an expression names directly, if any. A
/// borrow of that binding reads a local collection without cloning it; anything
/// qualified or computed is not a plain binding reference.
fn local_collection_name(arg: &ExecExpr) -> Option<&str> {
    match arg {
        ExecExpr::Name {
            segments,
            enum_member: None,
            ..
        } => match segments.as_slice() {
            [name] => Some(name),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn enumerate_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<Value>, RuntimeError> {
    let mut keys: Vec<Value> = match value {
        Value::Sequence(items) => items.positions().map(Value::Int).collect(),
        Value::LocalTree(tree) => {
            // Rows already iterate in key-tuple order, so the distinct first-column keys
            // come out ascending; only collapse the runs a multi-column tree repeats.
            let mut seen = Vec::<marrow_store::key::SavedKey>::new();
            for (keys, _) in tree.rows() {
                let Some(key) = keys.first() else {
                    continue;
                };
                if seen.last() != Some(key) {
                    seen.push(key.clone());
                }
            }
            seen.into_iter()
                .map(|key| saved_key_to_value(key, span))
                .collect::<Result<_, _>>()?
        }
        _ => return Err(unsupported("keys over this value", span)),
    };
    apply_direction(&mut keys, dir);
    Ok(keys)
}

/// Enumerate the keys of a local collection in ascending key order, peeling any
/// `reversed(...)` wrappers by parity so a nested reversal composes correctly. A
/// saved place is not a local value and yields `None` for the caller to handle.
pub(crate) fn enumerate_local_keys_call_arg(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    enumerate_local_keys_in_dir(arg, Direction::Ascending, span, env)
}

/// Enumerate the keys of a local collection in descending key order. `None` when the
/// argument is a saved path, which is iterated in place rather than materialized.
pub(crate) fn enumerate_reversed_local_keys_call_arg(
    arg: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    enumerate_local_keys_in_dir(arg, Direction::Descending, span, env)
}

fn enumerate_local_keys_in_dir(
    arg: &ExecExpr,
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    if arg.saved_place().is_some() {
        return Ok(None);
    }
    enumerate_local_collection_dir(eval_expr(arg, env)?, dir, span).map(Some)
}

pub(crate) fn materialize_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    let mut rows = match value {
        Value::Sequence(items) => items
            .rows()
            .map(|(position, value)| (Value::Int(position), value.clone()))
            .collect(),
        Value::LocalTree(tree) => tree
            .into_rows()
            .map(|(keys, value)| {
                let key = keys.into_iter().next().ok_or_else(|| {
                    unsupported("entries over a local tree with no key column", span)
                })?;
                Ok((saved_key_to_value(key, span)?, value))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?,
        _ => return Err(unsupported("values/entries over this value", span)),
    };
    apply_direction(&mut rows, dir);
    Ok(rows)
}

/// Reverse the rows for a descending walk. The whole row reverses as one, so keyed
/// entries stay paired with their values.
fn apply_direction<T>(rows: &mut [T], dir: Direction) {
    if dir == Direction::Descending {
        rows.reverse();
    }
}

/// The 1-based [`Position`] a local-sequence lookup addresses. A non-int key is a type
/// fault; a zero or negative position addresses no node, so it never becomes a
/// `Position` and raises the catchable absent fault instead — a guarded read resolves
/// it through `??`/`if const`/`exists`/`catch`, and a write aborts before mutating the
/// binding, keeping the spec's "resolved at the read site" promise for every int
/// position and matching the saved side's non-positive rule.
fn eval_local_sequence_position(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Position, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("a local sequence lookup takes one key", span));
    };
    reject_named_lookup_arg(arg, span)?;
    let Value::Int(position) = eval_expr(&arg.value, env)? else {
        return Err(type_error("a local sequence key must be an int", span));
    };
    Position::new(position).ok_or_else(|| non_positive_sequence_position(span))
}

fn eval_local_keys(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<CollectionKey, RuntimeError> {
    let keys = args
        .iter()
        .map(|arg| {
            reject_named_lookup_arg(arg, span)?;
            value_to_key(eval_expr(&arg.value, env)?, span)?
                .ok_or_else(|| unsupported("a key of this type", span))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CollectionKey::new(keys))
}

/// A local-collection lookup takes only positional value keys.
fn reject_named_lookup_arg(arg: &ExecArg, span: SourceSpan) -> Result<(), RuntimeError> {
    if arg.name.is_some() {
        return Err(unsupported(
            "named arguments in a local collection lookup",
            span,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod local_kernel_laws {
    //! The local-collection kernel obeys the tree laws that apply to an in-memory,
    //! `Value`-holding ordered map: 1-based position rejection, ascending key-ordered
    //! iteration, gap-skipping, hole-preserving delete, and strictly-ascending append.
    //!
    //! The store's byte-substrate 22-law conformance suite (`marrow_store::conformance`)
    //! does not apply here. That suite exercises a `Backend` of byte keys and byte
    //! values under write transactions and read snapshots; a local collection has
    //! neither a byte encoding — its leaves are arbitrary runtime `Value`s, including
    //! nested collections, resources, and identities that no store cell can hold — nor
    //! transactional visibility. These are the same tree laws expressed at the `Value`
    //! level, the only level at which a local collection has a contract.

    use crate::value::{CollectionKey, LocalTree, Position, Sequence, Value};
    use marrow_store::key::SavedKey;

    fn pos(n: i64) -> Position {
        Position::new(n).expect("test position is 1-based")
    }

    #[test]
    fn a_position_below_one_addresses_no_node() {
        assert_eq!(Position::new(0), None);
        assert_eq!(Position::new(-1), None);
        assert!(Position::new(1).is_some());
        assert!(Position::new(i64::MAX).is_some());
    }

    #[test]
    fn a_sequence_iterates_ascending_and_skips_holes() {
        let mut seq = Sequence::default();
        seq.set(pos(3), Value::Int(30));
        seq.set(pos(1), Value::Int(10));
        // position 2 is left as a hole
        assert_eq!(seq.positions().collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(seq.get(pos(2)), None);
        assert_eq!(seq.get(pos(3)), Some(&Value::Int(30)));
    }

    #[test]
    fn a_delete_leaves_a_hole_that_append_never_fills() {
        let mut seq = Sequence::default();
        seq.set(pos(1), Value::Int(10));
        seq.set(pos(2), Value::Int(20));
        seq.set(pos(3), Value::Int(30));
        assert!(seq.remove(pos(2)));
        assert_eq!(seq.get(pos(2)), None);
        let appended = seq.append(Value::Int(40)).expect("append succeeds");
        assert_eq!(appended, pos(4));
        assert_eq!(seq.positions().collect::<Vec<_>>(), vec![1, 3, 4]);
    }

    #[test]
    fn append_is_strictly_ascending_past_the_highest_position() {
        let mut seq = Sequence::default();
        assert_eq!(seq.append(Value::Int(1)).expect("append"), pos(1));
        assert_eq!(seq.append(Value::Int(2)).expect("append"), pos(2));
        seq.set(pos(10), Value::Int(100));
        assert_eq!(seq.append(Value::Int(3)).expect("append"), pos(11));
    }

    #[test]
    fn a_tree_orders_rows_by_full_key_tuple() {
        let mut tree = LocalTree::default();
        let key =
            |a: i64, b: &str| CollectionKey::new(vec![SavedKey::Int(a), SavedKey::Str(b.into())]);
        tree.insert(key(2, "a"), Value::Int(1));
        tree.insert(key(1, "b"), Value::Int(2));
        tree.insert(key(1, "a"), Value::Int(3));
        let order: Vec<Vec<SavedKey>> = tree.rows().map(|(keys, _)| keys.to_vec()).collect();
        assert_eq!(
            order,
            vec![
                vec![SavedKey::Int(1), SavedKey::Str("a".into())],
                vec![SavedKey::Int(1), SavedKey::Str("b".into())],
                vec![SavedKey::Int(2), SavedKey::Str("a".into())],
            ]
        );
    }

    #[test]
    fn a_tree_row_address_round_trips() {
        let mut tree = LocalTree::default();
        let key = CollectionKey::new(vec![SavedKey::Str("k".into())]);
        tree.insert(key.clone(), Value::Int(7));
        assert_eq!(tree.get(&key), Some(&Value::Int(7)));
        tree.remove(&key);
        assert_eq!(tree.get(&key), None);
    }

    #[test]
    fn a_lone_int_key_is_recognized_as_a_sequence_position() {
        assert_eq!(
            CollectionKey::new(vec![SavedKey::Int(5)]).lone_int(),
            Some(5)
        );
        assert_eq!(
            CollectionKey::new(vec![SavedKey::Str("x".into())]).lone_int(),
            None
        );
        assert_eq!(
            CollectionKey::new(vec![SavedKey::Int(1), SavedKey::Int(2)]).lone_int(),
            None
        );
    }
}
