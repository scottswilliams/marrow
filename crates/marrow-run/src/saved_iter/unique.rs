use std::ops::ControlFlow;

use marrow_check::CheckedSavedPlace;
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_syntax::SourceSpan;

use crate::durable_read::read_resource;
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{Located, RUN_TYPE, RuntimeError, unsupported};
use crate::store::IndexAddress;
use crate::value::identity_value;

use super::{LoopShape, SavedLoopRow, SavedLoopSpec};

pub(super) struct UniqueIndexScan {
    place: CheckedSavedPlace,
    address: IndexAddress,
    identity_arity: usize,
    index_name: String,
    remaining_key_depth: usize,
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
            place: place.clone(),
            address,
            identity_arity,
            index_name,
            remaining_key_depth,
            shape: spec.shape,
            span: spec.span,
        }
    }

    pub(super) fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::index(self.address.clone())
    }

    pub(super) fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        if self.remaining_key_depth > 0 {
            return Err(unsupported(
                "iterating an incomplete unique index lookup",
                self.span,
            ));
        }
        let page = env
            .store
            .scan_index_tuple(&self.address.index, &self.address.keys, 1)
            .map_err(|error| error.located(self.span))?;
        let Some(entry) = page.entries.first() else {
            return Ok(Flow::Normal);
        };
        let identity = decode_identity_payload_arity(&entry.value, self.identity_arity)
            .ok_or_else(|| RuntimeError {
                throw: None,
                origin: None,
                code: RUN_TYPE,
                message: format!(
                    "the `{}` index entry did not decode to an identity",
                    self.index_name
                ),
                span: self.span,
            })?;
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
        let key = identity_value(identity.clone());
        match self.shape {
            LoopShape::Keys => visit(SavedLoopRow::Single(key), env),
            LoopShape::Values => Err(unsupported("values over a unique index lookup", self.span)),
            LoopShape::Entries => {
                let value = read_resource(&self.place, &identity, self.span, env)?;
                visit(SavedLoopRow::Pair(key, value), env)
            }
        }
    }
}
