use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::tooling::ToolingError;
use crate::{CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place};

use super::query::data_query_under_prefix;
use super::query_error::QueryError;
use super::record_nav;
use super::render::render_query_segments;
use super::shape::{
    cursor_names_value_path, path_can_match, query_segment_for_member, tooling_catalog_id,
};
use super::{
    DataEntry, DataQuery, DataQuerySegment, DataWalkPage, DebugDataCursorPath, DebugDataPayload,
};

pub fn walk_data(
    program: &CheckedProgram,
    store: &TreeStore,
    query: &DataQuery,
    cursor: Option<&DataQuery>,
    limit: usize,
) -> Result<DataWalkPage, ToolingError> {
    if limit == 0 {
        return Err(QueryError::ZeroLimit.into());
    }
    let place =
        checked_saved_root_place(program, query.root(), marrow_syntax::SourceSpan::default())
            .ok_or_else(|| QueryError::UnknownRoot {
                root: query.root().to_string(),
            })?;
    let mut state = WalkState::new(limit);
    let start = cursor.unwrap_or(query);
    if !data_query_under_prefix(start, query) {
        return Err(QueryError::CursorOutsidePath.into());
    }
    if cursor.is_some() && start.storage.identity.len() != query.storage.identity_arity {
        return Err(QueryError::CursorNotAPosition.into());
    }
    if cursor.is_some() && !cursor_names_value_path(&place.root_members, &start.storage.data_path) {
        return Err(QueryError::CursorNotAPosition.into());
    }

    let mut identity = if cursor.is_some() {
        Some(start.storage.identity.clone())
    } else {
        first_identity_under(store, query, &query.storage.identity)?
    };
    let mut cursor_pending = cursor.map(|cursor| cursor.storage.data_path.clone());
    while let Some(current) = identity {
        if !current.starts_with(&query.storage.identity) {
            break;
        }
        let mut path = Vec::with_capacity(1 + current.len());
        path.push(DataQuerySegment::Root(query.root().to_string()));
        path.extend(current.iter().cloned().map(DataQuerySegment::Key));
        let start_path = cursor_pending.take();
        let mut waiting_for_cursor = start_path.is_some();
        walk_members(
            WalkMembers {
                store,
                store_id: &query.storage.store,
                identity: &current,
                filter_prefix: &query.storage.data_path,
                start_path: start_path.as_deref(),
                waiting_for_cursor: &mut waiting_for_cursor,
                state: &mut state,
            },
            &place.root_members,
            &mut Vec::new(),
            &mut path,
        )?;
        if waiting_for_cursor {
            return Err(QueryError::CursorNotAnEntry.into());
        }
        if state.next_cursor_path.is_some() {
            break;
        }
        identity = next_identity_after(store, query, &current)?;
    }
    Ok(DataWalkPage {
        entries: state.entries,
        truncated: state.next_cursor_path.is_some(),
        next_cursor_path: state.next_cursor_path,
    })
}

struct WalkState {
    entries: Vec<DataEntry>,
    limit: usize,
    next_cursor_path: Option<DebugDataCursorPath>,
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
            self.next_cursor_path = Some(DebugDataCursorPath::new(path.to_vec()));
            return;
        }
        self.entries.push(DataEntry {
            path: render_query_segments(path),
            segments: path.to_vec(),
            payload: DebugDataPayload::new(value),
        });
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
) -> Result<(), ToolingError> {
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
) -> Result<(), ToolingError> {
    let Some(catalog) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
        return Ok(());
    };
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

fn walk_member_keys(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
    key_index: usize,
) -> Result<(), ToolingError> {
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
        .data_first_child(walk.store_id, walk.identity, data_path)?;
    while let Some(key) = child {
        let anchor = key.clone();
        walk_member_key(walk, member, data_path, path, key_index, key)?;
        if walk.state.next_cursor_path.is_some() {
            break;
        }
        child = walk
            .store
            .data_next_child(walk.store_id, walk.identity, data_path, &anchor)?;
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
) -> Result<(), ToolingError> {
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
) -> Result<(), ToolingError> {
    let mut child = walk
        .store
        .data_next_child(walk.store_id, walk.identity, data_path, anchor)?;
    while let Some(key) = child {
        let anchor = key.clone();
        walk_member_key(walk, member, data_path, path, key_index, key)?;
        if walk.state.next_cursor_path.is_some() {
            break;
        }
        child = walk
            .store
            .data_next_child(walk.store_id, walk.identity, data_path, &anchor)?;
    }
    Ok(())
}

fn walk_member_terminal(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut Vec<DataQuerySegment>,
) -> Result<(), ToolingError> {
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => {
            if !data_path.starts_with(walk.filter_prefix) {
                return Ok(());
            }
            let waiting_for_this_path = *walk.waiting_for_cursor;
            if waiting_for_this_path && Some(data_path.as_slice()) != walk.start_path {
                return Ok(());
            }
            if let Some(value) =
                walk.store
                    .read_data_value(walk.store_id, walk.identity, data_path)?
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

fn first_identity_under(
    store: &TreeStore,
    query: &DataQuery,
    prefix: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, ToolingError> {
    if prefix.len() == query.storage.identity_arity {
        return Ok(Some(prefix.to_vec()));
    }
    let Some(child) = record_nav::first_record_child(
        store,
        &query.storage.store,
        prefix,
        query.storage.identity_arity,
    )?
    else {
        return Ok(None);
    };
    let mut identity = prefix.to_vec();
    identity.push(child);
    while identity.len() < query.storage.identity_arity {
        let Some(child) = record_nav::first_record_child(
            store,
            &query.storage.store,
            &identity,
            query.storage.identity_arity,
        )?
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
) -> Result<Option<Vec<SavedKey>>, ToolingError> {
    for level in (query.storage.identity.len()..identity.len()).rev() {
        let prefix = &identity[..level];
        let anchor = &identity[level];
        if let Some(next) = record_nav::next_record_child(
            store,
            &query.storage.store,
            prefix,
            query.storage.identity_arity,
            anchor,
        )? {
            let mut candidate = prefix.to_vec();
            candidate.push(next);
            return first_identity_under(store, query, &candidate);
        }
    }
    Ok(None)
}
