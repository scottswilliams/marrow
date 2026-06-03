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
//! typed `data_roots`, `data_get`, `data_children`, and `data_walk` queries.

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
use crate::cmd_data::inspect::{checked_catalog_id, data_roots_in_store};

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
        "data_roots" => op_data_roots(program, store),
        "data_get" => op_data_get(program, store, request),
        "data_children" => op_data_children(program, store, request),
        "data_walk" => op_data_walk(program, store, request),
        other => Err(ProtocolError {
            code: PROTOCOL_UNKNOWN_OP,
            message: format!("unknown operation `{other}`"),
        }),
    }
}

/// `data_roots` returns the project's stored root names in store order.
fn op_data_roots(program: &CheckedProgram, store: &TreeStore) -> Result<Value, ProtocolError> {
    let roots = data_roots_in_store(program, store).map_err(store_error)?;
    Ok(json!({ "roots": roots }))
}

/// `data_get` returns presence at a checked data query plus its canonical
/// payload as base64 (`null` when no value is stored there).
fn op_data_get(
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

/// `data_children` returns distinct immediate children below a checked data
/// query in Marrow order.
fn op_data_children(
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

/// The largest `data_walk` page the server returns, so an unbounded request
/// cannot force a huge scan. A client pages by resubmitting returned cursors.
const MAX_WALK: usize = 10_000;

/// `data_walk` returns up to `limit` `(path, value)` entries in the typed data
/// subtree, in Marrow order, plus whether the page was truncated. A truncated
/// page returns an opaque `nextCursor`, which can be sent as `cursor` to resume
/// at the next page position.
fn op_data_walk(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
) -> Result<Value, ProtocolError> {
    let query = request_query(program, request)?;
    let limit = request_walk_limit(request)?;
    let cursor = request
        .get("cursor")
        .map(|value| decode_cursor(program, value, &query))
        .transpose()?;
    let page = checked_walk(program, store, &query, cursor.as_ref(), limit)?;
    Ok(json!({
        "entries": page.entries,
        "truncated": page.truncated,
        "nextCursor": page.next_cursor,
    }))
}

fn request_walk_limit(request: &Value) -> Result<usize, ProtocolError> {
    let value = request
        .get("limit")
        .ok_or_else(|| bad_request("`data_walk` requires an integer `limit`"))?;
    if let Some(limit) = value.as_u64() {
        if limit == 0 {
            return Err(bad_request(
                "`data_walk` requires a positive integer `limit`",
            ));
        }
        return Ok(limit.min(MAX_WALK as u64) as usize);
    }
    if value.as_i64().is_some() {
        return Err(bad_request(
            "`data_walk` requires a positive integer `limit`",
        ));
    }
    let Some(number) = value.as_number() else {
        return Err(bad_request("`data_walk` requires an integer `limit`"));
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
    Err(bad_request("`data_walk` requires an integer `limit`"))
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
        "key" => DataQuerySegment::Key(decode_key(inner)?),
        "field" => DataQuerySegment::Field(segment_name(inner, kind)?),
        "layer" => DataQuerySegment::Layer(segment_name(inner, kind)?),
        other => return Err(bad_request(&format!("unknown path segment `{other}`"))),
    };
    Ok(segment)
}

/// A path segment's string name (for `root`/`field`/`layer`).
fn segment_name(value: &Value, kind: &str) -> Result<String, ProtocolError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| bad_request(&format!("`{kind}` must name a string")))
}

/// Decode a key value — a one-field object tagged by its type — into a [`SavedKey`].
/// Wide integer keys (`duration`, `instant`) are strings because JSON numbers
/// cannot hold an `i128`; `int` and `date` are JSON numbers; `bytes` is base64.
fn decode_key(value: &Value) -> Result<SavedKey, ProtocolError> {
    let (tag, inner) = one_field(value, "a key")?;
    let key = match tag.as_str() {
        "int" => SavedKey::Int(
            inner
                .as_i64()
                .ok_or_else(|| bad_request("`int` key must be an integer"))?,
        ),
        "bool" => SavedKey::Bool(
            inner
                .as_bool()
                .ok_or_else(|| bad_request("`bool` key must be a boolean"))?,
        ),
        "str" => SavedKey::Str(segment_name(inner, "str")?),
        "date" => {
            let days = inner
                .as_i64()
                .ok_or_else(|| bad_request("`date` key must be an integer"))?;
            SavedKey::Date(
                i32::try_from(days).map_err(|_| bad_request("`date` key is out of range"))?,
            )
        }
        "duration" => SavedKey::Duration(parse_i128(inner, "duration")?),
        "instant" => SavedKey::Instant(parse_i128(inner, "instant")?),
        "bytes" => SavedKey::Bytes(decode_base64_field(inner, "bytes")?),
        other => return Err(bad_request(&format!("unknown key type `{other}`"))),
    };
    Ok(key)
}

