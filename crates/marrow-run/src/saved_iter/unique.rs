use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::stdlib::{UniqueIndexLookup, read_unique_index_identity};
use crate::store::IndexAddress;
use crate::value::identity_value;

use super::{LoopShape, SavedLoopRow, SavedLoopSpec, shape_row};

pub(super) struct UniqueIndexScan {
    lookup: UniqueIndexLookup,
    shape: LoopShape,
    span: SourceSpan,
}

impl UniqueIndexScan {
    pub(super) fn new(
        place: &CheckedSavedPlace,
        address: IndexAddress,
        identity_arity: usize,
        index_name: String,
        remaining_key_depth: usize,
        spec: SavedLoopSpec<'_>,
    ) -> Self {
        Self {
            lookup: UniqueIndexLookup {
                address,
                identity_arity,
                index_name,
                place: place.clone(),
                remaining_key_depth,
            },
            shape: spec.shape,
            span: spec.span,
        }
    }

    pub(super) fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::index(self.lookup.address.clone())
    }

    pub(super) fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        if self.lookup.remaining_key_depth > 0 {
            return Err(unsupported(
                "iterating an incomplete unique index lookup",
                self.span,
            ));
        }
        let Some(identity) = read_unique_index_identity(&self.lookup, self.span, env)? else {
            return Ok(Flow::Normal);
        };
        match self.visit_identity(identity, env, visit)? {
            ControlFlow::Continue(()) => Ok(Flow::Normal),
            ControlFlow::Break(flow) => Ok(flow),
        }
    }

    fn visit_identity(
        &self,
        identity: Vec<SavedKey>,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        let key = identity_value(&self.lookup.place.root, identity.clone());
        let row = shape_row(self.shape, key, || match self.shape {
            LoopShape::Values => Err(unsupported("values over a unique index lookup", self.span)),
            _ => read_resource(&self.lookup.place, &identity, self.span, env),
        })?;
        visit(row, env)
    }
}
