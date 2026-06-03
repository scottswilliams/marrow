//! Streaming saved-layer iteration for `for` loops.

use std::ops::ControlFlow;

use marrow_check::CheckedExpr as ExecExpr;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, MaterializeKind, values_or_entries};
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
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
