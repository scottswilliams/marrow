use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::RuntimeError;
use crate::read::{collected_identity_value, first_record_child, next_record_child};

use super::{ChildCursor, LoopShape, SavedLoopRow, SavedLoopSpec, shape_row, walk_keyed_children};

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
        let cursor = RecordCursor {
            store: &self.store,
            dir: self.dir,
            span: self.span,
        };
        let mut visit_identity =
            |identity: Vec<SavedKey>, env: &mut Env<'_>| self.visit_identity(identity, env, visit);
        walk_keyed_children(&cursor, self.arity, &[], &[], env, &mut visit_identity)
    }

    fn visit_identity(
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

struct RecordCursor<'a> {
    store: &'a marrow_store::cell::CatalogId,
    dir: Direction,
    span: SourceSpan,
}

impl ChildCursor for RecordCursor<'_> {
    fn first(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, RuntimeError> {
        first_record_child(env.store, self.store, prefix, self.dir, self.span)
    }

    fn next(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
        anchor: &SavedKey,
    ) -> Result<Option<SavedKey>, RuntimeError> {
        next_record_child(env.store, self.store, prefix, anchor, self.dir, self.span)
    }
}
