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

use super::{LoopShape, SavedLoopRow, SavedLoopSpec};

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
        stream_index_identities(
            IndexWalk {
                index: &self.branch.index.index,
                dir: self.dir,
                span: self.span,
            },
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
        let key = collected_identity_value(&keys, self.span)?;
        match self.shape {
            LoopShape::Keys => visit(SavedLoopRow::Single(key), env),
            LoopShape::Values if self.yields_identity => {
                let value = read_resource(&self.place, &keys, self.span, env)?;
                visit(SavedLoopRow::Single(value), env)
            }
            LoopShape::Entries if self.yields_identity => {
                let value = read_resource(&self.place, &keys, self.span, env)?;
                visit(SavedLoopRow::Pair(key, value), env)
            }
            LoopShape::Values | LoopShape::Entries => Err(unsupported(
                "values/entries over this index branch",
                self.span,
            )),
        }
    }
}

#[derive(Clone, Copy)]
struct IndexWalk<'a> {
    index: &'a marrow_store::cell::CatalogId,
    dir: Direction,
    span: SourceSpan,
}

fn stream_index_identities(
    walk: IndexWalk<'_>,
    depth: usize,
    query_keys: &[SavedKey],
    identity_keys: &[SavedKey],
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let mut child = first_index_child(env.store, walk.index, query_keys, walk.dir, walk.span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut next_query_keys = query_keys.to_vec();
        next_query_keys.push(key.clone());
        let mut next_identity_keys = identity_keys.to_vec();
        next_identity_keys.push(key);
        if depth <= 1 {
            match visit(next_identity_keys, env)? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(flow) => return Ok(flow),
            }
        } else {
            match stream_index_identities(
                walk,
                depth - 1,
                &next_query_keys,
                &next_identity_keys,
                env,
                visit,
            )? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        child = next_index_child(
            env.store, walk.index, query_keys, &anchor, walk.dir, walk.span,
        )?;
    }
    Ok(Flow::Normal)
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
