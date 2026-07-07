use std::ops::ControlFlow;

use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedLayer, CheckedSavedPlace};
use marrow_store::key::SavedKey;
use marrow_store::tree::IndexRangeBounds;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::durable_read::{LayerEntryAddress, read_layer_entry, read_layer_entry_at};
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::path::{KeyRole, lower, lower_keys};
use crate::read::{
    first_data_child, first_data_child_in_range, is_key_range_expr, key_range_bounds,
    next_data_child, next_data_child_in_range, validate_scanned_child_key,
};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, saved_key_to_value};

use super::{SavedLoopRow, SavedLoopSpec, saved_loop_row};

pub(super) struct ChildLayerScan {
    place: CheckedSavedPlace,
    identity: Vec<SavedKey>,
    parent_layers: Vec<LayerAddress>,
    layer_facts: CheckedSavedLayer,
    key_scalars: Vec<Option<ScalarType>>,
    exact_prefix: Vec<SavedKey>,
    range: Option<IndexRangeBounds>,
    address: DataAddress,
    dir: crate::collection::Direction,
    /// The number of key columns this head streams. One for a key-first single
    /// binding or a two-name single-column layer; more for an (n+1)-name head over a
    /// composite layer, which walks every remaining column outermost-first. A range
    /// bound coincides only with a single streamed column.
    key_columns: usize,
    with_value: bool,
    span: SourceSpan,
}

impl ChildLayerScan {
    pub(super) fn new(spec: SavedLoopSpec<'_>, env: &mut Env<'_>) -> Result<Self, RuntimeError> {
        let base = child_layer_base(spec.layer)?;
        let base_path = lower(base, env)?;
        let Some(place) = spec.layer.saved_place() else {
            return Err(unsupported("iterating this saved path", spec.layer.span()));
        };
        let Some(layer_facts) = place.layers.last() else {
            return Err(unsupported("iterating this saved path", spec.layer.span()));
        };
        let (exact_prefix, range) = layer_key_range(layer_facts, spec.span, env)?;
        let mut address_layers = base_path.layer_addresses.clone();
        address_layers.push(LayerAddress::from_checked(
            layer_facts,
            exact_prefix.clone(),
        ));
        let address =
            DataAddress::layer_prefix(place, &base_path.identity, &address_layers, spec.span)?;
        Ok(Self {
            place: place.clone(),
            identity: base_path.identity,
            parent_layers: base_path.layer_addresses,
            layer_facts: layer_facts.clone(),
            key_scalars: layer_facts
                .key_params
                .iter()
                .map(|param| param.scalar)
                .collect(),
            exact_prefix,
            range,
            address,
            dir: spec.dir,
            key_columns: spec.key_columns,
            with_value: spec.with_value,
            span: spec.span,
        })
    }

