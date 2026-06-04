use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place,
};
use marrow_run::base64;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use serde_json::{Value, json};

use crate::cmd_data::get::{DataQuery, DataQuerySegment, presence_name, read_query};
use crate::cmd_data::inspect::{checked_catalog_id, data_roots_in_store};

use super::codec::{encode_key, request_path, request_query};
use super::cursor::CursorState;
use super::walk::MAX_WALK;
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
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let segments = request_path(request)?;
    if segments.is_empty() {
        let children: Vec<Value> = data_roots_in_store(program, store)
            .map_err(store_error)?
            .into_iter()
            .map(|root| json!({ "name": root }))
            .collect();
        return Ok(json!({ "children": children, "truncated": false, "cursor": Value::Null }));
    }
    let limit = request_children_limit(request)?;
    checked_children(program, store, request, &segments, limit, cursors)
}

/// The page size for a key-scanning `debug_data_children` request: a positive
/// integer `limit`, an oversized one clamped to [`MAX_WALK`] (not rejected), or the
/// server max when `limit` is omitted.
fn request_children_limit(request: &Value) -> Result<usize, ProtocolError> {
    let Some(value) = request.get("limit") else {
        return Ok(MAX_WALK);
    };
    let invalid = bad_request("`debug_data_children` `limit` must be a positive integer");
    match value.as_u64() {
        // A positive integer is used as the page size, clamped to the server max.
        Some(limit) if limit > 0 => Ok(limit.min(MAX_WALK as u64) as usize),
        // Zero is not a page size.
        Some(_) => Err(invalid),
        // A negative integer is rejected. A non-integer JSON number, or a magnitude
        // beyond u64, clamps to the max. Any non-number is not a valid limit.
        None if value.as_i64().is_some_and(|limit| limit < 0) => Err(invalid),
        None if value.is_number() => Ok(MAX_WALK),
        None => Err(invalid),
    }
}

fn checked_children(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
    segments: &[DataQuerySegment],
    limit: usize,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let query =
        crate::cmd_data::get::resolve_data_query(program, segments).map_err(|m| bad_request(&m))?;
    let resume = request
        .get("cursor")
        .map(|value| cursors.decode_children_cursor(value, &query.path))
        .transpose()?;
    if query.identity.len() < query.identity_arity {
        return record_children(store, &query, limit, resume.as_ref(), cursors);
    }
    if resume.is_none() {
        let Some((DataQuerySegment::Root(root), _)) = segments.split_first() else {
            return Err(bad_request("path must start with a saved root"));
        };
        let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
            .ok_or_else(|| bad_request(&format!("unknown saved root `^{root}`")))?;
        if query.data_path.is_empty() {
            let children = member_children(store, &query, &place.root_members)?;
            return Ok(json!({ "children": children, "truncated": false, "cursor": Value::Null }));
        }
    }
    if query.data_path.is_empty() {
        return Err(bad_request(
            "declared members are not a paged child scan, so they take no `cursor`",
        ));
    }
    data_key_children(store, &query, limit, resume.as_ref(), cursors)
}

/// Page the record-key children under an identity prefix, mirroring how
/// `debug_data_walk` signs and validates its resume cursor.
fn record_children(
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
    resume: Option<&SavedKey>,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let first = match resume {
        Some(anchor) => store
            .record_next_child(&query.store, &query.identity, anchor)
            .map_err(store_error)?,
        None => store
            .record_first_child(&query.store, &query.identity)
            .map_err(store_error)?,
    };
    page_key_children(query, limit, cursors, first, |anchor| {
        store
            .record_next_child(&query.store, &query.identity, anchor)
            .map_err(store_error)
    })
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

/// Page the keyed children under a data path, mirroring how `debug_data_walk` signs
/// and validates its resume cursor.
fn data_key_children(
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
    resume: Option<&SavedKey>,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let first = match resume {
        Some(anchor) => store
            .data_next_child(&query.store, &query.identity, &query.data_path, anchor)
            .map_err(store_error)?,
        None => store
            .data_first_child(&query.store, &query.identity, &query.data_path)
            .map_err(store_error)?,
    };
    page_key_children(query, limit, cursors, first, |anchor| {
        store
            .data_next_child(&query.store, &query.identity, &query.data_path, anchor)
            .map_err(store_error)
    })
}

/// Collect up to `limit` keyed children, walking forward with `next`. When a child
/// remains past the limit, report `truncated` and a signed resume cursor anchored
/// at the last returned key, so resuming on the same path returns the rest.
fn page_key_children(
    query: &DataQuery,
    limit: usize,
    cursors: &CursorState,
    first: Option<SavedKey>,
    mut next: impl FnMut(&SavedKey) -> Result<Option<SavedKey>, ProtocolError>,
) -> Result<Value, ProtocolError> {
    let mut children = Vec::new();
    let mut last = None;
    let mut child = first;
    while let Some(key) = child {
        if children.len() == limit {
            let anchor = last.as_ref().expect("a truncated page returned a child");
            let cursor = cursors.encode_children_cursor(&query.path, anchor);
            return Ok(json!({ "children": children, "truncated": true, "cursor": cursor }));
        }
        children.push(json!({ "key": encode_key(&key) }));
        child = next(&key)?;
        last = Some(key);
    }
    Ok(json!({ "children": children, "truncated": false, "cursor": Value::Null }))
}
