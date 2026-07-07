//! Streaming saved-layer iteration for `for` loops.

use std::ops::ControlFlow;

use marrow_check::CheckedExpr as ExecExpr;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, overflow, unsupported};
use crate::read::{IterableLayer, iterable_layer};
use crate::stdlib::unique_index_lookup;
use crate::value::Value;

mod child_layer;
mod index;
mod root;
mod unique;

use child_layer::ChildLayerScan;
use index::IndexScan;
use root::RootScan;
use unique::UniqueIndexScan;

pub(crate) use index::IndexCursor;
pub(crate) use root::RecordCursor;

/// A streamed loop row: a key-first single binding yields the streamed key; an
/// (n+1)-name head yields every bound key column outermost-first plus the leaf
/// value.
pub(crate) enum SavedLoopRow {
    Key(Value),
    Full(Vec<Value>, Value),
}

/// The single owner of the row contract. `read_value` is consulted only when the
/// head binds a value, so a key-only head pays nothing to read the leaf.
pub(super) fn saved_loop_row(
    with_value: bool,
    columns: Vec<Value>,
    read_value: impl FnOnce() -> Result<Value, RuntimeError>,
) -> Result<SavedLoopRow, RuntimeError> {
    if with_value {
        Ok(SavedLoopRow::Full(columns, read_value()?))
    } else {
        let mut columns = columns;
        Ok(SavedLoopRow::Key(
            columns.pop().expect("a key-only row binds one column"),
        ))
    }
}

/// One keyed level of a saved-tree child walk. Record and index iteration share
/// this first/next contract; only the cell kind they address differs.
pub(crate) trait ChildCursor {
    fn first(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, RuntimeError>;

    fn next(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
        anchor: &SavedKey,
    ) -> Result<Option<SavedKey>, RuntimeError>;
}

/// Walk `depth` keyed levels under `query_prefix`, yielding each leaf's accumulated
/// identity to `visit`. The cursor prefix and the visited identity are threaded
/// separately so an index walk can seek over its full argument-plus-identity prefix
/// while yielding only the identity suffix; a record walk passes the same slice for
/// both. A `Break` from `visit` stops the whole walk.
pub(crate) fn walk_keyed_children(
    cursor: &dyn ChildCursor,
    depth: usize,
    query_prefix: &[SavedKey],
    identity_prefix: &[SavedKey],
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let mut first = |env: &mut Env<'_>, prefix: &[SavedKey]| cursor.first(env, prefix);
    let mut next = |env: &mut Env<'_>, prefix: &[SavedKey], anchor: &SavedKey| {
        cursor.next(env, prefix, anchor)
    };
    match walk_keyed_children_after(
        env,
        KeyedChildrenWalk {
            depth,
            query_prefix,
            identity_prefix,
            after_identity: None,
        },
        &mut first,
        &mut next,
        visit,
    )? {
        ControlFlow::Continue(()) => Ok(Flow::Normal),
        ControlFlow::Break(flow) => Ok(flow),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KeyedChildrenWalk<'a> {
    pub(crate) depth: usize,
    pub(crate) query_prefix: &'a [SavedKey],
    pub(crate) identity_prefix: &'a [SavedKey],
    pub(crate) after_identity: Option<&'a [SavedKey]>,
}

/// Walk keyed children in forward order, optionally resuming strictly after a
/// previously yielded identity. This is the shared depth-bounded traversal core
/// for language loops, counts, and transport-neutral surface pages.
pub(crate) fn walk_keyed_children_after<C, E, B>(
    context: &mut C,
    walk: KeyedChildrenWalk<'_>,
    first: &mut impl FnMut(&mut C, &[SavedKey]) -> Result<Option<SavedKey>, E>,
    next: &mut impl FnMut(&mut C, &[SavedKey], &SavedKey) -> Result<Option<SavedKey>, E>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut C) -> Result<ControlFlow<B>, E>,
) -> Result<ControlFlow<B>, E> {
    let mut child = first_walk_child(
        context,
        walk.query_prefix,
        walk.identity_prefix,
        walk.after_identity,
        first,
    )?;
    let mut child_after = walk
        .after_identity
        .filter(|after| after.starts_with(walk.identity_prefix));
    while let Some(key) = child {
        let anchor = key.clone();
        let mut next_query = walk.query_prefix.to_vec();
        next_query.push(key.clone());
        let mut next_identity = walk.identity_prefix.to_vec();
        next_identity.push(key);
        if walk.depth <= 1 {
            // SavedKey ordering mirrors the store's order-preserving key encoding,
            // so this typed comparison is the same boundary as the physical scan.
            if walk
                .after_identity
                .is_none_or(|after| next_identity.as_slice() > after)
            {
                match visit(next_identity, context)? {
                    ControlFlow::Continue(()) => {}
                    ControlFlow::Break(value) => return Ok(ControlFlow::Break(value)),
                }
            }
        } else {
            match walk_keyed_children_after(
                context,
                KeyedChildrenWalk {
                    depth: walk.depth - 1,
                    query_prefix: &next_query,
                    identity_prefix: &next_identity,
                    after_identity: child_after,
                },
                first,
                next,
                visit,
            )? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(value) => return Ok(ControlFlow::Break(value)),
            }
        }
        child_after = None;
        child = next(context, walk.query_prefix, &anchor)?;
    }
    Ok(ControlFlow::Continue(()))
}

