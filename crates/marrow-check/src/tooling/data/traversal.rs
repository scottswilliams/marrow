use std::ops::ControlFlow;

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    checked_saved_root_place,
};

use super::record_nav;
use super::render::{push_key, push_member};
use super::shape::{key_mismatch, tooling_catalog_id};
use super::{DataRecord, DebugDataPayload, KeyMismatch};

pub fn data_roots_in_store(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<Vec<String>, StoreError> {
    let mut roots = Vec::new();
    for place in checked_places(program) {
        if place_has_data(&place, store)? {
            roots.push(place.root);
        }
    }
    Ok(roots)
}

pub fn count_data_records(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<usize, StoreError> {
    visit_data_records(program, store, |_| Ok(()))
}

pub fn visit_data_records(
    program: &CheckedProgram,
    store: &TreeStore,
    mut visit: impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    visit_data_records_in_places(&checked_places(program), store, &mut visit)
}

pub(crate) fn visit_data_records_in_places(
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    mut visit: impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    match visit_data_records_in_places_until(places, store, |record| {
        visit(record)?;
        Ok(ControlFlow::Continue(()))
    })? {
        ControlFlow::Continue(records) | ControlFlow::Break(records) => Ok(records),
    }
}

pub(crate) fn visit_data_records_in_places_until(
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    mut visit: impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    let mut records = 0usize;
    for place in places {
        match visit_place_records_until(place, store, &mut visit)? {
            ControlFlow::Continue(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
            }
            ControlFlow::Break(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
                return Ok(ControlFlow::Break(records));
            }
        }
    }
    Ok(ControlFlow::Continue(records))
}

pub(crate) fn visit_place_record_identities_until(
    place: &CheckedSavedPlace,
    store: &TreeStore,
    visit: &mut impl FnMut(
        &CheckedSavedPlace,
        &CatalogId,
        &[SavedKey],
    ) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    let Some(store_id) = tooling_catalog_id(&place.store_catalog_id, "store")? else {
        return Ok(ControlFlow::Continue(0));
    };
    let mut identity = Vec::new();
    visit_identity_record_nodes_until(place, &store_id, store, &mut identity, visit)
}

pub(crate) fn checked_places(program: &CheckedProgram) -> Vec<CheckedSavedPlace> {
    program
        .facts
        .stores()
        .iter()
        .filter_map(|store| {
            checked_saved_root_place(program, &store.root, marrow_syntax::SourceSpan::default())
        })
        .collect()
}

fn place_has_data(place: &CheckedSavedPlace, store: &TreeStore) -> Result<bool, StoreError> {
    let Some(store_id) = tooling_catalog_id(&place.store_catalog_id, "store")? else {
        return Ok(false);
    };
    if place.identity_keys.is_empty() {
        return store.data_subtree_exists(&store_id, &[], &[]);
    }
    record_nav::first_record_child(store, &store_id, &[], place.identity_keys.len())
        .map(|key| key.is_some())
}

fn visit_place_records_until(
    place: &CheckedSavedPlace,
    store: &TreeStore,
    visit: &mut impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    let Some(store_id) = tooling_catalog_id(&place.store_catalog_id, "store")? else {
        return Ok(ControlFlow::Continue(0));
    };
    let mut identity = Vec::new();
    let mut path = format!("^{}", place.root);
    visit_identity_records_until(
        place,
        &store_id,
        store,
        &mut identity,
        &mut path,
        None,
        visit,
    )
}

fn visit_identity_records_until(
    place: &CheckedSavedPlace,
    store_id: &CatalogId,
    store: &TreeStore,
    identity: &mut Vec<SavedKey>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    if identity.len() == place.identity_keys.len() {
        let mut data_path = Vec::new();
        let context = MemberVisit {
            store_id,
            store,
            identity,
        };
        return visit_members_until(
            &context,
            &place.root_members,
            &mut data_path,
            path,
            mismatch,
            visit,
        );
    }

    let key_index = identity.len();
    let mut records = 0usize;
    let mut child =
        record_nav::first_record_child(store, store_id, identity, place.identity_keys.len())?;
    while let Some(key) = child {
        let next_after = key.clone();
        let prior_len = push_key(path, &key);
        let next_mismatch = mismatch
            .clone()
            .or_else(|| key_mismatch(place.identity_keys[key_index].scalar, &key));
        identity.push(key);
        match visit_identity_records_until(
            place,
            store_id,
            store,
            identity,
            path,
            next_mismatch,
            visit,
        )? {
            ControlFlow::Continue(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
            }
            ControlFlow::Break(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
                identity.pop();
                path.truncate(prior_len);
                return Ok(ControlFlow::Break(records));
            }
        }
        identity.pop();
        path.truncate(prior_len);
        child = record_nav::next_record_child(
            store,
            store_id,
            identity,
            place.identity_keys.len(),
            &next_after,
        )?;
    }
    Ok(ControlFlow::Continue(records))
}

fn visit_identity_record_nodes_until(
    place: &CheckedSavedPlace,
    store_id: &CatalogId,
    store: &TreeStore,
    identity: &mut Vec<SavedKey>,
    visit: &mut impl FnMut(
        &CheckedSavedPlace,
        &CatalogId,
        &[SavedKey],
    ) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    if identity.len() == place.identity_keys.len() {
        if !store.data_subtree_exists(store_id, identity, &[])? {
            return Ok(ControlFlow::Continue(0));
        }
        return match visit(place, store_id, identity)? {
            ControlFlow::Continue(()) => Ok(ControlFlow::Continue(1)),
            ControlFlow::Break(()) => Ok(ControlFlow::Break(0)),
        };
    }

    let mut records = 0usize;
    let mut child =
        record_nav::first_record_child(store, store_id, identity, place.identity_keys.len())?;
    while let Some(key) = child {
        let next_after = key.clone();
        identity.push(key);
        match visit_identity_record_nodes_until(place, store_id, store, identity, visit)? {
            ControlFlow::Continue(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
            }
            ControlFlow::Break(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
                identity.pop();
                return Ok(ControlFlow::Break(records));
            }
        }
        identity.pop();
        child = record_nav::next_record_child(
            store,
            store_id,
            identity,
            place.identity_keys.len(),
            &next_after,
        )?;
    }
    Ok(ControlFlow::Continue(records))
}

struct MemberVisit<'a> {
    store_id: &'a CatalogId,
    store: &'a TreeStore,
    identity: &'a [SavedKey],
}

fn visit_members_until(
    context: &MemberVisit<'_>,
    members: &[CheckedSavedMember],
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    let mut records = 0usize;
    for member in members {
        match visit_member_until(context, member, data_path, path, mismatch.clone(), visit)? {
            ControlFlow::Continue(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
            }
            ControlFlow::Break(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
                return Ok(ControlFlow::Break(records));
            }
        }
    }
    Ok(ControlFlow::Continue(records))
}

fn visit_member_until(
    context: &MemberVisit<'_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    let Some(catalog_id) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
        return Ok(ControlFlow::Continue(0));
    };
    let prior_len = push_member(path, &member.name);
    data_path.push(DataPathSegment::Member(catalog_id.clone()));
    let cursor = MemberCursor {
        context: MemberVisit {
            store_id: context.store_id,
            store: context.store,
            identity: context.identity,
        },
        member,
        field_catalog_id: &catalog_id,
    };
    let records = if member.key_params.is_empty() {
        visit_member_terminal_until(&cursor, data_path, path, mismatch, visit)
    } else {
        visit_member_keys_until(&cursor, data_path, path, 0, mismatch, visit)
    };
    data_path.pop();
    path.truncate(prior_len);
    records
}

