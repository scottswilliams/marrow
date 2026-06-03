use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::RuntimeError;
use crate::read::{collected_identity_value, first_record_child, next_record_child};

use super::{LoopShape, SavedLoopRow, SavedLoopSpec};

pub(super) struct RootScan {
    place: CheckedSavedPlace,
    store: marrow_store::cell::CatalogId,
    arity: usize,
    dir: Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl RootScan {
    pub(super) fn new(
        place: &CheckedSavedPlace,
        spec: SavedLoopSpec<'_>,
    ) -> Result<Self, RuntimeError> {
        let arity = place.identity_keys.len();
        if arity == 0 {
            return Err(crate::error::type_error(
                &format!(
                    "`^{}` is a singleton with no identities to iterate",
                    place.root
                ),
                spec.span,
            ));
        }
        Ok(Self {
            place: place.clone(),
            store: crate::store::catalog_id(&place.store_catalog_id, "store", spec.span)?,
            arity,
            dir: spec.dir,
            shape: spec.shape,
            span: spec.span,
        })
    }

    pub(super) fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::Record {
            store: self.store.clone(),
        }
    }

    pub(super) fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        let mut visit_identity =
            |identity: Vec<SavedKey>, env: &mut Env<'_>| self.visit_identity(identity, env, visit);
        stream_record_identities(
            &self.store,
            self.arity,
            &[],
            self.dir,
            self.span,
            env,
            &mut visit_identity,
        )
    }

    fn visit_identity(
        &self,
        identity: Vec<SavedKey>,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        let key = collected_identity_value(&identity, self.span)?;
        match self.shape {
            LoopShape::Keys => visit(SavedLoopRow::Single(key), env),
            LoopShape::Values => {
                let value = read_resource(&self.place, &identity, self.span, env)?;
                visit(SavedLoopRow::Single(value), env)
            }
            LoopShape::Entries => {
                let value = read_resource(&self.place, &identity, self.span, env)?;
                visit(SavedLoopRow::Pair(key, value), env)
            }
        }
    }
}

fn stream_record_identities(
    store: &marrow_store::cell::CatalogId,
    depth: usize,
    keys: &[SavedKey],
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let mut child = first_record_child(env.store, store, keys, dir, span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut next_keys = keys.to_vec();
        next_keys.push(key);
        if depth <= 1 {
            match visit(next_keys.clone(), env)? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(flow) => return Ok(flow),
            }
        } else {
            match stream_record_identities(store, depth - 1, &next_keys, dir, span, env, visit)? {
                Flow::Normal => {}
                flow => return Ok(flow),
            }
        }
        child = next_record_child(env.store, store, keys, &anchor, dir, span)?;
    }
    Ok(Flow::Normal)
}
