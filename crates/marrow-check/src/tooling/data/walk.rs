use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment as StoreDataPathSegment, TreeStore};

use crate::tooling::ToolingError;
use crate::{CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place};

use super::path::data_path_under_prefix;
use super::path_error::DataPathError;
use super::record_nav;
use super::render::render_data_path_segments;
use super::shape::{
    cursor_names_value_path, data_path_segment_for_member, path_can_match, stored_key_mismatch,
    tooling_catalog_id,
};
use super::{
    DataEntry, DataPathSegment, DataWalkPage, DebugDataCursorPath, DebugDataPayload,
    ResolvedDataPath,
};

pub fn walk_data(
    program: &CheckedProgram,
    store: &TreeStore,
    resolved: &ResolvedDataPath,
    cursor: Option<&ResolvedDataPath>,
    limit: usize,
) -> Result<DataWalkPage, ToolingError> {
    if limit == 0 {
        return Err(DataPathError::ZeroLimit.into());
    }
    let place = checked_saved_root_place(
        program,
        resolved.root(),
        marrow_syntax::SourceSpan::default(),
    )
    .ok_or_else(|| DataPathError::UnknownRoot {
        root: resolved.root().to_string(),
    })?;
    let mut state = WalkState::new(limit);
    let start = cursor.unwrap_or(resolved);
    if !data_path_under_prefix(start, resolved) {
        return Err(DataPathError::CursorOutsidePath.into());
    }
    if cursor.is_some() && start.storage.identity.len() != resolved.storage.identity_arity {
        return Err(DataPathError::CursorNotAPosition.into());
    }
    if cursor.is_some() && !cursor_names_value_path(&place.root_members, &start.storage.data_path) {
        return Err(DataPathError::CursorNotAPosition.into());
    }

    let mut identity = if cursor.is_some() {
        Some(start.storage.identity.clone())
    } else {
        first_identity_under(store, resolved, &resolved.storage.identity)?
    };
    let mut cursor_pending = cursor.map(|cursor| cursor.storage.data_path.clone());
    while let Some(current) = identity {
        if !current.starts_with(&resolved.storage.identity) {
            break;
        }
        let mut rendered_path = Vec::with_capacity(1 + current.len());
        rendered_path.push(DataPathSegment::Root(resolved.root().to_string()));
        rendered_path.extend(current.iter().cloned().map(DataPathSegment::Key));
        let start_path = cursor_pending.take();
        let mut waiting_for_cursor = start_path.is_some();
        walk_members(
            &mut WalkMembers {
                store,
                store_id: &resolved.storage.store,
                identity: &current,
                filter_prefix: &resolved.storage.data_path,
                start_path: start_path.as_deref(),
                waiting_for_cursor: &mut waiting_for_cursor,
                state: &mut state,
            },
            &place.root_members,
            &mut Vec::new(),
            &mut rendered_path,
        )?;
        if waiting_for_cursor {
            return Err(DataPathError::CursorNotAnEntry.into());
        }
        if state.next_cursor_path.is_some() {
            break;
        }
        identity = next_identity_after(store, resolved, &current)?;
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

    fn push(&mut self, path: &[DataPathSegment], value: Vec<u8>) {
        if self.entries.len() == self.limit {
            self.next_cursor_path = Some(DebugDataCursorPath::new(path.to_vec()));
            return;
        }
        self.entries.push(DataEntry {
            path: render_data_path_segments(path),
            segments: path.to_vec(),
            payload: DebugDataPayload::new(value),
        });
    }
}

struct WalkMembers<'a, 'b> {
    store: &'a TreeStore,
    store_id: &'a CatalogId,
    identity: &'a [SavedKey],
    filter_prefix: &'a [StoreDataPathSegment],
    start_path: Option<&'a [StoreDataPathSegment]>,
    waiting_for_cursor: &'b mut bool,
    state: &'b mut WalkState,
}

fn walk_members(
    walk: &mut WalkMembers<'_, '_>,
    members: &[CheckedSavedMember],
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
) -> Result<(), ToolingError> {
    for member in members {
        if walk.state.next_cursor_path.is_some() {
            break;
        }
        walk_member(walk, member, data_path, path)?;
    }
    Ok(())
}

fn walk_member(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
) -> Result<(), ToolingError> {
    let Some(catalog) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
        return Ok(());
    };
    data_path.push(StoreDataPathSegment::Member(catalog));
    path.push(data_path_segment_for_member(member));
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
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
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
    let first = walk
        .store
        .data_first_child(walk.store_id, walk.identity, data_path)?;
    walk_member_key_run(walk, member, data_path, path, key_index, first)
}