/// Encode a [`SavedKey`] back to its one-field JSON form.
fn encode_key(key: &SavedKey) -> Value {
    let tag = key_json_tag(key);
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

fn key_json_tag(key: &SavedKey) -> &'static str {
    match key {
        SavedKey::Int(_) => "int",
        SavedKey::Bool(_) => "bool",
        SavedKey::Str(_) => "str",
        SavedKey::Date(_) => "date",
        SavedKey::Duration(_) => "duration",
        SavedKey::Instant(_) => "instant",
        SavedKey::Bytes(_) => "bytes",
    }
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
    query: &DataQuery,
    cursor: Option<&DataQuery>,
    limit: usize,
) -> Result<WalkPage, ProtocolError> {
    let place =
        checked_saved_root_place(program, &query.root, marrow_syntax::SourceSpan::default())
            .ok_or_else(|| bad_request(&format!("unknown saved root `^{}`", query.root)))?;
    let mut state = WalkState::new(limit);
    let start = cursor.unwrap_or(query);
    if !query_under_prefix(start, query) {
        return Err(bad_request("`cursor` is outside the requested path"));
    }
    if cursor.is_some() && start.identity.len() != query.identity_arity {
        return Err(bad_request("`cursor` is not a data_walk position"));
    }

    let mut identity = if cursor.is_some() {
        Some(start.identity.clone())
    } else {
        first_identity_under(store, query, &query.identity)?
    };
    let mut cursor_pending = cursor.map(|cursor| cursor.data_path.clone());
    while let Some(current) = identity {
        if !current.starts_with(&query.identity) {
            break;
        }
        let mut path = Vec::with_capacity(1 + current.len());
        path.push(DataQuerySegment::Root(query.root.clone()));
        path.extend(current.iter().cloned().map(DataQuerySegment::Key));
        let start_path = cursor_pending.take();
        let mut waiting_for_cursor = start_path.is_some();
        walk_members(
            WalkMembers {
                store,
                store_id: &query.store,
                identity: &current,
                filter_prefix: &query.data_path,
                start_path: start_path.as_deref(),
                waiting_for_cursor: &mut waiting_for_cursor,
                state: &mut state,
            },
            &place.root_members,
            &mut Vec::new(),
            &mut path,
        )?;
        if waiting_for_cursor {
            return Err(bad_request("`cursor` does not name a data_walk entry"));
        }
        if state.next_cursor.is_some() {
            break;
        }
        identity = next_identity_after(store, query, &current)?;
    }
    Ok(WalkPage {
        entries: state.entries,
        truncated: state.next_cursor.is_some(),
        next_cursor: state.next_cursor,
    })
}

struct WalkState {
    entries: Vec<Value>,
    limit: usize,
    next_cursor: Option<String>,
}

impl WalkState {
    fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit,
            next_cursor: None,
        }
    }

    fn push(&mut self, path: &[DataQuerySegment], value: Vec<u8>) {
        if self.entries.len() == self.limit {
            self.next_cursor = Some(encode_cursor(path));
            return;
        }
        self.entries.push(json!({
            "path": render_query_segments(path),
            "value": base64::encode(&value),
        }));
    }
}

struct WalkMembers<'a, 'b> {
    store: &'a TreeStore,
    store_id: &'a marrow_store::cell::CatalogId,
    identity: &'a [SavedKey],
    filter_prefix: &'a [DataPathSegment],
    start_path: Option<&'a [DataPathSegment]>,
    waiting_for_cursor: &'b mut bool,
    state: &'b mut WalkState,
}

fn walk_members(
    mut walk: WalkMembers<'_, '_>,
    members: &[CheckedSavedMember],
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
) -> Result<(), ProtocolError> {
    for member in members {
        if walk.state.next_cursor.is_some() {
            break;
        }
        walk_member(&mut walk, member, data_path, path)?;
    }
    Ok(())
}