fn first_walk_child<C, E>(
    context: &mut C,
    query_prefix: &[SavedKey],
    identity_prefix: &[SavedKey],
    after_identity: Option<&[SavedKey]>,
    first: &mut impl FnMut(&mut C, &[SavedKey]) -> Result<Option<SavedKey>, E>,
) -> Result<Option<SavedKey>, E> {
    if let Some(after) = after_identity
        && after.starts_with(identity_prefix)
        && let Some(anchor) = after.get(identity_prefix.len()).cloned()
    {
        return Ok(Some(anchor));
    }
    first(context, query_prefix)
}

/// Sum the records reachable by walking `depth` keyed levels under `query_prefix`,
/// guarding for overflow. `leaf_count` lets a record walk fold a bulk child count into
/// its final level while an index walk counts one per leaf. Counting reuses the
/// iteration walk so there is one tree-walk owner.
pub(crate) fn count_keyed_children(
    cursor: &dyn ChildCursor,
    depth: usize,
    query_prefix: &[SavedKey],
    env: &mut Env<'_>,
    span: SourceSpan,
    leaf_count: impl Fn(&[SavedKey], &mut Env<'_>) -> Result<usize, RuntimeError>,
) -> Result<usize, RuntimeError> {
    let mut count = 0usize;
    let flow = walk_keyed_children(cursor, depth, query_prefix, &[], env, &mut |keys, env| {
        let leaf = leaf_count(&keys, env)?;
        count = count.checked_add(leaf).ok_or_else(|| overflow(span))?;
        Ok(ControlFlow::Continue(()))
    })?;
    debug_assert!(matches!(flow, Flow::Normal));
    Ok(count)
}

pub(crate) struct SavedLoopSpec<'a> {
    pub(super) layer: &'a ExecExpr,
    pub(super) dir: Direction,
    /// The number of key columns the head binds and the scan streams. A store root
    /// or index branch collapses to one identity column; a composite child layer
    /// streams this many columns.
    pub(super) key_columns: usize,
    /// Whether the head binds the leaf value at the fully-keyed position.
    pub(super) with_value: bool,
    pub(super) span: SourceSpan,
}

impl<'a> SavedLoopSpec<'a> {
    /// The loop spec for a saved-path head. The iterable is the bare saved path;
    /// traversal direction is the head `reversed` keyword, not a wrapper, so there
    /// is nothing to peel. `key_columns` is the count of bound key columns and
    /// `with_value` whether the head also binds the leaf.
    pub(crate) fn from_head(
        iterable: &'a ExecExpr,
        key_columns: usize,
        with_value: bool,
        dir: Direction,
    ) -> Option<Self> {
        iterable.saved_place().is_some().then_some(Self {
            layer: iterable,
            dir,
            key_columns,
            with_value,
            span: iterable.span(),
        })
    }

    pub(crate) fn run(
        self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        let plan = SavedLoopPlan::new(self, env)?;
        plan.run(env, visit)
    }
}

enum SavedLoopPlan {
    Root(RootScan),
    Index(IndexScan),
    UniqueIndex(UniqueIndexScan),
    ChildLayer(Box<ChildLayerScan>),
}

impl SavedLoopPlan {
    fn new(spec: SavedLoopSpec<'_>, env: &mut Env<'_>) -> Result<Self, RuntimeError> {
        if let Some(lookup) = unique_index_lookup(spec.layer, env)? {
            let Some(place) = spec.layer.saved_place() else {
                return Err(unsupported("iterating this saved path", spec.layer.span()));
            };
            return Ok(Self::UniqueIndex(UniqueIndexScan::new(
                place,
                lookup.address,
                lookup.identity_arity,
                lookup.index_name,
                lookup.remaining_key_depth,
                spec,
            )));
        }
        match iterable_layer(spec.layer, env)? {
            IterableLayer::Root(place, address) => {
                Ok(Self::Root(RootScan::new(place, address, spec)?))
            }
            IterableLayer::Index(place, branch) => {
                Ok(Self::Index(IndexScan::new(place, branch, spec)))
            }
            IterableLayer::ChildLayer => {
                Ok(Self::ChildLayer(Box::new(ChildLayerScan::new(spec, env)?)))
            }
        }
    }

    fn run(
        self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        let layer = self.traversed_layer();
        env.traversed_layers.push(layer);
        let result = self.stream(env, visit);
        env.traversed_layers.pop();
        result
    }

    fn traversed_layer(&self) -> TraversedLayer {
        match self {
            Self::Root(scan) => scan.traversed_layer(),
            Self::Index(scan) => scan.traversed_layer(),
            Self::UniqueIndex(scan) => scan.traversed_layer(),
            Self::ChildLayer(scan) => scan.traversed_layer(),
        }
    }

    fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        match self {
            Self::Root(scan) => scan.stream(env, visit),
            Self::Index(scan) => scan.stream(env, visit),
            Self::UniqueIndex(scan) => scan.stream(env, visit),
            Self::ChildLayer(scan) => scan.stream(env, visit),
        }
    }
}
