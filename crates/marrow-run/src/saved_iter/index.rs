use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::RuntimeError;
use crate::read::{
    IndexBranchAddress, collected_identity_value, first_index_child, next_index_child,
    stream_index_branch,
};

use super::{ChildCursor, LoopShape, SavedLoopRow, SavedLoopSpec, shape_row};

pub(super) struct IndexScan {
    place: CheckedSavedPlace,
    branch: IndexBranchAddress,
    dir: Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl IndexScan {
    pub(super) fn new(
        place: &CheckedSavedPlace,
        branch: IndexBranchAddress,
        spec: SavedLoopSpec<'_>,
    ) -> Self {
        Self {
            place: place.clone(),
            branch,
            dir: spec.dir,
            shape: spec.shape,
            span: spec.span,
        }
    }

    pub(super) fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::index(self.branch.index.clone())
    }

    pub(super) fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        stream_index_branch(
            &self.place,
            &self.branch,
            self.dir,
            self.span,
            env,
            &mut |identity: Vec<SavedKey>, env: &mut Env<'_>| {
                self.yield_identity(identity, env, visit)
            },
        )
    }

    fn yield_identity(
        &self,
        identity: Vec<SavedKey>,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        let key = collected_identity_value(&identity, Some(&self.place.root), self.span)?;
        let row = shape_row(self.shape, key, || {
            read_resource(&self.place, &identity, self.span, env)
        })?;
        visit(row, env)
    }
}

pub(crate) struct IndexCursor<'a> {
    index: &'a marrow_store::cell::CatalogId,
    dir: Direction,
    span: SourceSpan,
}

impl<'a> IndexCursor<'a> {
    pub(crate) fn new(
        index: &'a marrow_store::cell::CatalogId,
        dir: Direction,
        span: SourceSpan,
    ) -> Self {
        Self { index, dir, span }
    }
}

impl ChildCursor for IndexCursor<'_> {
    fn first(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, RuntimeError> {
        first_index_child(env.store, self.index, prefix, self.dir, self.span)
    }

    fn next(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
        anchor: &SavedKey,
    ) -> Result<Option<SavedKey>, RuntimeError> {
        next_index_child(env.store, self.index, prefix, anchor, self.dir, self.span)
    }
}