fn walk_member(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
) -> Result<(), ProtocolError> {
    let catalog = checked_catalog_id(&member.catalog_id, "resource member").map_err(store_error)?;
    data_path.push(DataPathSegment::Member(catalog));
    path.push(query_segment_for_member(member));
    if path_can_match(data_path, walk.filter_prefix) {
        if member.key_params.is_empty() {
            walk_member_terminal(walk, member, data_path, path)?;
        } else {
            walk_member_keys(walk, member, data_path, path, 0)?;
        }
    }
    path.pop();
    data_path.pop();
    Ok(())
}

fn query_segment_for_member(member: &CheckedSavedMember) -> DataQuerySegment {
    if member.key_params.is_empty() && matches!(member.kind, CheckedSavedMemberKind::Field { .. }) {
        DataQuerySegment::Field(member.name.clone())
    } else {
        DataQuerySegment::Layer(member.name.clone())
    }
}

fn walk_member_keys(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
    key_index: usize,
) -> Result<(), ProtocolError> {
    if key_index == member.key_params.len() {
        return walk_member_terminal(walk, member, data_path, path);
    }
    if let Some(selection) = selected_key(walk, data_path) {
        let key = selection.key().clone();
        let was_waiting_for_cursor = *walk.waiting_for_cursor;
        walk_member_key(walk, member, data_path, path, key_index, key.clone())?;
        if selection.resumes_after_key()
            && was_waiting_for_cursor
            && !*walk.waiting_for_cursor
            && walk.state.next_cursor.is_none()
        {
            walk_member_keys_after(walk, member, data_path, path, key_index, &key)?;
        }
        return Ok(());
    }
    let mut child = walk
        .store
        .data_first_child(walk.store_id, walk.identity, data_path)
        .map_err(store_error)?;
    while let Some(key) = child {
        let anchor = key.clone();
        walk_member_key(walk, member, data_path, path, key_index, key)?;
        if walk.state.next_cursor.is_some() {
            break;
        }
        child = walk
            .store
            .data_next_child(walk.store_id, walk.identity, data_path, &anchor)
            .map_err(store_error)?;
    }
    Ok(())
}

enum SelectedKey {
    Filter(SavedKey),
    Cursor(SavedKey),
}

impl SelectedKey {
    fn key(&self) -> &SavedKey {
        match self {
            Self::Filter(key) | Self::Cursor(key) => key,
        }
    }

    fn resumes_after_key(&self) -> bool {
        matches!(self, Self::Cursor(_))
    }
}

fn selected_key(walk: &WalkMembers<'_, '_>, data_path: &[DataPathSegment]) -> Option<SelectedKey> {
    let next_segment = data_path.len();
    if let Some(DataPathSegment::Key(key)) = walk.filter_prefix.get(next_segment) {
        return Some(SelectedKey::Filter(key.clone()));
    }
    if !*walk.waiting_for_cursor {
        return None;
    }
    let start_path = walk.start_path?;
    match start_path.get(next_segment) {
        Some(DataPathSegment::Key(key)) => Some(SelectedKey::Cursor(key.clone())),
        _ => None,
    }
}

fn walk_member_key(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
    key_index: usize,
    key: SavedKey,
) -> Result<(), ProtocolError> {
    data_path.push(DataPathSegment::Key(key.clone()));
    path.push(DataQuerySegment::Key(key));
    if path_can_match(data_path, walk.filter_prefix) {
        walk_member_keys(walk, member, data_path, path, key_index + 1)?;
    }
    path.pop();
    data_path.pop();
    Ok(())
}

fn walk_member_keys_after(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
    key_index: usize,
    anchor: &SavedKey,
) -> Result<(), ProtocolError> {
    let mut child = walk
        .store
        .data_next_child(walk.store_id, walk.identity, data_path, anchor)
        .map_err(store_error)?;
    while let Some(key) = child {
        let anchor = key.clone();
        walk_member_key(walk, member, data_path, path, key_index, key)?;
        if walk.state.next_cursor.is_some() {
            break;
        }
        child = walk
            .store
            .data_next_child(walk.store_id, walk.identity, data_path, &anchor)
            .map_err(store_error)?;
    }
    Ok(())
}

