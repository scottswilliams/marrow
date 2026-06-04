use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, ScalarType,
    checked_saved_root_place,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{SavedValue, encode_value};

pub(crate) fn checked_catalog_id(
    raw: &str,
    context: &'static str,
) -> Result<CatalogId, StoreError> {
    CatalogId::new(raw.to_string()).map_err(|_| StoreError::Corruption {
        message: format!("checked {context} catalog id is malformed"),
    })
}

pub(crate) fn data_roots_in_store(
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

pub(crate) fn count_data_records(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<usize, StoreError> {
    visit_data_records(program, store, |_| Ok(()))
}

pub(crate) fn visit_data_records(
    program: &CheckedProgram,
    store: &TreeStore,
    mut visit: impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    let mut records = 0usize;
    for place in checked_places(program) {
        records = records
            .checked_add(visit_place_records(&place, store, &mut visit)?)
            .ok_or(StoreError::LimitExceeded {
                limit: "data record count",
            })?;
    }
    Ok(records)
}

fn checked_places(program: &CheckedProgram) -> Vec<CheckedSavedPlace> {
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
    let store_id = checked_catalog_id(&place.store_catalog_id, "store")?;
    if place.identity_keys.is_empty() {
        return store.data_subtree_exists(&store_id, &[], &[]);
    }
    store
        .record_first_child(&store_id, &[])
        .map(|key| key.is_some())
}

pub(crate) struct DataRecord {
    pub(crate) path: String,
    pub(crate) value: Vec<u8>,
    pub(crate) leaf: marrow_check::StoreLeafKind,
    pub(crate) key_mismatch: Option<KeyMismatch>,
}

#[derive(Debug, Clone)]
pub(crate) struct KeyMismatch {
    pub(crate) expected: ScalarType,
    pub(crate) found: ScalarType,
}

fn visit_place_records(
    place: &CheckedSavedPlace,
    store: &TreeStore,
    visit: &mut impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    let store_id = checked_catalog_id(&place.store_catalog_id, "store")?;
    let mut identity = Vec::new();
    let mut path = format!("^{}", place.root);
    visit_identity_records(
        place,
        &store_id,
        store,
        &mut identity,
        &mut path,
        None,
        visit,
    )
}

fn visit_identity_records(
    place: &CheckedSavedPlace,
    store_id: &CatalogId,
    store: &TreeStore,
    identity: &mut Vec<SavedKey>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    if identity.len() == place.identity_keys.len() {
        let mut data_path = Vec::new();
        let context = MemberVisit {
            store_id,
            store,
            identity,
        };
        return visit_members(
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
    let mut child = store.record_first_child(store_id, identity)?;
    while let Some(key) = child {
        let next_after = key.clone();
        let prior_len = push_key(path, &key);
        let next_mismatch = mismatch
            .clone()
            .or_else(|| key_mismatch(place.identity_keys[key_index].scalar, &key));
        identity.push(key);
        records = records
            .checked_add(visit_identity_records(
                place,
                store_id,
                store,
                identity,
                path,
                next_mismatch,
                visit,
            )?)
            .ok_or(StoreError::LimitExceeded {
                limit: "data record count",
            })?;
        identity.pop();
        path.truncate(prior_len);
        child = store.record_next_child(store_id, identity, &next_after)?;
    }
    Ok(records)
}

struct MemberVisit<'a> {
    store_id: &'a CatalogId,
    store: &'a TreeStore,
    identity: &'a [SavedKey],
}

fn visit_members(
    context: &MemberVisit<'_>,
    members: &[CheckedSavedMember],
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    let mut records = 0usize;
    for member in members {
        records = records
            .checked_add(visit_member(
                context,
                member,
                data_path,
                path,
                mismatch.clone(),
                visit,
            )?)
            .ok_or(StoreError::LimitExceeded {
                limit: "data record count",
            })?;
    }
    Ok(records)
}

fn visit_member(
    context: &MemberVisit<'_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    let catalog_id = checked_catalog_id(&member.catalog_id, "resource member")?;
    let prior_len = push_member(path, &member.name);
    data_path.push(DataPathSegment::Member(catalog_id));
    let records = if member.key_params.is_empty() {
        visit_member_terminal(context, member, data_path, path, mismatch, visit)
    } else {
        visit_member_keys(context, member, data_path, path, 0, mismatch, visit)
    };
    data_path.pop();
    path.truncate(prior_len);
    records
}

fn visit_member_keys(
    context: &MemberVisit<'_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    key_index: usize,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    if key_index == member.key_params.len() {
        return visit_member_terminal(context, member, data_path, path, mismatch, visit);
    }

    let mut records = 0usize;
    let mut child =
        context
            .store
            .data_first_child(context.store_id, context.identity, data_path)?;
    while let Some(key) = child {
        let next_after = key.clone();
        let prior_len = push_key(path, &key);
        let next_mismatch = mismatch
            .clone()
            .or_else(|| key_mismatch(member.key_params[key_index].scalar, &key));
        data_path.push(DataPathSegment::Key(key));
        records = records
            .checked_add(visit_member_keys(
                context,
                member,
                data_path,
                path,
                key_index + 1,
                next_mismatch,
                visit,
            )?)
            .ok_or(StoreError::LimitExceeded {
                limit: "data record count",
            })?;
        data_path.pop();
        path.truncate(prior_len);
        child = context.store.data_next_child(
            context.store_id,
            context.identity,
            data_path,
            &next_after,
        )?;
    }
    Ok(records)
}

fn visit_member_terminal(
    context: &MemberVisit<'_>,
    member: &CheckedSavedMember,
    data_path: &mut Vec<DataPathSegment>,
    path: &mut String,
    mismatch: Option<KeyMismatch>,
    visit: &mut impl FnMut(DataRecord) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    match &member.kind {
        CheckedSavedMemberKind::Field { .. } => {
            let Some(leaf) = member.leaf.clone() else {
                return Ok(0);
            };
            let Some(value) =
                context
                    .store
                    .read_data_value(context.store_id, context.identity, data_path)?
            else {
                return Ok(0);
            };
            visit(DataRecord {
                path: path.clone(),
                value,
                leaf,
                key_mismatch: mismatch,
            })?;
            Ok(1)
        }
        CheckedSavedMemberKind::Group => visit_members(
            context,
            &member.group_members,
            data_path,
            path,
            mismatch,
            visit,
        ),
    }
}

pub(crate) fn key_mismatch(expected: Option<ScalarType>, key: &SavedKey) -> Option<KeyMismatch> {
    let expected = expected?;
    let found = key.scalar_type();
    (expected != found).then_some(KeyMismatch { expected, found })
}

fn push_member(path: &mut String, name: &str) -> usize {
    let prior_len = path.len();
    path.push('.');
    path.push_str(name);
    prior_len
}

pub(crate) fn push_key(path: &mut String, key: &SavedKey) -> usize {
    let prior_len = path.len();
    path.push('(');
    path.push_str(&render_key(key));
    path.push(')');
    prior_len
}

fn render_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Bytes(value) => {
            let mut text = String::from("0x");
            crate::push_hex(&mut text, value);
            text
        }
        SavedKey::Date(value) => render_key_temporal(SavedValue::Date(*value)),
        SavedKey::Instant(value) => render_key_temporal(SavedValue::Instant(*value)),
        SavedKey::Duration(value) => render_key_temporal(SavedValue::Duration(*value)),
    }
}

fn render_key_temporal(value: SavedValue) -> String {
    encode_value(&value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| format!("{value:?}"))
}

pub(crate) fn render_value_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let mut text = String::from("0x");
            crate::push_hex(&mut text, bytes);
            text
        }
    }
}
