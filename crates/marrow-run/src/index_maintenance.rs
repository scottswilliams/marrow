//! Generated-index maintenance for managed resource writes.

use marrow_check::{
    CheckedFacts, CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedMember, CheckedSavedPlace,
    ResourceMemberId, StoreIndexKeySource, StoredValueMeaning,
};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::store::{DataAddress, IndexAddress, read_data};
use crate::value::{
    LeafValue, diagnostic_saved_key_preview, stored_enum_member_path, stored_identity_referent_path,
};
use crate::write::{
    ResourceValue, WRITE_INVALID_DATA, WRITE_STORE, WRITE_UNIQUE_CONFLICT, WriteError,
};
use crate::write_plan::PlanStep;

const INDEX_MARKER: &[u8] = b"1";

#[derive(Clone, Copy)]
pub(crate) struct IndexWriteContext<'a> {
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    store: &'a TreeStore,
    /// Checked facts for the run, used to name an enum key read back from the
    /// store when a unique-conflict diagnostic renders the conflicting tuple.
    facts: &'a CheckedFacts,
    span: SourceSpan,
}

impl<'a> IndexWriteContext<'a> {
    pub(crate) fn new(
        place: &'a CheckedSavedPlace,
        identity: &'a [SavedKey],
        store: &'a TreeStore,
        facts: &'a CheckedFacts,
        span: SourceSpan,
    ) -> Self {
        Self {
            place,
            identity,
            store,
            facts,
            span,
        }
    }

    pub(crate) fn place(&self) -> &'a CheckedSavedPlace {
        self.place
    }

    pub(crate) fn identity(&self) -> &'a [SavedKey] {
        self.identity
    }

    pub(crate) fn span(&self) -> SourceSpan {
        self.span
    }
}

pub(crate) fn reject_resource_unique_conflicts(
    context: IndexWriteContext<'_>,
    value: &ResourceValue,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if index.unique {
            let new_keys = index_keys(&index.keys, context.place, context.identity, value)?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn stage_resource_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    value: &ResourceValue,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
        if let Some(new_keys) = index_keys(&index.keys, context.place, context.identity, value)? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, context.span)?,
                identity: context.identity.to_vec(),
                value: index_entry_value(index.unique, context.identity),
            });
        }
    }
    Ok(())
}

pub(crate) fn index_rebuild_entry(
    index: &CheckedSavedIndex,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    facts: &CheckedFacts,
    span: SourceSpan,
) -> Result<Option<PlanStep>, WriteError> {
    let context = IndexWriteContext::new(place, identity, store, facts, span);
    let Some(keys) = stored_index_keys(&index.keys, context)? else {
        return Ok(None);
    };
    Ok(Some(PlanStep::WriteIndex {
        address: index_address(index, keys, span)?,
        identity: identity.to_vec(),
        value: index_entry_value(index.unique, identity),
    }))
}

pub(crate) fn stage_resource_index_deletes(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if let Some(keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
    }
    Ok(())
}

