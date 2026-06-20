//! Resolve and check a Marrow project's source.
//!
//! Discover the project's `.mw` files, parse each one, and report parse
//! diagnostics together with module/path resolution, type, and schema problems,
//! producing a resolved [`CheckedProgram`] alongside the diagnostics.

pub use marrow_store::value::ScalarType;

pub mod analysis;
mod annotation_refs;
mod backing_validity;
pub mod binding;
mod catalog;
mod checks;
mod diagnostics;
mod driver;
pub mod durable_path;
mod entry_abi;
mod enums;
pub mod evolution;
pub mod executable;
pub mod facts;
mod hex;
mod infer;
mod keyed_entries;
mod presence;
pub mod program;
mod project_io;
mod rejected_surface;
pub mod resolve;
mod rules;
mod source_spans;
mod surface;
mod surface_abi;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod tooling;
mod typerules;
mod walk;

pub use analysis::{
    AnalysisIdentity, AnalysisSnapshot, AnalyzedFile, CatalogDeclaration,
    SurfaceActionOperationAnalysis, SurfaceCreateOperationAnalysis, SurfaceDeleteOperationAnalysis,
    SurfaceReadOperationAnalysis, SurfaceUpdateOperationAnalysis, UseSite, UseSiteKind,
    analyze_project, scope_at, type_at,
};
pub use binding::{
    BindingIndex, ParameterDefinition, RenameAction, RenameSafety, SourceEdit, SymbolKind,
    SymbolRef, build_binding_index,
};
pub use diagnostics::{
    AppendTargetDiagnostic, CHECK_AMBIGUOUS_CALL, CHECK_AMBIGUOUS_MATCH_ARM,
    CHECK_AMBIGUOUS_MEMBER, CHECK_ASSIGNMENT_TYPE, CHECK_BARE_MAYBE_PRESENT_READ,
    CHECK_BYTES_ESCAPE, CHECK_CALL_ARGUMENT, CHECK_CATALOG_INTENT, CHECK_CATEGORY_NOT_SELECTABLE,
    CHECK_COLLECTION_UNSUPPORTED, CHECK_COMMIT_AMPLIFICATION, CHECK_CONDITION_TYPE,
    CHECK_DEFAULT_ENTRY, CHECK_DUPLICATE_DECLARATION, CHECK_DUPLICATE_MATCH_ARM,
    CHECK_DUPLICATE_MODULE, CHECK_DURABLE_STORE_REQUIRED, CHECK_EVOLVE_TARGET,
    CHECK_EVOLVE_TRANSFORM, CHECK_EVOLVE_TYPE, CHECK_EXPOSED_PRIVATE_ENUM, CHECK_IS_REQUIRES_ENUM,
    CHECK_IS_TYPE, CHECK_KEY_REQUIRES_SINGLE_KEY, CHECK_KEY_TYPE, CHECK_LAYER_NOT_VALUE,
    CHECK_LITERAL_RANGE, CHECK_LOCK_CORRUPT, CHECK_LOSSY_ROUND_TRIP, CHECK_MATCH_REQUIRES_ENUM,
    CHECK_MISSING_RETURN, CHECK_MODULE_PATH, CHECK_MULTIPLE_SCRIPTS, CHECK_NEIGHBOR_UNSUPPORTED,
    CHECK_NEXT_ID_COLLISION, CHECK_NEXT_ID_REQUIRES_SINGLE_INT, CHECK_NONEXHAUSTIVE_MATCH,
    CHECK_OPERATOR_TYPE, CHECK_PRIVATE_ENUM, CHECK_PRIVATE_FUNCTION, CHECK_RANGE,
    CHECK_RANGE_VALUE, CHECK_READ_ONLY_EXPRESSION_CONTEXT, CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT,
    CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP, CHECK_READ_ONLY_EXPRESSION_WRITE,
    CHECK_RECURSIVE_KEYED_ENTRY, CHECK_REJECTED_SURFACE, CHECK_REQUIRED_ABSENT, CHECK_RETURN_TYPE,
    CHECK_RETURN_VALUE, CHECK_STRING_ESCAPE, CHECK_SURFACE_ACTION, CHECK_SURFACE_COLLISION,
    CHECK_SURFACE_COMPUTED_READ, CHECK_SURFACE_FIELD, CHECK_SURFACE_TARGET, CHECK_THROW_TYPE,
    CHECK_UNKNOWN_ENUM_MEMBER, CHECK_UNKNOWN_FIELD, CHECK_UNKNOWN_TYPE, CHECK_UNRESOLVED_CALL,
    CHECK_UNRESOLVED_IMPORT, CHECK_UNRESOLVED_NAME, CHECK_UNTYPED_VALUE, CatalogIntentDiagnostic,
    CatalogIntentKind, CatalogPathCandidate, CheckDiagnostic, CheckReport, ConversionTarget,
    ConversionUnsupportedSourceDiagnostic, DefaultEntryProblem, DiagnosticPayload, EnumDiagnostic,
    IO_READ, RejectedSurface, SCHEMA_DUPLICATE_ROOT_OWNER, SurfaceActionDiagnostic,
    SurfaceCollisionNameKind, SurfaceComputedReadDiagnostic, SurfaceFieldDiagnostic,
    SurfaceFieldList, SurfaceFieldProblem, SurfaceTargetDiagnostic,
};
pub use driver::{
    ProjectSources, check_project, check_project_with_catalog, check_tests, check_tests_program,
};
pub use durable_path::{
    PathParseError, PathSegment, StoreLeafKind, display_path, identity_leaf_key_mismatch,
    parse_path,
};
pub use entry_abi::{
    ENTRY_PROTOCOL_TAG_VERSION, EntryArgumentShape, EntryDescriptor, EntryDescriptorError,
    EntryEnumMember, EntryFunctionSurfaceDescriptor, EntryIdentity, EntryIdentityKey,
    EntryParameter, EntryResourceResultField, EntryResultDescriptor, EntrySurfaceProfile,
    EntrySurfaceValueShape,
};
pub use executable::{
    CheckedArg, CheckedBinaryOp, CheckedBody, CheckedBuiltinCall, CheckedCallTarget,
    CheckedCatchClause, CheckedElseIf, CheckedEnumMemberRef, CheckedEnumRef, CheckedExpr,
    CheckedForBinding, CheckedFunctionRef, CheckedIdentityConstructor, CheckedInterpolationPart,
    CheckedLiteralKind, CheckedMatchArm, CheckedResourceConstructor,
    CheckedResourceConstructorField, CheckedResourceRef, CheckedRuntimeValueType,
    CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedLayer,
    CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, CheckedSavedTerminal,
    CheckedStdCall, CheckedStmt, CheckedUnaryOp, checked_activation_root_places,
    checked_place_store_id, checked_saved_root_place, for_each_place_record,
};
pub use facts::PresenceProofRead;
pub use facts::{
    CheckedFacts, CheckedType, DirectEffectFacts, EffectClosureFacts, EntryCostShapeFact,
    EntryFootprintFact, EntryStoreOpenMode, EnumFact, EnumId, EnumMemberFact, EnumMemberId,
    FunctionFact, FunctionId, HostEffect, LocalFact, LocalId, ModuleFact, ModuleId, ResourceFact,
    ResourceId, ResourceMemberFact, ResourceMemberId, ResourceMemberKind, SavedPlaceEffect,
    StoreFact, StoreId, StoreIdentityKeyFact, StoreIndexFact, StoreIndexId, StoreIndexKeyFact,
    StoreIndexKeySource, StoredValueMeaning, SurfaceActionFact, SurfaceCatalogBlocker,
    SurfaceCatalogStatus, SurfaceCollectionFact, SurfaceCollectionTarget, SurfaceComputedReadFact,
    SurfaceDeleteFact, SurfaceFact, SurfaceFieldFact, SurfaceId, SurfaceReadFootprint,
    SurfaceReadOperationFact, SurfaceReadOperationKind, WorkShapeClass,
};
pub use facts::{
    PresenceProofFact, PresenceProofId, PresenceProofPlace, PresenceProofSource,
    PresenceProofStatus,
};
pub use marrow_catalog::{CatalogEntryKind, CatalogLifecycle};
pub use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};
pub use marrow_schema::{
    IndexSchema, KeyDef, Node, NodeKind, ResourceSchema, ReturnPresence, StoreSchema, Type,
};
pub use program::{
    CheckedConst, CheckedDebugExpression, CheckedEntryFunction, CheckedFunction, CheckedModule,
    CheckedParam, CheckedProgram, CheckedReadOnlyExpression, CheckedRuntimeConst,
    CheckedRuntimeFunction, CheckedRuntimeModule, CheckedRuntimeProgram, DebugSourceIdentity,
    EvolveTransform, FileId, MarrowType, ProgramCatalog, RuntimeStopPoint,
};
pub use project_io::{
    CONFIG_DATA_DIR, ProjectIoError, check_project_against, check_source_project_analysis_against,
    load_config, native_store_path, read_accepted_catalog_artifact,
    read_accepted_catalog_with_store, read_accepted_catalog_with_store_read_only,
    recheck_against_store_catalog, recheck_source_project_analysis_against_store_catalog,
    render_accepted_catalog_file, resolve_store_path,
};
pub use resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};
pub use surface_abi::{
    SURFACE_COMPUTED_READ_OPERATION_TAG_VERSION, SURFACE_CREATE_OPERATION_TAG_VERSION,
    SURFACE_DELETE_OPERATION_TAG_VERSION, SURFACE_READ_OPERATION_TAG_VERSION,
    SURFACE_UPDATE_OPERATION_TAG_VERSION, SurfaceActionOperationDescriptor,
    SurfaceComputedReadCostShape, SurfaceComputedReadOperationDescriptor,
    SurfaceCreateBodySemantics, SurfaceCreateExistenceSemantics, SurfaceCreateIdentityPolicy,
    SurfaceCreateOperationDescriptor, SurfaceCreateOperationDescriptorKind,
    SurfaceCreateOperationField, SurfaceDeleteOperationDescriptor,
    SurfaceDeleteOperationDescriptorKind, SurfaceDeleteSemantics, SurfaceOperationIdentityKey,
    SurfaceOperationValueShape, SurfaceReadOperationDescriptor, SurfaceReadOperationDescriptorKind,
    SurfaceReadOperationIndexKey, SurfaceReadOperationIndexKeySource,
    SurfaceReadOperationProjectionField, SurfaceUpdateOperationDescriptor,
    SurfaceUpdateOperationDescriptorKind, SurfaceUpdateOperationField, SurfaceUpdatePatchSemantics,
};

pub(crate) use driver::{
    CheckedFile, TestResolutionSuppression, build_alias_map, builtin_return_type,
    check_file_source, check_tests_with_sources_analysis, conversion_return_type, enum_visibility,
    expand_alias, expand_module_alias, expand_unique_import_alias, has_duplicate_error,
    identity_type_for_store, is_builtin_call, is_resolved_import, is_unknown_std_operation,
    module_of_file, module_path_error, push_schema_error, read_source, resolve_function_in_module,
    resolve_resource_schema_type, resolve_resource_type, resource_type_name, short_name,
    split_type_path, std_call_params, std_call_return_type,
};
pub(crate) use program::TypeNames;
pub(crate) use rejected_surface::check_rejected_surface;
