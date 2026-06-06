//! Streaming saved-layer iteration for `for` loops.

use std::ops::ControlFlow;

use marrow_check::CheckedExpr as ExecExpr;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, MaterializeKind, values_or_entries};
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, overflow, unsupported};
use crate::read::{IterableLayer, iterable_layer, keys_argument, reversed_argument};
use crate::stdlib::{check_key_collection, unique_index_lookup};
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

#[derive(Clone, Copy)]
pub(super) enum LoopShape {
    Keys,
    Values,
    Entries,
}

pub(crate) enum SavedLoopRow {
    Single(Value),
    Pair(Value, Value),
}

/// Build the loop row a scan yields for one identity under `shape`: the key alone for
/// `Keys`, the value alone for `Values`, the key/value pair for `Entries`. `read_value`
/// is consulted only when the shape needs the value, so a scan whose values are
/// unsupported (or gated) reports that through its reader and pays nothing for `Keys`.
/// Keeping this dispatch here is the single owner of the Keys/Values/Entries row contract.
pub(super) fn shape_row(
    shape: LoopShape,
    key: Value,
    read_value: impl FnOnce() -> Result<Value, RuntimeError>,
) -> Result<SavedLoopRow, RuntimeError> {
    match shape {
        LoopShape::Keys => Ok(SavedLoopRow::Single(key)),
        LoopShape::Values => Ok(SavedLoopRow::Single(read_value()?)),
        LoopShape::Entries => Ok(SavedLoopRow::Pair(key, read_value()?)),
    }
}

/// One level of a saved-tree child walk: the first child under a key prefix and the next
/// child after an anchor, in the scan's direction. Record iteration and index iteration
/// both seek over keyed tree levels with this same first/next contract; only the cell kind
/// they address (records vs index entries) differs.
pub(super) trait ChildCursor {
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
/// identity to `visit`. The cursor prefix (`query_prefix`) and the visited identity
/// (`identity_prefix`) are threaded separately so an index walk can seek over its full
/// argument-plus-identity prefix while yielding only the identity suffix; a record walk
/// passes the same slice for both. A `Break` from `visit` stops the whole walk.
pub(super) fn walk_keyed_children(
    cursor: &dyn ChildCursor,
    depth: usize,
    query_prefix: &[SavedKey],
    identity_prefix: &[SavedKey],
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let mut child = cursor.first(env, query_prefix)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut next_query = query_prefix.to_vec();
        next_query.push(key.clone());
        let mut next_identity = identity_prefix.to_vec();
        next_identity.push(key);
        if depth <= 1 {
            match visit(next_identity, env)? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(flow) => return Ok(flow),
            }
        } else {
            match walk_keyed_children(cursor, depth - 1, &next_query, &next_identity, env, visit)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        child = cursor.next(env, query_prefix, &anchor)?;
    }
    Ok(Flow::Normal)
}

/// Sum, with overflow guarding, the records reachable by walking `depth` keyed levels
/// under `query_prefix`. `leaf_count` reports how many records each walked leaf prefix
/// contributes, letting a record walk fold a bulk child count into its final level while
/// an index walk counts one per leaf. The same depth-bounded walk that drives iteration
/// drives counting, so there is one tree-walk owner per concept.
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
    pub(super) shape: LoopShape,
    from_keys_builtin: bool,
    pub(super) span: SourceSpan,
}

impl<'a> SavedLoopSpec<'a> {
    pub(crate) fn from_iterable(iterable: &'a ExecExpr, two_name: bool) -> Option<Self> {
        let (iterable, dir) = match reversed_argument(iterable) {
            Some(inner) => (inner, Direction::Descending),
            None => (iterable, Direction::Ascending),
        };
        if let Some(layer) = keys_argument(iterable) {
            return (!two_name && layer.saved_place().is_some()).then_some(Self {
                layer,
                dir,
                shape: LoopShape::Keys,
                from_keys_builtin: true,
                span: iterable.span(),
            });
        }
        if let Some(inner) = values_or_entries(iterable) {
            let shape = match inner.kind {
                MaterializeKind::Values => {
                    if two_name {
                        return None;
                    }
                    LoopShape::Values
                }
                MaterializeKind::Entries => LoopShape::Entries,
            };
            return inner.layer.saved_place().is_some().then_some(Self {
                layer: inner.layer,
                dir,
                shape,
                from_keys_builtin: false,
                span: iterable.span(),
            });
        }
        iterable.saved_place().is_some().then_some(Self {
            layer: iterable,
            dir,
            shape: if two_name {
                LoopShape::Entries
            } else {
                LoopShape::Keys
            },
            from_keys_builtin: false,
            span: iterable.span(),
        })
    }

    pub(crate) fn run(
        self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        if self.from_keys_builtin {
            check_key_collection(self.layer, self.span)?;
        }
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
            IterableLayer::Root(place) => Ok(Self::Root(RootScan::new(place, spec)?)),
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
