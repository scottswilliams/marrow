use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{Located, RuntimeError, unsupported};
use crate::read::{
    INDEX_SCAN_PAGE_LIMIT, IndexBranchAddress, collected_identity_value, first_index_child,
    next_index_child,
};

use super::{ChildCursor, LoopShape, SavedLoopRow, SavedLoopSpec, shape_row, walk_keyed_children};

pub(super) struct IndexScan {
    place: CheckedSavedPlace,
    branch: IndexBranchAddress,
    yields_identity: bool,
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
            yields_identity: branch.arg_keys.len() >= branch.identity_start,
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
        let mut visit_keys =
            |keys: Vec<SavedKey>, env: &mut Env<'_>| self.visit_keys(keys, env, visit);
        if self.branch.depth == 0 {
            return stream_exact_index_tuple(&self.branch, self.span, env, &mut visit_keys);
        }
        let identity_prefix = self
            .branch
            .arg_keys
            .get(self.branch.identity_start..)
            .map_or_else(Vec::new, |keys| keys.to_vec());
        let cursor = IndexCursor::new(&self.branch.index.index, self.dir, self.span);
        walk_keyed_children(
            &cursor,
            self.branch.depth,
            &self.branch.arg_keys,
            &identity_prefix,
            env,
            &mut visit_keys,
        )
    }

    fn visit_keys(
        &self,
        keys: Vec<SavedKey>,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        let identity_root = self.yields_identity.then_some(self.place.root.as_str());
        let key = collected_identity_value(&keys, identity_root, self.span)?;
        let row = shape_row(self.shape, key, || {
            if self.yields_identity {
                read_resource(&self.place, &keys, self.span, env)
            } else {
                Err(unsupported(
                    "values/entries over this index branch",
                    self.span,
                ))
            }
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

fn stream_exact_index_tuple(
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let mut page = env
        .store
        .scan_index_tuple(&branch.index.index, &branch.arg_keys, INDEX_SCAN_PAGE_LIMIT)
        .map_err(|error| error.located(span))?;
    loop {
        for entry in page.entries {
            match visit(entry.identity, env)? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(flow) => return Ok(flow),
            }
        }
        let Some(cursor) = page.cursor else {
            break;
        };
        page = env
            .store
            .scan_index_tuple_after(
                &branch.index.index,
                &branch.arg_keys,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            )
            .map_err(|error| error.located(span))?;
    }
    Ok(Flow::Normal)
}