pub(crate) fn reject_field_unique_conflicts(
    context: IndexWriteContext<'_>,
    field: &str,
    value: &LeafValue,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if index.unique && index.keys.iter().any(|key| key.name == field) {
            let new_keys =
                field_write_index_keys(&index.keys, context, field, FieldIndexValue::Leaf(value))?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn reject_identity_field_unique_conflicts(
    context: IndexWriteContext<'_>,
    field: &str,
    keys: &[SavedKey],
) -> Result<(), WriteError> {
    let bytes = encode_identity_payload(keys);
    for index in &context.place.indexes {
        if index.unique && index.keys.iter().any(|key| key.name == field) {
            let new_keys = field_write_index_keys(
                &index.keys,
                context,
                field,
                FieldIndexValue::IdentityBytes(&bytes),
            )?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn stage_field_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
    value: &LeafValue,
) -> Result<(), WriteError> {
    stage_field_index_rewrites_for_value(steps, context, field, FieldIndexValue::Leaf(value))
}

pub(crate) fn stage_identity_field_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
    keys: &[SavedKey],
) -> Result<(), WriteError> {
    let bytes = encode_identity_payload(keys);
    stage_field_index_rewrites_for_value(
        steps,
        context,
        field,
        FieldIndexValue::IdentityBytes(&bytes),
    )
}

#[derive(Clone)]
pub(crate) struct IndexFieldPatch {
    pub(crate) member: ResourceMemberId,
    pub(crate) value: IndexFieldPatchValue,
}

#[derive(Clone)]
pub(crate) enum IndexFieldPatchValue {
    Leaf(LeafValue),
    IdentityBytes(Vec<u8>),
}

impl IndexFieldPatchValue {
    fn key_for(&self, key: &CheckedSavedIndexKey) -> Result<Option<IndexKey>, WriteError> {
        match self {
            Self::Leaf(value) => Ok(value
                .as_key()
                .map_err(WriteError::from)?
                .map(|saved| IndexKey::from_leaf(saved, value))),
            Self::IdentityBytes(bytes) => stored_index_key(&key.value_meaning, bytes)
                .map(|saved| Some(IndexKey::from_meaning(saved, &key.value_meaning))),
        }
    }
}

pub(crate) fn reject_field_patch_unique_conflicts(
    context: IndexWriteContext<'_>,
    patch: &[IndexFieldPatch],
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if index.unique && index_touches_patch(index, patch) {
            let new_keys = field_patch_index_keys(&index.keys, context, patch)?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn stage_field_patch_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    patch: &[IndexFieldPatch],
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if !index_touches_patch(index, patch) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
        if let Some(new_keys) = field_patch_index_keys(&index.keys, context, patch)? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, context.span)?,
                identity: context.identity.to_vec(),
                value: index_entry_value(index.unique, context.identity),
            });
        }
    }
    Ok(())
}

pub(crate) fn stage_field_index_deletes(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if !index.keys.iter().any(|key| key.name == field) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
    }
    Ok(())
}

/// Write the entry for every index keyed solely by identity components. These
/// indexes mention no resource field, so a field write never lists them, yet
/// they exist whenever the record does. Writing them idempotently on each
/// record-establishing write keeps incremental maintenance in step with a bulk
/// restore rebuild, which populates them from identity alone.
pub(crate) fn stage_identity_only_index_writes(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if index.unique || !index_is_identity_only(index) {
            continue;
        }
        if let Some(keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, keys, context.span)?,
                identity: context.identity.to_vec(),
                value: index_entry_value(index.unique, context.identity),
            });
        }
    }
    Ok(())
}

fn index_is_identity_only(index: &CheckedSavedIndex) -> bool {
    index
        .keys
        .iter()
        .all(|key| matches!(key.source, StoreIndexKeySource::IdentityKey))
}

fn stage_field_index_rewrites_for_value(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
    value: FieldIndexValue<'_>,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if !index.keys.iter().any(|key| key.name == field) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
        if let Some(new_keys) = field_write_index_keys(&index.keys, context, field, value)? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, context.span)?,
                identity: context.identity.to_vec(),
                value: index_entry_value(index.unique, context.identity),
            });
        }
    }
    Ok(())
}

fn index_keys(
    keys: &[CheckedSavedIndexKey],
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    value: &ResourceValue,
) -> Result<Option<Vec<IndexKey>>, WriteError> {
    let mut result = Vec::with_capacity(keys.len());
    for key in keys {
        match key.source {
            StoreIndexKeySource::IdentityKey => {
                let Some(position) = place
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)
                else {
                    return Ok(None);
                };
                result.push(IndexKey::scalar(identity[position].clone()));
            }
            StoreIndexKeySource::ResourceMember(_) => {
                if let Some((_, saved)) = value.fields.iter().find(|(name, _)| name == &key.name) {
                    let Some(saved_key) = saved.as_key()? else {
                        return Ok(None);
                    };
                    result.push(IndexKey::from_leaf(saved_key, saved));
                } else {
                    let Some(supplied) = value
                        .identities
                        .iter()
                        .find(|supplied| supplied.field == key.name)
                    else {
                        return Ok(None);
                    };
                    let bytes = encode_identity_payload(&supplied.keys);
                    let Some(saved_key) = key.value_meaning.stored_key(&bytes) else {
                        return Ok(None);
                    };
                    result.push(IndexKey::from_meaning(saved_key, &key.value_meaning));
                }
            }
        }
    }
    Ok(Some(result))
}

