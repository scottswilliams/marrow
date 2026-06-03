//! The `marrow serve` request/response protocol.
//!
//! Each request and reply is one JSON object on its own line (newline-delimited).
//! This module is the transport-free core: [`handle_request`] turns one request
//! value into one reply value against checked tree-cell data, so it is
//! unit-tested without sockets. A reply is `{"id": …, "ok": …}` on success or
//! `{"id": …, "error": {"code": …, "message": …}}` on failure, echoing the
//! request's `id`.
//!
//! This is a read-only tooling surface: it never writes managed data. It serves
//! `saved_roots` and the path-addressed reads `saved_get`, `saved_children`, and
//! `saved_walk`.

use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place,
};
use marrow_run::base64;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use serde_json::{Value, json};

use crate::cmd_data::get::{
    DataQuery, DataQuerySegment, presence_name, read_query, render_query_segments,
    resolve_data_query,
};
use crate::cmd_data::inspect::{checked_catalog_id, data_roots_in_store, visit_data_records};

/// A request was malformed — not an object, or missing a string `op`.
pub const PROTOCOL_MALFORMED: &str = "protocol.malformed";
/// A request named an operation the server does not support.
pub const PROTOCOL_UNKNOWN_OP: &str = "protocol.unknown_op";
/// A request named a known operation but its arguments were malformed — a bad
/// path segment, an unknown key type, or a missing `path`.
pub const PROTOCOL_BAD_REQUEST: &str = "protocol.bad_request";

/// A protocol-level failure (a malformed or unsupported request). A storage
/// failure is surfaced separately, carrying the store's own `store.*` code.
#[derive(Debug)]
struct ProtocolError {
    code: &'static str,
    message: String,
}

/// Handle one request, returning its reply. Never fails: every error — protocol
/// or storage — becomes an `error` reply that echoes the request's `id`.
pub fn handle_request(program: &CheckedProgram, store: &TreeStore, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    match dispatch(program, store, request) {
        Ok(result) => json!({ "id": id, "ok": result }),
        Err(error) => json!({
            "id": id,
            "error": { "code": error.code, "message": error.message },
        }),
    }
}

/// Route a request to its operation handler by the `op` field.
fn dispatch(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
) -> Result<Value, ProtocolError> {
    let op = request
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError {
            code: PROTOCOL_MALFORMED,
            message: "request is missing a string `op`".to_string(),
        })?;
    match op {
        "saved_roots" => op_saved_roots(program, store),
        "saved_get" => op_saved_get(program, store, request),
        "saved_children" => op_saved_children(program, store, request),
        "saved_walk" => op_saved_walk(program, store, request),
        other => Err(ProtocolError {
            code: PROTOCOL_UNKNOWN_OP,
            message: format!("unknown operation `{other}`"),
        }),
    }
}

/// `saved_roots` → the project's saved root names, in store order.
fn op_saved_roots(program: &CheckedProgram, store: &TreeStore) -> Result<Value, ProtocolError> {
    let roots = data_roots_in_store(program, store).map_err(store_error)?;
    Ok(json!({ "roots": roots }))
}