/// Enumerates stored child keys for one key level starting from `child`, walking each and
/// resuming via `data_next_child` until exhausted or the page limit fills `next_cursor_path`.
fn walk_member_key_run(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
    key_index: usize,
    mut child: Option<SavedKey>,
) -> Result<(), ToolingError> {
    while let Some(key) = child {
        let anchor = key.clone();
        stored_key_mismatch(member.key_params[key_index].scalar, &key)?;
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

fn selected_key(
    walk: &WalkMembers<'_, '_>,
    data_path: &[StoreDataPathSegment],
) -> Option<SelectedKey> {
    let next_segment = data_path.len();
    if let Some(StoreDataPathSegment::Key(key)) = walk.filter_prefix.get(next_segment) {
        return Some(SelectedKey::Filter(key.clone()));
    }
    if !*walk.waiting_for_cursor {
        return None;
    }
    let start_path = walk.start_path?;
    match start_path.get(next_segment) {
        Some(StoreDataPathSegment::Key(key)) => Some(SelectedKey::Cursor(key.clone())),
        _ => None,
    }
}

fn walk_member_key(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
    key_index: usize,
    key: SavedKey,
) -> Result<(), ToolingError> {
    data_path.push(StoreDataPathSegment::Key(key.clone()));
    path.push(DataPathSegment::Key(key));
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
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
    key_index: usize,
    anchor: &SavedKey,
) -> Result<(), ToolingError> {
    let first = walk
        .store
        .data_next_child(walk.store_id, walk.identity, data_path, anchor)?;
    walk_member_key_run(walk, member, data_path, path, key_index, first)
}

fn walk_member_terminal(
    walk: &mut WalkMembers<'_, '_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<StoreDataPathSegment>,
    path: &mut Vec<DataPathSegment>,
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
        CheckedSavedMemberKind::Group => {
            walk_members(walk, &member.group_members, data_path, path)?;
        }
    }
    Ok(())
}

fn first_identity_under(
    store: &TreeStore,
    path: &ResolvedDataPath,
    prefix: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, ToolingError> {
    if prefix.len() == path.storage.identity_arity {
        return Ok(Some(prefix.to_vec()));
    }
    let Some(child) = record_nav::first_record_child(
        store,
        &path.storage.store,
        prefix,
        path.storage.identity_arity,
    )?
    else {
        return Ok(None);
    };
    stored_key_mismatch(path.storage.identity_key_scalars[prefix.len()], &child)?;
    let mut identity = prefix.to_vec();
    identity.push(child);
    while identity.len() < path.storage.identity_arity {
        let Some(child) = record_nav::first_record_child(
            store,
            &path.storage.store,
            &identity,
            path.storage.identity_arity,
        )?
        else {
            return Ok(None);
        };
        stored_key_mismatch(path.storage.identity_key_scalars[identity.len()], &child)?;
        identity.push(child);
    }
    Ok(Some(identity))
}

fn next_identity_after(
    store: &TreeStore,
    path: &ResolvedDataPath,
    identity: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, ToolingError> {
    for level in (path.storage.identity.len()..identity.len()).rev() {
        let prefix = &identity[..level];
        let anchor = &identity[level];
        if let Some(next) = record_nav::next_record_child(
            store,
            &path.storage.store,
            prefix,
            path.storage.identity_arity,
            anchor,
        )? {
            stored_key_mismatch(path.storage.identity_key_scalars[level], &next)?;
            let mut candidate = prefix.to_vec();
            candidate.push(next);
            return first_identity_under(store, path, &candidate);
        }
    }
    Ok(None)
}
