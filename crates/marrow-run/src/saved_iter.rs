//! Streaming saved-layer iteration for `for` loops.

use std::ops::ControlFlow;

use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedLayer, CheckedSavedPlace};
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, MaterializeKind, ReadPosition, values_or_entries};
use crate::durable_read::{
    LayerEntryAddress, read_layer_entry, read_layer_entry_at, read_resource,
};
use crate::env::{Env, Flow, TraversedLayer};
use crate::error::{Located, RUN_TYPE, RuntimeError, unsupported};
use crate::path::lower;
use crate::read::{
    INDEX_SCAN_PAGE_LIMIT, IndexBranchAddress, collected_identity_value, first_data_child,
    first_index_child, first_record_child, iterable_layer, keys_argument, next_data_child,
    next_index_child, next_record_child, reversed_argument,
};
use crate::stdlib::{check_key_collection, unique_index_lookup};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, identity_value, saved_key_to_value};

#[derive(Clone, Copy)]
enum LoopShape {
    Keys,
    Values,
    Entries,
}

pub(crate) enum SavedLoopRow {
    Single(Value),
    Pair(Value, Value),
}

pub(crate) struct SavedLoopSpec<'a> {
    layer: &'a ExecExpr,
    dir: Direction,
    shape: LoopShape,
    from_keys_builtin: bool,
    span: SourceSpan,
}

impl<'a> SavedLoopSpec<'a> {
    pub(crate) fn from_iterable(iterable: &'a ExecExpr, two_name: bool) -> Option<Self> {
        let (iterable, dir) = match reversed_argument(iterable) {
            Some(inner) => (inner, Direction::Descending),
            None => (iterable, Direction::Ascending),
        };
        if let Some(layer) = keys_argument(iterable) {
            return (!two_name && layer.saved_place().is_some()).then_some(Self {
                layer,
                dir,
                shape: LoopShape::Keys,
                from_keys_builtin: true,
                span: iterable.span(),
            });
        }
        if let Some(inner) = values_or_entries(iterable) {
            let shape = match inner.kind {
                MaterializeKind::Values => {
                    if two_name {
                        return None;
                    }
                    LoopShape::Values
                }
                MaterializeKind::Entries => LoopShape::Entries,
            };
            return inner.layer.saved_place().is_some().then_some(Self {
                layer: inner.layer,
                dir,
                shape,
                from_keys_builtin: false,
                span: iterable.span(),
            });
        }
        iterable.saved_place().is_some().then_some(Self {
            layer: iterable,
            dir,
            shape: if two_name {
                LoopShape::Entries
            } else {
                LoopShape::Keys
            },
            from_keys_builtin: false,
            span: iterable.span(),
        })
    }

    pub(crate) fn run(
        self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        if self.from_keys_builtin {
            check_key_collection(self.layer, self.span, env)?;
        }
        let plan = SavedLoopPlan::new(self, env)?;
        plan.run(env, visit)
    }
}

enum SavedLoopPlan {
    Root(RootScan),
    Index(IndexScan),
    UniqueIndex(UniqueIndexScan),
    ChildLayer(Box<ChildLayerScan>),
}

impl SavedLoopPlan {
    fn new(spec: SavedLoopSpec<'_>, env: &mut Env<'_>) -> Result<Self, RuntimeError> {
        if let Some(lookup) = unique_index_lookup(spec.layer, env)? {
            let Some(place) = spec.layer.saved_place() else {
                return Err(unsupported("iterating this saved path", spec.layer.span()));
            };
            return Ok(Self::UniqueIndex(UniqueIndexScan {
                place: place.clone(),
                address: lookup.address,
                identity_arity: lookup.identity_arity,
                index_name: lookup.index_name,
                remaining_key_depth: lookup.remaining_key_depth,
                shape: spec.shape,
                span: spec.span,
            }));
        }
        match iterable_layer(spec.layer, env)? {
            crate::read::IterableLayer::Root(place) => Ok(Self::Root(RootScan::new(place, spec)?)),
            crate::read::IterableLayer::Index(place, branch) => {
                Ok(Self::Index(IndexScan::new(place, branch, spec)))
            }
            crate::read::IterableLayer::ChildLayer => {
                Ok(Self::ChildLayer(Box::new(ChildLayerScan::new(spec, env)?)))
            }
        }
    }

    fn run(
        self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        let layer = self.traversed_layer();
        env.traversed_layers.push(layer);
        let result = self.stream(env, visit);
        env.traversed_layers.pop();
        result
    }

    fn traversed_layer(&self) -> TraversedLayer {
        match self {
            Self::Root(scan) => scan.traversed_layer(),
            Self::Index(scan) => scan.traversed_layer(),
            Self::UniqueIndex(scan) => scan.traversed_layer(),
            Self::ChildLayer(scan) => scan.traversed_layer(),
        }
    }

    fn stream(
        &self,
        env: &mut Env<'_>,
        visit: &mut impl FnMut(SavedLoopRow, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
    ) -> Result<Flow, RuntimeError> {
        match self {
            Self::Root(scan) => scan.stream(env, visit),
            Self::Index(scan) => scan.stream(env, visit),
            Self::UniqueIndex(scan) => scan.stream(env, visit),
            Self::ChildLayer(scan) => scan.stream(env, visit),
        }
    }
}

struct RootScan {
    place: CheckedSavedPlace,
    store: marrow_store::cell::CatalogId,
    arity: usize,
    dir: Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl RootScan {
    fn new(place: &CheckedSavedPlace, spec: SavedLoopSpec<'_>) -> Result<Self, RuntimeError> {
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

    fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::Record {
            store: self.store.clone(),
        }
    }

    fn stream(
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

struct IndexScan {
    place: CheckedSavedPlace,
    branch: IndexBranchAddress,
    yields_identity: bool,
    dir: Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl IndexScan {
    fn new(place: &CheckedSavedPlace, branch: IndexBranchAddress, spec: SavedLoopSpec<'_>) -> Self {
        Self {
            place: place.clone(),
            yields_identity: branch.arg_keys.len() >= branch.identity_start,
            branch,
            dir: spec.dir,
            shape: spec.shape,
            span: spec.span,
        }
    }

    fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::index(self.branch.index.clone())
    }

    fn stream(
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

struct UniqueIndexScan {
    place: CheckedSavedPlace,
    address: crate::store::IndexAddress,
    identity_arity: usize,
    index_name: String,
    remaining_key_depth: usize,
    shape: LoopShape,
    span: SourceSpan,
}

impl UniqueIndexScan {
    fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::index(self.address.clone())
    }

    fn stream(
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

struct ChildLayerScan {
    place: CheckedSavedPlace,
    identity: Vec<SavedKey>,
    parent_layers: Vec<LayerAddress>,
    layer_facts: CheckedSavedLayer,
    address: DataAddress,
    dir: Direction,
    shape: LoopShape,
    span: SourceSpan,
}

impl ChildLayerScan {
    fn new(spec: SavedLoopSpec<'_>, env: &mut Env<'_>) -> Result<Self, RuntimeError> {
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

    fn traversed_layer(&self) -> TraversedLayer {
        TraversedLayer::data(self.address.clone())
    }

    fn stream(
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