/// One resolved index-entry key segment: the stored [`SavedKey`] the index entry
/// is addressed by, plus a readable rendering for segments whose stored form is
/// opaque — an enum member's canonical path or an identity's referent path. The
/// rendering is carried so a unique-conflict diagnostic names the conflicting
/// value rather than its catalog id or physical encoding, whether the value was
/// freshly written or read back from the store.
struct IndexKey {
    saved: SavedKey,
    display: Option<String>,
}

impl IndexKey {
    fn scalar(saved: SavedKey) -> Self {
        Self {
            saved,
            display: None,
        }
    }

    fn from_leaf(saved: SavedKey, leaf: &LeafValue) -> Self {
        Self {
            saved,
            display: leaf.enum_display_name().map(str::to_string),
        }
    }

    /// A segment whose readable form is known only from its declared meaning: an
    /// identity index key, stored as opaque bytes, renders as its referent path.
    fn from_meaning(saved: SavedKey, meaning: &StoredValueMeaning) -> Self {
        let display = stored_identity_referent_path(meaning, &saved);
        Self { saved, display }
    }

    /// A segment read back from the store, recovering the readable form from
    /// `bytes` when the column is an enum, or from the stored key when it is an
    /// identity, so the conflict diagnostic names the value rather than its
    /// catalog id or physical encoding.
    fn from_stored(
        saved: SavedKey,
        meaning: &StoredValueMeaning,
        bytes: &[u8],
        facts: &CheckedFacts,
    ) -> Self {
        let display = match meaning {
            StoredValueMeaning::Enum { enum_id, .. } => {
                stored_enum_member_path(facts, *enum_id, bytes)
            }
            StoredValueMeaning::Identity { .. } => stored_identity_referent_path(meaning, &saved),
            StoredValueMeaning::Scalar(_) => None,
        };
        Self { saved, display }
    }
}

fn stored_arg_key(
    key: &CheckedSavedIndexKey,
    context: IndexWriteContext<'_>,
) -> Result<Option<IndexKey>, WriteError> {
    match key.source {
        StoreIndexKeySource::IdentityKey => {
            let Some(position) = context
                .place
                .identity_keys
                .iter()
                .position(|identity_key| identity_key.name == key.name)
            else {
                return Ok(None);
            };
            Ok(Some(IndexKey::scalar(context.identity[position].clone())))
        }
        StoreIndexKeySource::ResourceMember(member_id) => {
            let Some(member) = checked_root_member(context.place, member_id) else {
                return Ok(None);
            };
            let address = DataAddress::member(
                context.place,
                context.identity,
                &[],
                &member.catalog_id,
                context.span,
            )
            .map_err(runtime_store_error)?;
            let Some(bytes) =
                read_data(context.store, &address, context.span).map_err(runtime_store_error)?
            else {
                return Ok(None);
            };
            stored_index_key(&key.value_meaning, &bytes).map(|saved| {
                Some(IndexKey::from_stored(
                    saved,
                    &key.value_meaning,
                    &bytes,
                    context.facts,
                ))
            })
        }
    }
}

fn stored_index_keys(
    keys: &[CheckedSavedIndexKey],
    context: IndexWriteContext<'_>,
) -> Result<Option<Vec<IndexKey>>, WriteError> {
    keys.iter()
        .map(|key| stored_arg_key(key, context))
        .collect()
}

#[derive(Clone, Copy)]
enum FieldIndexValue<'a> {
    Leaf(&'a LeafValue),
    IdentityBytes(&'a [u8]),
}

impl FieldIndexValue<'_> {
    fn key_for(self, key: &CheckedSavedIndexKey) -> Result<Option<IndexKey>, WriteError> {
        match self {
            Self::Leaf(value) => Ok(value
                .as_key()
                .map_err(WriteError::from)?
                .map(|saved| IndexKey::from_leaf(saved, value))),
            Self::IdentityBytes(bytes) => stored_index_key(&key.value_meaning, bytes)
                .map(|saved| Some(IndexKey::from_meaning(saved, &key.value_meaning))),
        }
    }
}

