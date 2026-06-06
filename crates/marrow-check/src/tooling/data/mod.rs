mod children;
mod query;
mod query_error;
mod read;
mod render;
mod shape;
mod traversal;
mod walk;

use marrow_store::key::SavedKey;

use crate::{ScalarType, StoreLeafKind};

pub use children::{data_children, data_children_supports_paging};
pub use query::{data_query_under_prefix, resolve_data_query, resolve_source_text_data_query};
pub use query_error::{MemberFlavor, QueryError};
pub use read::read_data_query;
pub use render::render_query_segments;
pub use traversal::{count_data_records, data_roots_in_store, visit_data_records};
pub use walk::walk_data;

pub(crate) use query::StorageDataQuery;
pub(crate) use render::{push_key, render_data_path};
pub(crate) use shape::validate_member_value_path;
pub(crate) use traversal::{checked_places, visit_data_records_in_places};

pub const MAX_PREVIEW_ITEMS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataQuery {
    path: String,
    root: String,
    segments: Vec<DataQuerySegment>,
    pub(crate) storage: StorageDataQuery,
}

impl DataQuery {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn segments(&self) -> &[DataQuerySegment] {
        &self.segments
    }

    pub(crate) fn new(
        path: String,
        root: String,
        segments: Vec<DataQuerySegment>,
        storage: StorageDataQuery,
    ) -> Self {
        Self {
            path,
            root,
            segments,
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
    Key(SavedKey),
    Member(String),
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
    pub(crate) leaf: StoreLeafKind,
    pub(crate) key_mismatch: Option<KeyMismatch>,
}

#[derive(Debug, Clone)]
pub(crate) struct KeyMismatch {
    pub(crate) expected: ScalarType,
    pub(crate) found: ScalarType,
}
