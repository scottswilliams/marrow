//! Transport-free facts for CLI, editor, backup, restore, and debug adapters.

use marrow_store::StoreError;

pub mod data;
pub mod integrity;

pub use data::{
    DataChild, DataChildrenPage, DataCommitStamp, DataEntry, DataPresence, DataQuery,
    DataQuerySegment, DataReadResult, DataRecord, DataSnapshotStamp, DataWalkPage,
    DebugDataCursorPath, DebugDataPayload, MAX_PREVIEW_ITEMS, MemberFlavor, QueryError,
    StampedData, count_data_records, data_children, data_children_supports_paging,
    data_query_under_prefix, data_roots_in_store, read_data_query, render_data_query_value,
    render_data_value, render_query_segments, resolve_data_query, resolve_source_text_data_query,
    stamped_data_children, stamped_data_roots_in_store, stamped_read_data_query,
    visit_data_records, walk_data,
};
pub use integrity::{
    IntegrityOutcome, IntegrityProblem, IntegrityProblemSample, IntegritySample,
    count_activation_integrity_problems, count_integrity_problems,
    sample_integrity_problem_details, sample_integrity_problems, stamped_integrity_problem_details,
    visit_integrity_problems,
};

#[derive(Debug)]
pub enum ToolingError {
    Query(QueryError),
    Store(StoreError),
}

impl From<StoreError> for ToolingError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<QueryError> for ToolingError {
    fn from(error: QueryError) -> Self {
        Self::Query(error)
    }
}
