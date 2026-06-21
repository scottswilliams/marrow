//! Transport-free facts for CLI, editor, backup, restore, and debug adapters.

use marrow_store::StoreError;

pub mod data;
mod hover;
pub mod integrity;
pub mod signatures;
pub mod symbols;
mod type_annotations;

pub use data::{
    DEFAULT_VALUE_PREVIEW_LIMIT, DataChild, DataChildView, DataChildViewsPage, DataChildrenPage,
    DataCommitStamp, DataEntry, DataPathError, DataPathSegment, DataPresence,
    DataPreviewReadResult, DataReadResult, DataRecord, DataSnapshotStamp, DataTransactionStamp,
    DataValuePreview, DataWalkPage, DebugDataCursorPath, DebugDataPayload, DeclaredDataChild,
    DeclaredDataChildKind, DeclaredDataKeyParam, MAX_PREVIEW_ITEMS, MAX_VALUE_PREVIEW_LIMIT,
    MemberFlavor, ResolvedDataPath, SavedDataPathSegment, SourceDataPathSegment, StampedData,
    count_data_records, data_children, data_children_supports_paging, data_path_under_prefix,
    data_roots_in_store, data_snapshot_stamp, declared_data_children,
    declared_source_data_children, declared_source_receiver_data_children, preview_data_path,
    preview_runtime_data_path, read_data_path, render_data_path_segments, render_data_path_value,
    render_data_value, resolve_data_path, resolve_runtime_data_path,
    resolve_runtime_saved_data_path, resolve_saved_data_path, resolve_source_text_data_path,
    runtime_data_children, runtime_data_children_supports_paging, runtime_data_roots_in_store,
    runtime_saved_data_child_views, runtime_saved_data_root_views_in_store, saved_data_child_views,
    saved_data_root_views_in_store, stamped_data_children, stamped_data_roots_in_store,
    stamped_preview_data_path, stamped_read_data_path, stamped_runtime_data_children,
    stamped_runtime_data_roots_in_store, stamped_runtime_open_transaction_data_children,
    stamped_runtime_open_transaction_preview_data_path, stamped_runtime_preview_data_path,
    stamped_runtime_saved_data_child_views, stamped_runtime_saved_data_root_views_in_store,
    stamped_saved_data_child_views, stamped_saved_data_root_views_in_store, visit_data_records,
    walk_data,
};
pub use hover::{
    SavedPlaceHoverFact, SavedPlaceHoverKeyParam, SourceSymbolDocs, StoreRootHoverFact,
    StoreRootHoverMember, StoreRootHoverPathSegment, saved_place_hover_fact_at,
    source_symbol_docs_at, store_root_hover_fact_at,
};
pub use integrity::{
    IntegrityOutcome, IntegrityProblem, IntegrityProblemSample, IntegritySample,
    count_activation_integrity_problems, count_integrity_problems, count_orphan_cells,
    sample_integrity_problem_details, sample_integrity_problems, stamped_integrity_problem_details,
    visit_integrity_problems,
};
pub use signatures::{
    ActiveCallableContext, CallableArgumentStyle, CallableCalleeContext, CallableParameter,
    CallableSignature, CallableSignatureKind, CallableValueShape, ResourceConstructorField,
    ResourceConstructorSignature, active_callable_context, callable_callee_contexts,
    intrinsic_callable_signature, intrinsic_callable_signature_for_file,
    intrinsic_completion_callables, resource_constructor_signature,
};
pub use symbols::{
    DocumentSymbol, DocumentSymbolKind, SourceSymbol, SourceSymbolKind, document_symbols,
    source_symbols, source_symbols_matching,
};
pub use type_annotations::{IdentityTypeAnnotation, identity_type_annotations};

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
