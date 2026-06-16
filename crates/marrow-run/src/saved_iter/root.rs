use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::SavedKey;
use marrow_store::tree::IndexRangeBounds;
use marrow_store::value::{ScalarType, scalar_key_matches_type, validate_scalar_key};
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{Located, RuntimeError, type_error};
use crate::read::{
    KeyRangeAddress, RecordChildRange, collected_identity_value, first_record_child,
    first_record_child_in_range, next_record_child, next_record_child_in_range,
};
use crate::value::saved_key_to_value;

use super::{ChildCursor, LoopShape, SavedLoopRow, SavedLoopSpec, shape_row, walk_keyed_children};

pub(super) struct RootScan {
    place: CheckedSavedPlace,
    store: marrow_store::cell::CatalogId,
    key_scalars: Vec<Option<ScalarType>>,
    exact_prefix: Vec<SavedKey>,
    range: Option<IndexRangeBounds>,
    dir: Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl RootScan {
    pub(super) fn new(
        place: &CheckedSavedPlace,
        address: KeyRangeAddress,
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
            key_scalars: place.identity_keys.iter().map(|key| key.scalar).collect(),
            exact_prefix: address.exact_prefix,
            range: address.range,
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
        let depth = self
            .key_scalars
            .len()
            .saturating_sub(self.exact_prefix.len());
        let cursor = RecordCursor::new_bounded(
            &self.store,
            self.key_scalars.clone(),
            self.dir,
            self.span,
            self.range.clone(),
        );
        let mut visit_identity =
            |identity: Vec<SavedKey>, env: &mut Env<'_>| self.visit_identity(identity, env, visit);
        walk_keyed_children(
            &cursor,
            depth,
            &self.exact_prefix,
            &self.exact_prefix,
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
        let key = if self.range.is_some() {
            let component = identity
                .last()
                .cloned()
                .expect("walk_keyed_children yields a non-empty identity");
            saved_key_to_value(component, self.span)?
        } else {
            collected_identity_value(&identity, Some(&self.place.root), self.span)?
        };
        let row = shape_row(self.shape, key, || {
            read_resource(&self.place, &identity, self.span, env)
        })?;
        visit(row, env)
    }
}

pub(crate) struct RecordCursor<'a> {
    store: &'a marrow_store::cell::CatalogId,
    key_scalars: Vec<Option<ScalarType>>,
    dir: Direction,
    span: SourceSpan,
    range: Option<IndexRangeBounds>,
}

impl<'a> RecordCursor<'a> {
    pub(crate) fn new(
        store: &'a marrow_store::cell::CatalogId,
        key_scalars: Vec<Option<ScalarType>>,
        dir: Direction,
        span: SourceSpan,
    ) -> Self {
        Self {
            store,
            key_scalars,
            dir,
            span,
            range: None,
        }
    }

    pub(crate) fn new_bounded(
        store: &'a marrow_store::cell::CatalogId,
        key_scalars: Vec<Option<ScalarType>>,
        dir: Direction,
        span: SourceSpan,
        range: Option<IndexRangeBounds>,
    ) -> Self {
        Self {
            store,
            key_scalars,
            dir,
            span,
            range,
        }
    }

    fn arity(&self) -> usize {
        self.key_scalars.len()
    }

    fn validate_child(&self, prefix: &[SavedKey], key: &SavedKey) -> Result<(), RuntimeError> {
        validate_scalar_key(key).map_err(|error| error.located(self.span))?;
        if let Some(Some(expected)) = self.key_scalars.get(prefix.len())
            && !scalar_key_matches_type(key, *expected)
        {
            return Err(type_error(
                "stored identity keys do not match the store identity type",
                self.span,
            ));
        }
        Ok(())
    }
}

impl ChildCursor for RecordCursor<'_> {
    fn first(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, RuntimeError> {
        if let Some(range) = &self.range {
            return first_record_child_in_range(
                env.store,
                RecordChildRange {
                    store_id: self.store,
                    prefix,
                    range,
                    dir: self.dir,
                    arity: self.arity(),
                    span: self.span,
                },
            )
            .and_then(|key| {
                if let Some(key) = &key {
                    self.validate_child(prefix, key)?;
                }
                Ok(key)
            });
        }
        let key = first_record_child(
            env.store,
            self.store,
            prefix,
            self.dir,
            self.arity(),
            self.span,
        )?;
        if let Some(key) = &key {
            self.validate_child(prefix, key)?;
        }
        Ok(key)
    }

    fn next(
        &self,
        env: &mut Env<'_>,
        prefix: &[SavedKey],
        anchor: &SavedKey,
    ) -> Result<Option<SavedKey>, RuntimeError> {
        if let Some(range) = &self.range {
            return next_record_child_in_range(
                env.store,
                RecordChildRange {
                    store_id: self.store,
                    prefix,
                    range,
                    dir: self.dir,
                    arity: self.arity(),
                    span: self.span,
                },
                anchor,
            )
            .and_then(|key| {
                if let Some(key) = &key {
                    self.validate_child(prefix, key)?;
                }
                Ok(key)
            });
        }
        let key = next_record_child(
            env.store,
            self.store,
            prefix,
            anchor,
            self.dir,
            self.arity(),
            self.span,
        )?;
        if let Some(key) = &key {
            self.validate_child(prefix, key)?;
        }
        Ok(key)
    }
}
