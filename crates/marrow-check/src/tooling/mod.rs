//! Transport-free facts for CLI, editor, backup, restore, and debug adapters.

use marrow_store::StoreError;

pub mod data;
pub mod explain;
pub mod integrity;
pub mod metadata;

pub use data::{
    DataChild, DataChildrenPage, DataEntry, DataPresence, DataQuery, DataQuerySegment, DataRecord,
    DataWalkPage, DebugDataCursorPath, DebugDataPayload, MAX_PREVIEW_ITEMS, MemberFlavor,
    QueryError, count_data_records, data_children, data_children_supports_paging,
    data_query_under_prefix, data_roots_in_store, read_data_query, render_query_segments,
    resolve_data_query, resolve_source_text_data_query, visit_data_records, walk_data,
};
pub use explain::{
    IndexExplanation, NameExplanation, NameResolutionExplanation, SavedPathExplanation,
    explain_name, explain_saved_path,
};
pub use integrity::{
    IntegrityOutcome, IntegrityOutcomeKind, IntegrityProblem, ORPHAN_INTEGRITY_HELP,
    count_activation_integrity_problems, count_integrity_problems, visit_integrity_problems,
};
pub use metadata::{ToolingCatalogMetadata, store_is_newer_than_program, tooling_metadata};

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
