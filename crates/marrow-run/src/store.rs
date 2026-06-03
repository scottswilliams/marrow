use marrow_check::{CheckedSavedLayer, CheckedSavedMember, CheckedSavedPlace};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_syntax::SourceSpan;

use crate::error::{Located, RUN_STORE, RuntimeError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataAddress {
    pub(crate) store: CatalogId,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) path: Vec<DataPathSegment>,
}

impl DataAddress {
    /// A data address from already-resolved parts: a store catalog id, a record
    /// identity, and a data path of catalog-id member segments and keys. Evolution
    /// apply derives these directly from the checked facts and the live store, so it
    /// addresses cells without re-resolving a member name path.
    pub(crate) fn raw(
        store: CatalogId,
        identity: Vec<SavedKey>,
        path: Vec<DataPathSegment>,
    ) -> Self {
        Self {
            store,
            identity,
            path,
        }
    }

    pub(crate) fn record(
        place: &CheckedSavedPlace,
        identity: &[SavedKey],
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            store: catalog_id(&place.store_catalog_id, "store", span)?,
            identity: identity.to_vec(),
            path: Vec::new(),
        })
    }

    pub(crate) fn member(
        place: &CheckedSavedPlace,
        identity: &[SavedKey],
        layers: &[LayerAddress],
        member_catalog_id: &str,
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        let mut address = Self::record(place, identity, span)?;
        address.path = data_path(layers, Some(member_catalog_id), span)?;
        Ok(address)
    }

    pub(crate) fn layer_prefix(
        place: &CheckedSavedPlace,
        identity: &[SavedKey],
        layers: &[LayerAddress],
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        let mut address = Self::record(place, identity, span)?;
        address.path = data_path(layers, None, span)?;
        Ok(address)
    }

    pub(crate) fn member_path(
        place: &CheckedSavedPlace,
        identity: &[SavedKey],
        layers: &[LayerAddress],
        member_path: &[String],
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        let mut address = Self::layer_prefix(place, identity, layers, span)?;
        address.path.extend(member_path_segments(
            checked_members_for_layers(place, layers),
            member_path,
            span,
        )?);
        Ok(address)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerAddress {
    pub(crate) name: String,
    pub(crate) catalog_id: String,
    pub(crate) keys: Vec<SavedKey>,
}

impl LayerAddress {
    pub(crate) fn from_checked(layer: &CheckedSavedLayer, keys: Vec<SavedKey>) -> Self {
        Self {
            name: layer.name.clone(),
            catalog_id: layer.catalog_id.clone(),
            keys,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexAddress {
    pub(crate) index: CatalogId,
    pub(crate) keys: Vec<SavedKey>,
}

impl IndexAddress {
    pub(crate) fn new(
        catalog_id: &str,
        keys: Vec<SavedKey>,
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            index: self::catalog_id(catalog_id, "store index", span)?,
            keys,
        })
    }

    pub(crate) fn from_place(
        place: &CheckedSavedPlace,
        name: &str,
        keys: Vec<SavedKey>,
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        let Some(index) = place.indexes.iter().find(|index| index.name == name) else {
            return Err(RuntimeError {
                throw: None,
                origin: None,
                code: RUN_STORE,
                message: format!("checked index `{name}` is missing from the executable facts"),
                span,
            });
        };
        Self::new(&index.catalog_id, keys, span)
    }
}

pub(crate) fn catalog_id(
    raw: &str,
    what: &'static str,
    span: SourceSpan,
) -> Result<CatalogId, RuntimeError> {
    CatalogId::new(raw.to_string()).map_err(|_| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_STORE,
        message: format!(
            "checked {what} catalog identity is missing or malformed; accept the catalog proposal before running durable store code"
        ),
        span,
    })
}

pub(crate) fn read_data(
    store: &TreeStore,
    address: &DataAddress,
    span: SourceSpan,
) -> Result<Option<Vec<u8>>, RuntimeError> {
    store
        .read_data_value(&address.store, &address.identity, &address.path)
        .map_err(|error| error.located(span))
}

pub(crate) fn data_exists(
    store: &TreeStore,
    address: &DataAddress,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    store
        .data_subtree_exists(&address.store, &address.identity, &address.path)
        .map_err(|error| error.located(span))
}

pub(crate) fn data_child_count(
    store: &TreeStore,
    address: &DataAddress,
    span: SourceSpan,
) -> Result<usize, RuntimeError> {
    store
        .data_child_count(&address.store, &address.identity, &address.path)
        .map_err(|error| error.located(span))
}

pub(crate) fn max_int_data_child(
    store: &TreeStore,
    address: &DataAddress,
    span: SourceSpan,
) -> Result<Option<i64>, RuntimeError> {
    store
        .max_int_data_child(&address.store, &address.identity, &address.path)
        .map_err(|error| error.located(span))
}

pub(crate) fn max_int_record_child(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    identity_prefix: &[SavedKey],
    span: SourceSpan,
) -> Result<Option<i64>, RuntimeError> {
    let store_id = catalog_id(&place.store_catalog_id, "store", span)?;
    store
        .max_int_record_child(&store_id, identity_prefix)
        .map_err(|error| error.located(span))
}

fn data_path(
    layers: &[LayerAddress],
    terminal_member: Option<&str>,
    span: SourceSpan,
) -> Result<Vec<DataPathSegment>, RuntimeError> {
    let mut path = Vec::new();
    for layer in layers {
        path.push(DataPathSegment::Member(catalog_id(
            &layer.catalog_id,
            "resource member",
            span,
        )?));
        path.extend(layer.keys.iter().cloned().map(DataPathSegment::Key));
    }
    if let Some(member) = terminal_member {
        path.push(DataPathSegment::Member(catalog_id(
            member,
            "resource member",
            span,
        )?));
    }
    Ok(path)
}

fn checked_members_for_layers<'a>(
    place: &'a CheckedSavedPlace,
    layers: &[LayerAddress],
) -> &'a [CheckedSavedMember] {
    let mut members = place.root_members.as_slice();
    for layer in layers {
        let Some(member) = members
            .iter()
            .find(|member| member.catalog_id == layer.catalog_id)
        else {
            return &[];
        };
        members = member.group_members.as_slice();
    }
    members
}

fn member_path_segments(
    mut members: &[CheckedSavedMember],
    member_path: &[String],
    span: SourceSpan,
) -> Result<Vec<DataPathSegment>, RuntimeError> {
    let mut path = Vec::with_capacity(member_path.len());
    for name in member_path {
        let Some(member) = members.iter().find(|member| member.name == *name) else {
            return Err(RuntimeError {
                throw: None,
                origin: None,
                code: RUN_STORE,
                message: format!("checked member `{name}` is missing from the executable facts"),
                span,
            });
        };
        path.push(DataPathSegment::Member(catalog_id(
            &member.catalog_id,
            "resource member",
            span,
        )?));
        members = member.group_members.as_slice();
    }
    Ok(path)
}
