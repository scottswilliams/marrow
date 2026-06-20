use std::ops::ControlFlow;

use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedLayer, CheckedSavedPlace};
use marrow_store::key::SavedKey;
use marrow_store::tree::IndexRangeBounds;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::durable_read::{LayerEntryAddress, read_layer_entry, read_layer_entry_at};
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::path::{lower, lower_keys};
use crate::read::{
    first_data_child, first_data_child_in_range, is_key_range_expr, key_range_bounds,
    next_data_child, next_data_child_in_range, validate_scanned_child_key,
};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, saved_key_to_value};

use super::{LoopShape, SavedLoopRow, SavedLoopSpec, shape_row};

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
    shape: LoopShape,
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
            shape: spec.shape,
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
        let mut child = match &self.range {
            Some(range) => {
                first_data_child_in_range(env.store, &self.address, range, self.dir, self.span)?
            }
            None => first_data_child(env.store, &self.address, self.dir, self.span)?,
        };
        while let Some(key) = child {
            let anchor = key.clone();
            match self.visit_key(key, env, visit)? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(flow) => return Ok(flow),
            }
            child = match &self.range {
                Some(range) => next_data_child_in_range(
                    env.store,
                    &self.address,
                    &anchor,
                    range,
                    self.dir,
                    self.span,
                )?,
                None => next_data_child(env.store, &self.address, &anchor, self.dir, self.span)?,
            };
        }
        Ok(Flow::Normal)
    }

    fn visit_key(
        &self,
        key: SavedKey,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        validate_scanned_child_key(&self.key_scalars, self.exact_prefix.len(), &key, self.span)?;
        let key_value = saved_key_to_value(key.clone(), self.span)?;
        let row = shape_row(self.shape, key_value, || self.read_entry(key, env))?;
        visit(row, env)
    }

    fn read_entry(&self, key: SavedKey, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
        let mut layers = self.parent_layers.clone();
        let mut keys = self.exact_prefix.clone();
        keys.push(key);
        layers.push(LayerAddress::from_checked(&self.layer_facts, keys));
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
            false,
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
    let exact_prefix = lower_keys(&layer.args, span, false, None, &layer.key_params, env)?;
    Ok((exact_prefix, None))
}