/// `saved_get` → the four-state presence at a saved path plus its stored value as
/// base64 (`null` when no value is stored there). The bytes are the store's raw
/// canonical encoding; the client decodes them with the field's schema type.
fn op_saved_get(
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

/// `saved_children` → the distinct immediate children directly below a saved path,
/// in Marrow order: each is a `{"key": …}` (a record/index key) or `{"name": …}`
/// (a field, layer, or index name).
fn op_saved_children(
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

/// The largest `saved_walk` page the server returns, so an unbounded request
/// cannot force a huge scan. A client pages by resubmitting returned cursors.
const MAX_WALK: usize = 10_000;

/// `saved_walk` → up to `limit` `(path, value)` entries in the subtree at a saved
/// path, in Marrow order, plus whether the page was truncated. Each entry's path
/// is the checked logical address; each value is base64. A truncated page returns
/// an opaque `nextCursor`, which can be sent as `cursor` to resume after the last
/// returned entry. The `limit` is required and clamped to [`MAX_WALK`].
fn op_saved_walk(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
) -> Result<Value, ProtocolError> {
    let segments = request_path(request)?;
    let prefix = render_query_segments(&segments);
    let limit = request_walk_limit(request)?;
    let cursor = request
        .get("cursor")
        .map(|value| decode_cursor(value, &prefix))
        .transpose()?;
    let page = checked_walk(program, store, &prefix, cursor.as_deref(), limit)?;
    Ok(json!({
        "entries": page.entries,
        "truncated": page.truncated,
        "nextCursor": page.next_cursor,
    }))
}

fn request_walk_limit(request: &Value) -> Result<usize, ProtocolError> {
    let value = request
        .get("limit")
        .ok_or_else(|| bad_request("`saved_walk` requires an integer `limit`"))?;
    if let Some(limit) = value.as_u64() {
        if limit == 0 {
            return Err(bad_request(
                "`saved_walk` requires a positive integer `limit`",
            ));
        }
        return Ok(limit.min(MAX_WALK as u64) as usize);
    }
    if value.as_i64().is_some() {
        return Err(bad_request(
            "`saved_walk` requires a positive integer `limit`",
        ));
    }
    let Some(number) = value.as_number() else {
        return Err(bad_request("`saved_walk` requires an integer `limit`"));
    };
    if number
        .as_f64()
        .is_some_and(|value| value.is_finite() && value.fract() == 0.0 && value >= u64::MAX as f64)
    {
        return Ok(MAX_WALK);
    }
    let text = number.to_string();
    if text.bytes().all(|byte| byte.is_ascii_digit()) && text != "0" {
        return Ok(MAX_WALK);
    }
    Err(bad_request("`saved_walk` requires an integer `limit`"))
}

/// The decoded `path` of a request, or a `protocol.bad_request` error.
fn request_path(request: &Value) -> Result<Vec<DataQuerySegment>, ProtocolError> {
    let path = request
        .get("path")
        .ok_or_else(|| bad_request("request is missing `path`"))?;
    decode_query_path(path)
}

/// Decode a `path` value — a JSON array of segment objects — into path segments.
fn decode_query_path(value: &Value) -> Result<Vec<DataQuerySegment>, ProtocolError> {
    value
        .as_array()
        .ok_or_else(|| bad_request("`path` must be an array of segments"))?
        .iter()
        .map(decode_segment)
        .collect()
}

/// Decode one path segment: a one-field object tagged by its kind, e.g.
/// `{"root":"books"}`, `{"key":{"int":1}}`, `{"layer":"versions"}`.
fn decode_segment(value: &Value) -> Result<DataQuerySegment, ProtocolError> {
    let (kind, inner) = one_field(value, "a path segment")?;
    let segment = match kind.as_str() {
        "root" => DataQuerySegment::Root(segment_name(inner, "root")?),
        "key" | "index_key" => DataQuerySegment::Key(decode_key(inner)?),
        "field" | "layer" | "index" => DataQuerySegment::Member(segment_name(inner, kind)?),
        other => return Err(bad_request(&format!("unknown path segment `{other}`"))),
    };
    Ok(segment)
}

/// A path segment's string name (for `root`/`field`/`layer`/`index`).
fn segment_name(value: &Value, kind: &str) -> Result<String, ProtocolError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| bad_request(&format!("`{kind}` must name a string")))
}

/// Decode a key value — a one-field object tagged by its type — into a [`SavedKey`].
/// The accepted tags are the [`SavedKey::wire_tag`] of each key kind, so they
/// stay in lockstep with [`encode_key`] and the shared scalar-name table. Wide
/// integer keys (`duration`, `instant`) are strings because JSON numbers cannot
/// hold an `i128`; `int` and `date` are JSON numbers; `bytes` is base64.
fn decode_key(value: &Value) -> Result<SavedKey, ProtocolError> {
    let (tag, inner) = one_field(value, "a key")?;
    let tag = tag.as_str();
    let key = if tag == SavedKey::Int(0).wire_tag() {
        SavedKey::Int(
            inner
                .as_i64()
                .ok_or_else(|| bad_request("`int` key must be an integer"))?,
        )
    } else if tag == SavedKey::Bool(false).wire_tag() {
        SavedKey::Bool(
            inner
                .as_bool()
                .ok_or_else(|| bad_request("`bool` key must be a boolean"))?,
        )
    } else if tag == SavedKey::Str(String::new()).wire_tag() {
        SavedKey::Str(segment_name(inner, "str")?)
    } else if tag == SavedKey::Date(0).wire_tag() {
        let days = inner
            .as_i64()
            .ok_or_else(|| bad_request("`date` key must be an integer"))?;
        SavedKey::Date(i32::try_from(days).map_err(|_| bad_request("`date` key is out of range"))?)
    } else if tag == SavedKey::Duration(0).wire_tag() {
        SavedKey::Duration(parse_i128(inner, "duration")?)
    } else if tag == SavedKey::Instant(0).wire_tag() {
        SavedKey::Instant(parse_i128(inner, "instant")?)
    } else if tag == SavedKey::Bytes(Vec::new()).wire_tag() {
        SavedKey::Bytes(decode_base64_field(inner, "bytes")?)
    } else {
        return Err(bad_request(&format!("unknown key type `{tag}`")));
    };
    Ok(key)
}

