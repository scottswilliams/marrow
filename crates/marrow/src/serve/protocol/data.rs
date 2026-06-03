use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place,
};
use marrow_run::base64;
use marrow_store::tree::{DataPathSegment, TreeStore};
use serde_json::{Value, json};

use crate::cmd_data::get::{DataQuery, DataQuerySegment, presence_name, read_query};
use crate::cmd_data::inspect::{checked_catalog_id, data_roots_in_store};

use super::codec::{encode_key, request_path, request_query};
use super::{ProtocolError, bad_request, store_error};

pub(super) fn op_debug_data_roots(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<Value, ProtocolError> {
    let roots = data_roots_in_store(program, store).map_err(store_error)?;
    Ok(json!({ "roots": roots }))
}

pub(super) fn op_debug_data_get(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
) -> Result<Value, ProtocolError> {
    let query = request_query(program, request)?;
    let (value, presence) = read_query(store, &query).map_err(store_error)?;
    Ok(json!({
        "presence": presence_name(presence),
        "value": value.map(|bytes| base64::encode(&bytes)),
    }))
}

pub(super) fn op_debug_data_children(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
) -> Result<Value, ProtocolError> {
    let segments = request_path(request)?;
    if segments.is_empty() {
        let children: Vec<Value> = data_roots_in_store(program, store)
            .map_err(store_error)?
            .into_iter()
            .map(|root| json!({ "name": root }))
            .collect();
        return Ok(json!({ "children": children }));
    }
    let children = checked_children(program, store, &segments)?;
    Ok(json!({ "children": children }))
}

fn checked_children(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[DataQuerySegment],
) -> Result<Vec<Value>, ProtocolError> {
    let query =
        crate::cmd_data::get::resolve_data_query(program, segments).map_err(|m| bad_request(&m))?;
    if query.identity.len() < query.identity_arity {
        return record_children(store, &query);
    }
    let Some((DataQuerySegment::Root(root), _)) = segments.split_first() else {
        return Err(bad_request("path must start with a saved root"));
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| bad_request(&format!("unknown saved root `^{root}`")))?;
    if query.data_path.is_empty() {
        return member_children(store, &query, &place.root_members);
    }
    data_key_children(store, &query)
}

fn record_children(store: &TreeStore, query: &DataQuery) -> Result<Vec<Value>, ProtocolError> {
    let mut children = Vec::new();
    let mut child = store
        .record_first_child(&query.store, &query.identity)
        .map_err(store_error)?;
    while let Some(key) = child {
        let anchor = key.clone();
        children.push(json!({ "key": encode_key(&key) }));
        child = store
            .record_next_child(&query.store, &query.identity, &anchor)
            .map_err(store_error)?;
    }
    Ok(children)
}

fn member_children(
    store: &TreeStore,
    query: &DataQuery,
    members: &[CheckedSavedMember],
) -> Result<Vec<Value>, ProtocolError> {
    let mut children = Vec::new();
    for member in members {
        let catalog =
            checked_catalog_id(&member.catalog_id, "resource member").map_err(store_error)?;
        let path = vec![DataPathSegment::Member(catalog)];
        let present = match &member.kind {
            CheckedSavedMemberKind::Field { .. } => store
                .read_data_value(&query.store, &query.identity, &path)
                .map_err(store_error)?
                .is_some(),
            CheckedSavedMemberKind::Group => store
                .data_subtree_exists(&query.store, &query.identity, &path)
                .map_err(store_error)?,
        };
        if present {
            children.push(json!({ "name": member.name }));
        }
    }
    Ok(children)
}

fn data_key_children(store: &TreeStore, query: &DataQuery) -> Result<Vec<Value>, ProtocolError> {
    let mut children = Vec::new();
    let mut child = store
        .data_first_child(&query.store, &query.identity, &query.data_path)
        .map_err(store_error)?;
    while let Some(key) = child {
        let anchor = key.clone();
        children.push(json!({ "key": encode_key(&key) }));
        child = store
            .data_next_child(&query.store, &query.identity, &query.data_path, &anchor)
            .map_err(store_error)?;
    }
    Ok(children)
}
