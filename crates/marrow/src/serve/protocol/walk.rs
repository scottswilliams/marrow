use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use serde_json::{Value, json};

use crate::cmd_data::get::{DataQuery, DataQuerySegment, render_query_segments};
use crate::cmd_data::inspect::checked_catalog_id;

use super::codec::request_query;
use super::cursor::{CursorState, query_under_prefix};
use super::{ProtocolError, bad_request, store_error};

const MAX_WALK: usize = 10_000;

pub(super) fn op_data_walk(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let query = request_query(program, request)?;
    let limit = request_walk_limit(request)?;
    let cursor = request
        .get("cursor")
        .map(|value| cursors.decode_cursor(program, value, &query))
        .transpose()?;
    let page = checked_walk(program, store, &query, cursor.as_ref(), limit, cursors)?;
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
    cursors: &CursorState,
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
    if cursor.is_some() && !cursor_names_value_path(&place.root_members, &start.data_path)? {
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
        if state.next_cursor_path.is_some() {
            break;
        }
        identity = next_identity_after(store, query, &current)?;
    }
    Ok(WalkPage {
        entries: state.entries,
        truncated: state.next_cursor_path.is_some(),
        next_cursor: state
            .next_cursor_path
            .as_deref()
            .map(|path| cursors.encode_cursor(&query.path, path)),
    })
}

fn cursor_names_value_path(
    members: &[CheckedSavedMember],
    data_path: &[DataPathSegment],
) -> Result<bool, ProtocolError> {
    let Some(DataPathSegment::Member(catalog)) = data_path.first() else {
        return Ok(false);
    };
    let Some(member) = checked_member_by_catalog(members, catalog)? else {
        return Ok(false);
    };
    let mut rest = &data_path[1..];
    for _ in &member.key_params {
        let Some(DataPathSegment::Key(_)) = rest.first() else {
            return Ok(false);
        };
        rest = &rest[1..];
    }
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => Ok(rest.is_empty()),
        CheckedSavedMemberKind::Group => {
            if rest.is_empty() {
                return Ok(false);
            }
            cursor_names_value_path(&member.group_members, rest)
        }
    }
}

fn checked_member_by_catalog<'a>(
    members: &'a [CheckedSavedMember],
    catalog: &CatalogId,
) -> Result<Option<&'a CheckedSavedMember>, ProtocolError> {
    for member in members {
        let member_catalog =
            checked_catalog_id(&member.catalog_id, "resource member").map_err(store_error)?;
        if &member_catalog == catalog {
            return Ok(Some(member));
        }
    }
    Ok(None)
}

struct WalkState {
    entries: Vec<Value>,
    limit: usize,
    next_cursor_path: Option<Vec<DataQuerySegment>>,
}

impl WalkState {
    fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit,
            next_cursor_path: None,
        }
    }

    fn push(&mut self, path: &[DataQuerySegment], value: Vec<u8>) {
        if self.entries.len() == self.limit {
            self.next_cursor_path = Some(path.to_vec());
            return;
        }
        self.entries.push(json!({
            "path": render_query_segments(path),
            "value": marrow_run::base64::encode(&value),
        }));
    }
}

struct WalkMembers<'a, 'b> {
    store: &'a TreeStore,
    store_id: &'a CatalogId,
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
        if walk.state.next_cursor_path.is_some() {
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
            && walk.state.next_cursor_path.is_none()
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
        if walk.state.next_cursor_path.is_some() {
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
        if walk.state.next_cursor_path.is_some() {
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