fn walk_member_terminal(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
) -> Result<(), ProtocolError> {
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => {
            if !data_path.starts_with(walk.filter_prefix) {
                return Ok(());
            }
            let waiting_for_this_path = *walk.waiting_for_cursor;
            if waiting_for_this_path && Some(data_path.as_slice()) != walk.start_path {
                return Ok(());
            }
            if let Some(value) = walk
                .store
                .read_data_value(walk.store_id, walk.identity, data_path)
                .map_err(store_error)?
            {
                if waiting_for_this_path {
                    *walk.waiting_for_cursor = false;
                }
                walk.state.push(path, value);
            }
        }
        CheckedSavedMemberKind::Group => walk_members(
            WalkMembers {
                store: walk.store,
                store_id: walk.store_id,
                identity: walk.identity,
                filter_prefix: walk.filter_prefix,
                start_path: walk.start_path,
                waiting_for_cursor: walk.waiting_for_cursor,
                state: walk.state,
            },
            &member.group_members,
            data_path,
            path,
        )?,
    }
    Ok(())
}

fn path_can_match(path: &[DataPathSegment], filter: &[DataPathSegment]) -> bool {
    path.starts_with(filter) || filter.starts_with(path)
}

fn first_identity_under(
    store: &TreeStore,
    query: &DataQuery,
    prefix: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, ProtocolError> {
    if prefix.len() == query.identity_arity {
        return Ok(Some(prefix.to_vec()));
    }
    let Some(child) = store
        .record_first_child(&query.store, prefix)
        .map_err(store_error)?
    else {
        return Ok(None);
    };
    let mut identity = prefix.to_vec();
    identity.push(child);
    while identity.len() < query.identity_arity {
        let Some(child) = store
            .record_first_child(&query.store, &identity)
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        identity.push(child);
    }
    Ok(Some(identity))
}

fn next_identity_after(
    store: &TreeStore,
    query: &DataQuery,
    identity: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, ProtocolError> {
    for level in (query.identity.len()..identity.len()).rev() {
        let prefix = &identity[..level];
        let anchor = &identity[level];
        if let Some(next) = store
            .record_next_child(&query.store, prefix, anchor)
            .map_err(store_error)?
        {
            let mut candidate = prefix.to_vec();
            candidate.push(next);
            return first_identity_under(store, query, &candidate);
        }
    }
    Ok(None)
}

fn encode_cursor(path: &[DataQuerySegment]) -> String {
    base64::encode(
        json!({ "v": 1, "path": encode_query_path(path) })
            .to_string()
            .as_bytes(),
    )
}

fn encode_query_path(path: &[DataQuerySegment]) -> Value {
    Value::Array(path.iter().map(encode_query_segment).collect())
}

fn encode_query_segment(segment: &DataQuerySegment) -> Value {
    match segment {
        DataQuerySegment::Root(name) => json!({ "root": name }),
        DataQuerySegment::Field(name) | DataQuerySegment::SourceMember(name) => {
            json!({ "field": name })
        }
        DataQuerySegment::Layer(name) => json!({ "layer": name }),
        DataQuerySegment::Key(key) => json!({ "key": encode_key(key) }),
    }
}

fn decode_cursor(
    program: &CheckedProgram,
    value: &Value,
    prefix: &DataQuery,
) -> Result<DataQuery, ProtocolError> {
    let cursor = decode_base64_field(value, "cursor")?;
    let cursor =
        String::from_utf8(cursor).map_err(|_| bad_request("`cursor` is not a checked path"))?;
    let cursor: Value = serde_json::from_str(&cursor)
        .map_err(|_| bad_request("`cursor` is not a data_walk cursor"))?;
    let object = cursor
        .as_object()
        .ok_or_else(|| bad_request("`cursor` is not a data_walk cursor"))?;
    if object.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(bad_request("`cursor` is not a data_walk cursor"));
    }
    let path = object
        .get("path")
        .ok_or_else(|| bad_request("`cursor` is not a data_walk cursor"))?;
    let segments = decode_query_path(path)?;
    let query = resolve_data_query(program, &segments).map_err(|message| bad_request(&message))?;
    if !query_under_prefix(&query, prefix) {
        return Err(bad_request("`cursor` is outside the requested path"));
    }
    Ok(query)
}

fn query_under_prefix(query: &DataQuery, prefix: &DataQuery) -> bool {
    if query.store != prefix.store || query.identity_arity != prefix.identity_arity {
        return false;
    }
    if !query.identity.starts_with(&prefix.identity) {
        return false;
    }
    if prefix.identity.len() < prefix.identity_arity {
        return true;
    }
    query.identity.len() == prefix.identity_arity && query.data_path.starts_with(&prefix.data_path)
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
