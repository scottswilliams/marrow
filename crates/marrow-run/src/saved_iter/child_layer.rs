use std::ops::ControlFlow;

use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedLayer, CheckedSavedPlace};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::ReadPosition;
use crate::durable_read::{LayerEntryAddress, read_layer_entry, read_layer_entry_at};
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::path::lower;
use crate::read::{first_data_child, next_data_child};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, saved_key_to_value};

use super::{LoopShape, SavedLoopRow, SavedLoopSpec};

pub(super) struct ChildLayerScan {
    place: CheckedSavedPlace,
    identity: Vec<SavedKey>,
    parent_layers: Vec<LayerAddress>,
    layer_facts: CheckedSavedLayer,
    address: DataAddress,
    dir: crate::collection::Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl ChildLayerScan {
    pub(super) fn new(spec: SavedLoopSpec<'_>, env: &mut Env<'_>) -> Result<Self, RuntimeError> {
        let ExecExpr::Field { base, .. } = spec.layer else {
            return Err(unsupported("iterating this saved path", spec.layer.span()));
        };
        let base_path = lower(base, env)?;
        let Some(place) = spec.layer.saved_place() else {
            return Err(unsupported("iterating this saved path", spec.layer.span()));
        };
        let Some(layer_facts) = place.layers.last() else {
            return Err(unsupported("iterating this saved path", spec.layer.span()));
        };
        let mut address_layers = base_path.layer_addresses.clone();
        address_layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
        let address =
            DataAddress::layer_prefix(place, &base_path.identity, &address_layers, spec.span)?;
        Ok(Self {
            place: place.clone(),
            identity: base_path.identity,
            parent_layers: base_path.layer_addresses,
            layer_facts: layer_facts.clone(),
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
        let mut child = first_data_child(env.store, &self.address, self.dir, self.span)?;
        while let Some(key) = child {
            let anchor = key.clone();
            match self.visit_key(key, env, visit)? {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(flow) => return Ok(flow),
            }
            child = next_data_child(env.store, &self.address, &anchor, self.dir, self.span)?;
        }
        Ok(Flow::Normal)
    }

    fn visit_key(
        &self,
        key: SavedKey,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<ControlFlow<Flow>, RuntimeError> {
        let key_value = saved_key_to_value(key.clone())
            .ok_or_else(|| unsupported("iterating keys of this type", self.span))?;
        match self.shape {
            LoopShape::Keys => visit(SavedLoopRow::Single(key_value), env),
            LoopShape::Values => {
                let value = self.read_entry(key, env)?;
                visit(SavedLoopRow::Single(value), env)
            }
            LoopShape::Entries => {
                let value = self.read_entry(key, env)?;
                visit(SavedLoopRow::Pair(key_value, value), env)
            }
        }
    }

    fn read_entry(&self, key: SavedKey, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
        let mut layers = self.parent_layers.clone();
        layers.push(LayerAddress::from_checked(&self.layer_facts, vec![key]));
        if layers.len() == 1 {
            read_layer_entry(
                &self.place,
                &self.identity,
                &self.layer_facts,
                &layers[0].keys,
                ReadPosition::Materialization,
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
                ReadPosition::Materialization,
                self.span,
                env,
            )
        }
    }
}