fn field_write_index_keys(
    keys: &[CheckedSavedIndexKey],
    context: IndexWriteContext<'_>,
    field: &str,
    value: FieldIndexValue<'_>,
) -> Result<Option<Vec<IndexKey>>, WriteError> {
    keys.iter()
        .map(|key| {
            if key.name == field {
                value.key_for(key)
            } else {
                stored_arg_key(key, context)
            }
        })
        .collect()
}

fn index_touches_patch(index: &CheckedSavedIndex, patch: &[IndexFieldPatch]) -> bool {
    index.keys.iter().any(|key| match key.source {
        StoreIndexKeySource::ResourceMember(member) => {
            patch.iter().any(|field| field.member == member)
        }
        StoreIndexKeySource::IdentityKey => false,
    })
}

fn field_patch_index_keys(
    keys: &[CheckedSavedIndexKey],
    context: IndexWriteContext<'_>,
    patch: &[IndexFieldPatch],
) -> Result<Option<Vec<IndexKey>>, WriteError> {
    keys.iter()
        .map(|key| {
            if let StoreIndexKeySource::ResourceMember(member) = key.source
                && let Some(field) = patch.iter().find(|field| field.member == member)
            {
                return field.value.key_for(key);
            }
            stored_arg_key(key, context)
        })
        .collect()
}

fn checked_root_member(
    place: &CheckedSavedPlace,
    member: ResourceMemberId,
) -> Option<&CheckedSavedMember> {
    place
        .root_members
        .iter()
        .find(|checked| checked.id == Some(member))
}

fn stored_index_key(meaning: &StoredValueMeaning, bytes: &[u8]) -> Result<SavedKey, WriteError> {
    meaning.stored_key(bytes).ok_or_else(|| WriteError {
        code: WRITE_INVALID_DATA,
        message: "stored indexed value is not valid under its declared type".to_string(),
    })
}

fn index_address(
    index: &CheckedSavedIndex,
    keys: Vec<IndexKey>,
    span: SourceSpan,
) -> Result<IndexAddress, WriteError> {
    let keys = keys.into_iter().map(|key| key.saved).collect();
    IndexAddress::from_checked(&index.catalog_id, keys, span).map_err(runtime_store_error)
}

fn index_entry_value(unique: bool, identity: &[SavedKey]) -> Vec<u8> {
    if unique {
        encode_identity_payload(identity)
    } else {
        INDEX_MARKER.to_vec()
    }
}

fn check_unique_conflict(
    index: &CheckedSavedIndex,
    context: IndexWriteContext<'_>,
    new_keys: Option<&[IndexKey]>,
) -> Result<(), WriteError> {
    let Some(new_keys) = new_keys else {
        return Ok(());
    };
    let saved: Vec<SavedKey> = new_keys.iter().map(|key| key.saved.clone()).collect();
    let address = IndexAddress::from_checked(&index.catalog_id, saved, context.span)
        .map_err(runtime_store_error)?;
    let page = context
        .store
        .scan_index_tuple(&address.index, &address.keys, 2)
        .map_err(WriteError::from)?;
    if page
        .entries
        .iter()
        .any(|entry| entry.identity != context.identity)
    {
        return Err(WriteError {
            code: WRITE_UNIQUE_CONFLICT,
            message: format!(
                "unique index `{}` already holds key(s) {} for another identity",
                index.name,
                conflict_key_tuple_preview(new_keys)
            ),
        });
    }
    Ok(())
}

/// Render the conflicting key tuple for a unique-index diagnostic. An enum
/// segment shows its member name; every other segment uses the saved-key
/// preview, so a string is quoted and a scalar reads as itself.
fn conflict_key_tuple_preview(keys: &[IndexKey]) -> String {
    let rendered: Vec<String> = keys
        .iter()
        .map(|key| match &key.display {
            Some(display) => display.clone(),
            None => diagnostic_saved_key_preview(&key.saved),
        })
        .collect();
    format!("({})", rendered.join(", "))
}

fn runtime_store_error(error: crate::error::RuntimeError) -> WriteError {
    WriteError {
        code: WRITE_STORE,
        message: error.message,
    }
}
