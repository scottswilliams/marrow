mod children;
mod query;
mod query_error;
mod read;
mod record_nav;
mod render;
mod shape;
mod traversal;
mod walk;

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, EngineProfileDigest, StoreUid, TreeStore};

use crate::tooling::ToolingError;
use crate::{CheckedProgram, ScalarType, StoreLeafKind};

pub use children::{data_children, data_children_supports_paging};
pub use query::{data_query_under_prefix, resolve_data_query, resolve_source_text_data_query};
pub use query_error::{MemberFlavor, QueryError};
pub use read::{preview_data_query, read_data_query};
pub use render::{render_data_query_value, render_data_value, render_query_segments};
pub use traversal::{count_data_records, data_roots_in_store, visit_data_records};
pub use walk::walk_data;

pub(crate) use query::StorageDataQuery;
pub(crate) use render::{push_key, render_data_path};
pub(crate) use shape::{
    stored_key_mismatch, tooling_catalog_id, validate_member_path_node, validate_member_value_path,
};
pub(crate) use traversal::{
    checked_places, visit_data_records_in_places, visit_data_records_in_places_until,
    visit_place_record_identities_until,
};

pub const MAX_PREVIEW_ITEMS: usize = 10_000;
pub const DEFAULT_VALUE_PREVIEW_LIMIT: usize = 2048;
/// Public value previews clamp requested budgets before reading store bytes.
pub const MAX_VALUE_PREVIEW_LIMIT: usize = 64 * 1024;