    pub(super) fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::data(self.address.clone())
    }

    pub(super) fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        let mut columns = Vec::with_capacity(self.key_columns);
        match self.stream_columns(self.exact_prefix.clone(), &mut columns, env, visit)? {
            ControlFlow::Continue(()) => Ok(Flow::Normal),
            ControlFlow::Break(flow) => Ok(flow),
        }
    }

    /// Walk the remaining key columns depth-first. `prefix` is the pinned exact keys
    /// plus the columns bound so far; `columns` accumulates their bound values. At the
    /// last column the row is produced with the leaf value read once when the head
    /// binds it. A range bound applies only to a single streamed column, so it is
    /// consulted only at the final depth.
    fn stream_columns(
        &self,
        prefix: Vec<SavedKey>,
        columns: &mut Vec<Value>,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        let last_column = columns.len() + 1 == self.key_columns;
        let address = self.address_at(&prefix)?;
        let mut child = match (&self.range, last_column) {
            (Some(range), true) => {
                first_data_child_in_range(env.store, &address, range, self.dir, self.span)?
            }
            _ => first_data_child(env.store, &address, self.dir, self.span)?,
        };
        while let Some(key) = child {
            let anchor = key.clone();
            validate_scanned_child_key(&self.key_scalars, prefix.len(), &key, self.span)?;
            columns.push(saved_key_to_value(key.clone(), self.span)?);
            let mut next_prefix = prefix.clone();
            next_prefix.push(key);
            let flow = if last_column {
                let row = saved_loop_row(self.with_value, columns.clone(), || {
                    self.read_entry(&next_prefix, env)
                })?;
                visit(row, env)?
            } else {
                self.stream_columns(next_prefix, columns, env, visit)?
            };
            columns.pop();
            if let ControlFlow::Break(flow) = flow {
                return Ok(ControlFlow::Break(flow));
            }
            child = match (&self.range, last_column) {
                (Some(range), true) => next_data_child_in_range(
                    env.store, &address, &anchor, range, self.dir, self.span,
                )?,
                _ => next_data_child(env.store, &address, &anchor, self.dir, self.span)?,
            };
        }
        Ok(ControlFlow::Continue(()))
    }

    /// The data address of the layer under a given key prefix (the pinned keys plus
    /// the columns bound so far).
    fn address_at(&self, keys: &[SavedKey]) -> Result<DataAddress, RuntimeError> {
        let mut address_layers = self.parent_layers.clone();
        address_layers.push(LayerAddress::from_checked(&self.layer_facts, keys.to_vec()));
        DataAddress::layer_prefix(&self.place, &self.identity, &address_layers, self.span)
    }

    fn read_entry(&self, keys: &[SavedKey], env: &mut Env<'_>) -> Result<Value, RuntimeError> {
        let mut layers = self.parent_layers.clone();
        layers.push(LayerAddress::from_checked(&self.layer_facts, keys.to_vec()));
        if layers.len() == 1 {
            read_layer_entry(
                &self.place,
                &self.identity,
                &self.layer_facts,
                &layers[0].keys,
                self.span,
                env,
            )
        } else {
            read_layer_entry_at(
                LayerEntryAddress {
                    place: &self.place,
                    identity: &self.identity,
                    layers: &layers,
                    layer_facts: &self.layer_facts,
                },
                self.span,
                env,
            )
        }
    }
}

fn child_layer_base(layer: &ExecExpr) -> Result<&ExecExpr, RuntimeError> {
    match layer {
        ExecExpr::Field { base, .. } => Ok(base),
        ExecExpr::Call { callee, .. } => match callee.as_ref() {
            ExecExpr::Field { base, .. } => Ok(base),
            _ => Err(unsupported("iterating this saved path", layer.span())),
        },
        _ => Err(unsupported("iterating this saved path", layer.span())),
    }
}

/// The exact key prefix and optional final range bound an iterable layer pins. A
/// partial key prefix descends into the inner sub-layer of a composite layer; a
/// trailing range bounds the iterated column under that prefix. The streamed column
/// is the first one left unpinned, so a layer with neither extra keys nor a range
/// (zero args) streams its outermost column with an empty prefix.
fn layer_key_range(
    layer: &CheckedSavedLayer,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(Vec<SavedKey>, Option<IndexRangeBounds>), RuntimeError> {
    if let Some(range_position) = layer
        .args
        .iter()
        .position(|arg| is_key_range_expr(&arg.value))
    {
        if range_position + 1 != layer.args.len() || layer.args.len() != layer.key_params.len() {
            return Err(unsupported("iterating this saved path", span));
        }
        let exact_prefix = lower_keys(
            &layer.args[..range_position],
            span,
            KeyRole::Layer,
            None,
            &layer.key_params,
            env,
        )?;
        let range = key_range_bounds(
            &layer.args[range_position].value,
            &layer.key_params[range_position],
            span,
            env,
        )?;
        return Ok((exact_prefix, range));
    }
    if layer.args.len() >= layer.key_params.len() {
        return Err(unsupported("iterating this saved path", span));
    }
    let exact_prefix = lower_keys(
        &layer.args,
        span,
        KeyRole::Layer,
        None,
        &layer.key_params,
        env,
    )?;
    Ok((exact_prefix, None))
}
