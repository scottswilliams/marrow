//! Transport-free facts for CLI, editor, backup, restore, and debug adapters.

use marrow_store::StoreError;

mod completion;
pub mod data;
mod expected;
mod hover;
pub mod integrity;
mod navigation;
mod render;
mod semantic_tokens;
pub mod signatures;
mod source_roots;
pub mod symbols;
mod type_annotations;

pub use completion::{
    SOURCE_COMPLETION_PROFILE_VERSION, SourceCompletionContext, SourceCompletionFact,
    SourceCompletionItem, SourceCompletionItemKind, SourceEnumNamespaceCompletionFact,
    SourceModuleNamespaceCompletionFact, SourceNamespaceCompletionFact,
    SourceNamespaceEnumCompletion, SourceNamespaceEnumMemberCompletion,
    SourceNamespaceEnumMemberStatus, SourceNamespaceFunctionCompletion,
    SourceNamespaceFunctionParamCompletion, SourceNamespaceResourceCompletion,
    SourceSavedPathCompletionContextFact, SourceSavedPathCompletionFact,
    SourceSavedPathCompletionRoot, SourceSavedPathCompletionSegment,
    SourceSavedRootCompletionCandidate, SourceSavedRootCompletionFact,
    SourceStandardLibraryModuleCompletion, SourceStandardLibraryModuleNamespaceCompletionFact,
    SourceStandardLibraryOperationCompletion, SourceStandardLibraryRootNamespaceCompletionFact,
    SourceTypeBuiltin, SourceTypeCompletionCandidate, SourceTypeCompletionFact,
    source_completion_context, source_completion_fact, source_namespace_completion_fact,
    source_namespace_completion_file_fact, source_saved_path_completion_fact_at,
    source_saved_root_completion_fact, source_type_completion_fact,
};
pub use data::{
    DEFAULT_VALUE_PREVIEW_LIMIT, DataChild, DataChildView, DataChildViewsPage, DataChildrenPage,
    DataCommitStamp, DataEntry, DataPathError, DataPathSegment, DataPresence,
    DataPreviewReadResult, DataReadResult, DataRecord, DataSnapshotStamp, DataTransactionStamp,
    DataValuePreview, DataWalkPage, DebugDataCursorPath, DebugDataPayload, DeclaredDataChild,
    DeclaredDataChildKind, DeclaredDataKeyParam, MAX_PREVIEW_ITEMS, MAX_VALUE_PREVIEW_LIMIT,
    MemberFlavor, ResolvedDataPath, SavedDataPathSegment, SourceDataPathSegment, StampedData,
    count_data_records, data_children, data_children_supports_paging, data_path_under_prefix,
    data_roots_in_store, data_snapshot_stamp, declared_data_children,
    declared_source_data_children, preview_data_path, preview_runtime_data_path, read_data_path,
    render_data_path_segments, render_data_path_value, render_data_value, resolve_data_path,
    resolve_runtime_data_path, resolve_runtime_saved_data_path, resolve_saved_data_path,
    resolve_source_text_data_path, runtime_data_children, runtime_data_children_supports_paging,
    runtime_data_roots_in_store, runtime_data_snapshot_stamp,
    runtime_open_transaction_data_snapshot_stamp, runtime_saved_data_child_views,
    runtime_saved_data_root_views_in_store, saved_data_child_views, saved_data_root_views_in_store,
    stamped_data_children, stamped_data_roots_in_store, stamped_preview_data_path,
    stamped_read_data_path, stamped_runtime_data_children, stamped_runtime_data_roots_in_store,
    stamped_runtime_open_transaction_data_children,
    stamped_runtime_open_transaction_preview_data_path, stamped_runtime_preview_data_path,
    stamped_runtime_saved_data_child_views, stamped_runtime_saved_data_root_views_in_store,
    stamped_saved_data_child_views, stamped_saved_data_root_views_in_store, visit_data_records,
    walk_data,
};
pub(crate) use hover::{PrelexedSourceHover, source_non_type_hover_fact_at_prelexed};
pub use hover::{
    SavedPlaceHoverFact, SavedPlaceHoverKeyParam, SourceCallableFunctionFact,
    SourceCallableHoverFact, SourceCallableParamFact, SourceEnumHoverFact,
    SourceEnumMemberHoverFact, SourceEnumMemberStatus, SourceEnumMemberSummary, SourceHoverFact,
    SourceModulePathDefinitionFact, SourceModulePathHoverFact, SourceOperatorHoverFact,
    SourceProjectModuleHoverFact, SourceResourceHoverFact, SourceResourceHoverMember,
    SourceResourceHoverMemberKind, SourceResourceHoverPathSegment, SourceSchemaHoverFact,
    SourceSchemaHoverKeyParam, SourceStandardLibraryCapability,
    SourceStandardLibraryModuleHoverFact, SourceStandardLibraryNamespaceHoverFact,
    SourceStandardLibraryOperationHoverFact, SourceSymbolDocs, SourceTypeHoverFact,
    StoreRootHoverFact, StoreRootHoverMember, StoreRootHoverPathSegment, saved_place_hover_fact_at,
    source_callable_hover_fact_at, source_hover_fact_at, source_module_path_definition_fact_at,
    source_module_path_hover_fact_at, source_operator_hover_fact_at, source_schema_hover_fact_at,
    source_symbol_docs_at, source_type_hover_fact_at, store_root_hover_fact_at,
};
pub use integrity::{
    IntegrityOutcome, IntegrityProblem, IntegrityProblemSample, IntegritySample,
    count_activation_integrity_problems, count_integrity_problems, count_orphan_cells,
    sample_integrity_problem_details, sample_integrity_problems, stamped_integrity_problem_details,
    verify_index_integrity, verify_store_completeness, verify_store_roots_against_lock,
    visit_integrity_problems,
};
pub use navigation::{
    SourceCatalogLocationFact, source_catalog_definition_fact_at, source_catalog_reference_facts_at,
};
pub use render::{render_callable_shape, render_callable_signature, render_marrow_type};
pub use semantic_tokens::{
    SourceSemanticTokenFact, SourceSemanticTokenModifiers, SourceSemanticTokenRole,
    source_semantic_token_facts, source_semantic_token_facts_for_file,
};
pub use signatures::{
    ActiveCallableContext, CallableArgumentStyle, CallableCalleeContext, CallableParameter,
    CallableSignature, CallableSignatureKind, CallableValueShape, ResourceConstructorField,
    ResourceConstructorSignature, SourceSignatureHelpCallable, SourceSignatureHelpFact,
    SourceSignatureHelpParameter, active_callable_context, callable_callee_contexts,
    intrinsic_callable_signature, intrinsic_callable_signature_for_file,
    intrinsic_completion_callables, resource_constructor_signature, source_signature_help_fact_at,
};
pub use source_roots::{
    SourceSavedRootCursorFact, SourceSavedRootCursorKind, source_saved_root_cursor_fact_at,
};
pub use symbols::{
    DocumentSymbol, DocumentSymbolKind, SourceSymbol, SourceSymbolKind, document_symbols,
    source_symbols, source_symbols_matching,
};
pub use type_annotations::{
    IdentityTypeAnnotation, SourceTypeAnnotationCursorFact, identity_type_annotations,
    source_type_annotation_cursor_fact_at,
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