struct MemberCursor<'a> {
    context: MemberVisit<'a>,
    member: &'a CheckedSavedMember,
    field_catalog_id: &'a CatalogId,
}

fn visit_member_keys_until(
    cursor: &MemberCursor<'_>,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    key_index: usize,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    if key_index == cursor.member.key_params.len() {
        return visit_member_terminal_until(cursor, data_path, path, mismatch, visit);
    }

    let mut records = 0usize;
    let mut child = cursor.context.store.data_first_child(
        cursor.context.store_id,
        cursor.context.identity,
        data_path,
    )?;
    while let Some(key) = child {
        let next_after = key.clone();
        let prior_len = push_key(path, &key);
        let next_mismatch = mismatch
            .clone()
            .or_else(|| key_mismatch(cursor.member.key_params[key_index].scalar, &key));
        data_path.push(DataPathSegment::Key(key));
        match visit_member_keys_until(cursor, data_path, path, key_index + 1, next_mismatch, visit)?
        {
            ControlFlow::Continue(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
            }
            ControlFlow::Break(count) => {
                records = records
                    .checked_add(count)
                    .ok_or(StoreError::LimitExceeded {
                        limit: "data record count",
                    })?;
                data_path.pop();
                path.truncate(prior_len);
                return Ok(ControlFlow::Break(records));
            }
        }
        data_path.pop();
        path.truncate(prior_len);
        child = cursor.context.store.data_next_child(
            cursor.context.store_id,
            cursor.context.identity,
            data_path,
            &next_after,
        )?;
    }
    Ok(ControlFlow::Continue(records))
}

fn visit_member_terminal_until(
    cursor: &MemberCursor<'_>,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<ControlFlow<()>, StoreError>,
) -> Result<ControlFlow<usize, usize>, StoreError> {
    match &cursor.member.kind {
        CheckedSavedMemberKind::Field { .. } => {
            let Some(leaf) = cursor.member.leaf.clone() else {
                return Ok(ControlFlow::Continue(0));
            };
            let Some(value) = cursor.context.store.read_data_value(
                cursor.context.store_id,
                cursor.context.identity,
                data_path,
            )?
            else {
                return Ok(ControlFlow::Continue(0));
            };
            match visit(DataRecord {
                path: path.clone(),
                payload: DebugDataPayload::new(value),
                identity: cursor.context.identity.to_vec(),
                field_catalog_id: cursor.field_catalog_id.clone(),
                leaf,
                key_mismatch: mismatch,
            })? {
                ControlFlow::Continue(()) => Ok(ControlFlow::Continue(1)),
                ControlFlow::Break(()) => Ok(ControlFlow::Break(0)),
            }
        }
        CheckedSavedMemberKind::Group => visit_members_until(
            &cursor.context,
            &cursor.member.group_members,
            data_path,
            path,
            mismatch,
            visit,
        ),
    }
}
