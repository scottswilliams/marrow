//! Transport-free facts for CLI, editor, backup, restore, and debug adapters.

use marrow_store::StoreError;

pub mod data;
pub mod integrity;
pub mod signatures;

pub use data::{
    DEFAULT_VALUE_PREVIEW_LIMIT, DataChild, DataChildrenPage, DataCommitStamp, DataEntry,
    DataPathError, DataPathSegment, DataPresence, DataPreviewReadResult, DataReadResult,
    DataRecord, DataSnapshotStamp, DataValuePreview, DataWalkPage, DebugDataCursorPath,
    DebugDataPayload, DeclaredDataChild, DeclaredDataChildKind, DeclaredDataKeyParam,
    MAX_PREVIEW_ITEMS, MAX_VALUE_PREVIEW_LIMIT, MemberFlavor, ResolvedDataPath,
    SourceDataPathSegment, StampedData, count_data_records, data_children,
    data_children_supports_paging, data_path_under_prefix, data_roots_in_store,
    declared_data_children, declared_source_data_children, declared_source_receiver_data_children,
    preview_data_path, read_data_path, render_data_path_segments, render_data_path_value,
    render_data_value, resolve_data_path, resolve_source_text_data_path, stamped_data_children,
    stamped_data_roots_in_store, stamped_preview_data_path, stamped_read_data_path,
    visit_data_records, walk_data,
};
pub use integrity::{
    IntegrityOutcome, IntegrityProblem, IntegrityProblemSample, IntegritySample,
    count_activation_integrity_problems, count_integrity_problems,
    sample_integrity_problem_details, sample_integrity_problems, stamped_integrity_problem_details,
    visit_integrity_problems,
};
pub use signatures::{
    ActiveCallableContext, CallableArgumentStyle, CallableParameter, CallableSignature,
    CallableSignatureKind, CallableValueShape, ResourceConstructorField,
    ResourceConstructorSignature, active_callable_context, intrinsic_callable_signature,
    intrinsic_callable_signature_for_file, resource_constructor_signature,
};

#[derive(Debug)]
pub enum ToolingError {
    Path(DataPathError),
    Store(StoreError),
}

impl From<StoreError> for ToolingError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<DataPathError> for ToolingError {
    fn from(error: DataPathError) -> Self {
        Self::Path(error)
    }
}