pub(crate) fn clamp_value_preview_limit(limit: usize) -> usize {
    limit.min(MAX_VALUE_PREVIEW_LIMIT)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSnapshotStamp {
    pub store_uid: Option<StoreUid>,
    pub store_catalog_digest: Option<String>,
    pub store_commit: Option<DataCommitStamp>,
    pub checked_source_digest: String,
}

impl DataSnapshotStamp {
    fn read(program: &CheckedProgram, store: &TreeStore) -> Result<Self, StoreError> {
        let store_uid = store.read_store_uid()?;
        let commit = store.read_commit_metadata()?;
        let store_catalog_digest = store.catalog_snapshot_digest()?;
        Ok(Self {
            store_uid,
            store_catalog_digest,
            store_commit: commit.map(DataCommitStamp::from),
            checked_source_digest: program.source_digest(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCommitStamp {
    pub commit_id: u64,
    pub catalog_epoch: u64,
    pub source_digest: String,
    pub layout_epoch: u64,
    pub engine_profile_digest: EngineProfileDigest,
}

impl From<CommitMetadata> for DataCommitStamp {
    fn from(commit: CommitMetadata) -> Self {
        Self {
            commit_id: commit.commit_id,
            catalog_epoch: commit.catalog_epoch,
            source_digest: commit.source_digest,
            layout_epoch: commit.layout_epoch,
            engine_profile_digest: commit.engine_profile_digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampedData<T> {
    pub data: T,
    pub stamp: DataSnapshotStamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataReadResult {
    pub payload: Option<DebugDataPayload>,
    pub presence: DataPresence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataValuePreview {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPreviewReadResult {
    pub preview: Option<DataValuePreview>,
    pub presence: DataPresence,
}

pub fn stamped_data_roots_in_store(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<StampedData<Vec<String>>, StoreError> {
    with_stamped_read(program, store, |store| data_roots_in_store(program, store))
}

pub fn stamped_read_data_query(
    program: &CheckedProgram,
    store: &TreeStore,
    query: &DataQuery,
) -> Result<StampedData<DataReadResult>, StoreError> {
    with_stamped_read(program, store, |store| read_data_query(store, query))
}

pub fn stamped_preview_data_query(
    program: &CheckedProgram,
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
) -> Result<StampedData<DataPreviewReadResult>, StoreError> {
    with_stamped_read(program, store, |store| {
        preview_data_query(program, store, query, limit)
    })
}

pub fn stamped_data_children(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[DataQuerySegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<StampedData<DataChildrenPage>, ToolingError> {
    with_stamped_read(program, store, |store| {
        data_children(program, store, segments, limit, resume)
    })
}

pub(crate) fn with_stamped_read<T, E>(
    program: &CheckedProgram,
    store: &TreeStore,
    read: impl FnOnce(&TreeStore) -> Result<T, E>,
) -> Result<StampedData<T>, E>
where
    E: From<StoreError>,
{
    // The guard makes value/presence probes and the returned stamp describe one store version.
    let _snapshot = store.read_snapshot()?;
    let data = read(store)?;
    let stamp = DataSnapshotStamp::read(program, store)?;
    Ok(StampedData { data, stamp })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataQuery {
    path: String,
    root: String,
    segments: Vec<DataQuerySegment>,
    leaf: Option<StoreLeafKind>,
    pub(crate) storage: StorageDataQuery,
}

impl DataQuery {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn segments(&self) -> &[DataQuerySegment] {
        &self.segments
    }

    pub fn leaf(&self) -> Option<&StoreLeafKind> {
        self.leaf.as_ref()
    }

    pub(crate) fn new(
        path: String,
        root: String,
        segments: Vec<DataQuerySegment>,
        leaf: Option<StoreLeafKind>,
        storage: StorageDataQuery,
    ) -> Self {
        Self {
            path,
            root,
            segments,
            leaf,
            storage,
        }
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataQuerySegment {
    Root(String),
    Field(String),
    Layer(String),
    Key(SavedKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPresence {
    Absent,
    ValueOnly,
    ChildrenOnly,
}

impl DataPresence {
    /// The snake_case label this presence carries on the JSON wire.
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::ValueOnly => "value_only",
            Self::ChildrenOnly => "children_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataChild {
    Root(String),
    Key(SavedKey),
    Field(String),
    Layer(String),
}

impl From<DataQuerySegment> for DataChild {
    fn from(segment: DataQuerySegment) -> Self {
        match segment {
            DataQuerySegment::Root(root) => Self::Root(root),
            DataQuerySegment::Field(field) => Self::Field(field),
            DataQuerySegment::Layer(layer) => Self::Layer(layer),
            DataQuerySegment::Key(key) => Self::Key(key),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataChildrenPage {
    pub children: Vec<DataChild>,
    pub truncated: bool,
    pub cursor: Option<SavedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugDataPayload {
    bytes: Vec<u8>,
}

impl DebugDataPayload {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataEntry {
    pub path: String,
    pub segments: Vec<DataQuerySegment>,
    pub payload: DebugDataPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugDataCursorPath {
    segments: Vec<DataQuerySegment>,
}

impl DebugDataCursorPath {
    pub fn segments(&self) -> &[DataQuerySegment] {
        &self.segments
    }

    pub(crate) fn new(segments: Vec<DataQuerySegment>) -> Self {
        Self { segments }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataWalkPage {
    pub entries: Vec<DataEntry>,
    pub truncated: bool,
    pub next_cursor_path: Option<DebugDataCursorPath>,
}

#[derive(Debug, Clone)]
pub struct DataRecord {
    pub path: String,
    pub payload: DebugDataPayload,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) field_catalog_id: CatalogId,
    pub(crate) leaf: StoreLeafKind,
    pub(crate) key_mismatch: Option<KeyMismatch>,
}

impl DataRecord {
    pub fn leaf(&self) -> &StoreLeafKind {
        &self.leaf
    }
}

#[derive(Debug, Clone)]
pub(crate) struct KeyMismatch {
    pub(crate) expected: ScalarType,
    pub(crate) found: ScalarType,
}

#[cfg(test)]
mod tests {
    use marrow_store::tree::{CommitMetadata, EngineProfile, TreeStore};

    use crate::CheckedProgram;

    fn commit_metadata(commit_id: u64) -> CommitMetadata {
        let profile = EngineProfile::new(0);
        CommitMetadata {
            commit_id,
            catalog_epoch: 1,
            layout_epoch: profile.layout_epoch(),
            source_digest:
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
        }
    }

    #[test]
    fn stamped_store_read_runs_the_read_under_one_snapshot() {
        let program = CheckedProgram::default();
        let store = TreeStore::memory();

        let stamped = super::with_stamped_read(&program, &store, |store| {
            let error = store
                .write_commit_metadata(&commit_metadata(1))
                .expect_err("writes are rejected while the read snapshot is pinned");
            assert_eq!(error.code(), "store.transaction");
            Ok::<_, marrow_store::StoreError>(17)
        })
        .expect("stamped read");

        assert_eq!(stamped.data, 17);
    }
}