/// Encode a [`SavedKey`] back to its one-field JSON form (the inverse of
/// [`decode_key`]), used for `saved_children` output. The tag comes from
/// [`SavedKey::wire_tag`] (sourced from the shared scalar-name table) in the same
/// match as the payload, so tag and value cannot disagree.
fn encode_key(key: &SavedKey) -> Value {
    let tag = key.wire_tag();
    let payload = match key {
        SavedKey::Int(value) => json!(value),
        SavedKey::Bool(value) => json!(value),
        SavedKey::Str(value) => json!(value),
        SavedKey::Date(value) => json!(value),
        SavedKey::Duration(value) => json!(value.to_string()),
        SavedKey::Instant(value) => json!(value.to_string()),
        SavedKey::Bytes(value) => json!(base64::encode(value)),
    };
    json!({ tag: payload })
}

/// The single `(tag, value)` of a one-field object, or a `protocol.bad_request`.
fn one_field<'a>(value: &'a Value, what: &str) -> Result<(&'a String, &'a Value), ProtocolError> {
    let object = value
        .as_object()
        .ok_or_else(|| bad_request(&format!("{what} must be a one-field object")))?;
    if object.len() != 1 {
        return Err(bad_request(&format!("{what} must have exactly one tag")));
    }
    Ok(object.iter().next().expect("exactly one field"))
}

/// Parse a wide integer carried as a decimal string (JSON numbers cannot hold an
/// `i128`).
fn parse_i128(value: &Value, kind: &str) -> Result<i128, ProtocolError> {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| bad_request(&format!("`{kind}` key must be an integer in a string")))
}

/// Decode a base64 string field, or a `protocol.bad_request`.
fn decode_base64_field(value: &Value, kind: &str) -> Result<Vec<u8>, ProtocolError> {
    let text = value
        .as_str()
        .ok_or_else(|| bad_request(&format!("`{kind}` must be a base64 string")))?;
    base64::decode(text).ok_or_else(|| bad_request(&format!("`{kind}` is not valid base64")))
}

fn request_query(program: &CheckedProgram, request: &Value) -> Result<DataQuery, ProtocolError> {
    let segments = request_path(request)?;
    resolve_data_query(program, &segments).map_err(|message| bad_request(&message))
}

fn checked_children(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[DataQuerySegment],
) -> Result<Vec<Value>, ProtocolError> {
    let query = resolve_data_query(program, segments).map_err(|message| bad_request(&message))?;
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

struct WalkPage {
    entries: Vec<Value>,
    truncated: bool,
    next_cursor: Option<String>,
}

fn checked_walk(
    program: &CheckedProgram,
    store: &TreeStore,
    prefix: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<WalkPage, ProtocolError> {
    let mut entries = Vec::new();
    let mut after_cursor = cursor.is_none();
    let mut last_returned = None;
    let mut saw_extra = false;
    visit_data_records(program, store, |record| {
        if !record.path.starts_with(prefix) {
            return Ok(());
        }
        if !after_cursor {
            after_cursor = Some(record.path.as_str()) == cursor;
            return Ok(());
        }
        if entries.len() == limit {
            saw_extra = true;
            return Ok(());
        }
        last_returned = Some(record.path.clone());
        entries.push(json!({
            "path": record.path,
            "value": base64::encode(&record.value),
        }));
        Ok(())
    })
    .map_err(store_error)?;
    let next_cursor = saw_extra
        .then(|| {
            last_returned
                .as_deref()
                .map(|path| base64::encode(path.as_bytes()))
        })
        .flatten();
    Ok(WalkPage {
        entries,
        truncated: saw_extra,
        next_cursor,
    })
}

fn decode_cursor(value: &Value, prefix: &str) -> Result<String, ProtocolError> {
    let cursor = decode_base64_field(value, "cursor")?;
    let cursor =
        String::from_utf8(cursor).map_err(|_| bad_request("`cursor` is not a checked path"))?;
    if !cursor.starts_with(prefix) {
        return Err(bad_request("`cursor` is outside the requested path"));
    }
    Ok(cursor)
}

/// Build a `protocol.bad_request` error.
fn bad_request(message: &str) -> ProtocolError {
    ProtocolError {
        code: PROTOCOL_BAD_REQUEST,
        message: message.to_string(),
    }
}

/// Carry a storage failure through with its own stable `store.*` code.
fn store_error(error: marrow_store::StoreError) -> ProtocolError {
    ProtocolError {
        code: error.code(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
