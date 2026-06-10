//! Resolve and check a Marrow project's source.
//!
//! Discover the project's `.mw` files, parse each one, and report parse
//! diagnostics together with module/path resolution, type, and schema problems,
//! producing a resolved [`CheckedProgram`] alongside the diagnostics.

pub use marrow_store::value::ScalarType;

pub mod analysis;
pub mod binding;
mod catalog;
mod checks;
mod diagnostics;
mod driver;
pub mod durable_path;
mod enums;
pub mod evolution;
pub mod executable;
pub mod facts;
mod infer;
mod presence;
pub mod program;
mod rejected_surface;
pub mod resolve;
mod rules;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod tooling;
mod typerules;
mod walk;

pub use analysis::{AnalysisSnapshot, AnalyzedFile, analyze_project, scope_at, type_at};
pub use binding::{BindingIndex, RenameSafety, SymbolKind, SymbolRef, build_binding_index};
pub use diagnostics::{
    AppendTargetDiagnostic, CHECK_AMBIGUOUS_CALL, CHECK_AMBIGUOUS_MATCH_ARM,
    CHECK_AMBIGUOUS_MEMBER, CHECK_ASSIGNMENT_TYPE, CHECK_BARE_MAYBE_PRESENT_READ,
    CHECK_CALL_ARGUMENT, CHECK_CATALOG_INTENT, CHECK_CATEGORY_NOT_SELECTABLE,
    CHECK_COLLECTION_UNSUPPORTED, CHECK_CONDITION_TYPE, CHECK_DUPLICATE_DECLARATION,
    CHECK_DUPLICATE_MATCH_ARM, CHECK_DUPLICATE_MODULE, CHECK_EVOLVE_TARGET, CHECK_EVOLVE_TRANSFORM,
    CHECK_EVOLVE_TYPE, CHECK_IS_REQUIRES_ENUM, CHECK_IS_TYPE, CHECK_KEY_TYPE, CHECK_LITERAL_RANGE,
    CHECK_MATCH_REQUIRES_ENUM, CHECK_MISSING_RETURN, CHECK_MODULE_PATH, CHECK_MULTIPLE_SCRIPTS,
    CHECK_NEIGHBOR_UNSUPPORTED, CHECK_NEXT_ID_REQUIRES_SINGLE_INT, CHECK_NONEXHAUSTIVE_MATCH,
    CHECK_OPERATOR_TYPE, CHECK_PRIVATE_ENUM, CHECK_PRIVATE_FUNCTION, CHECK_RANGE,
    CHECK_RANGE_VALUE, CHECK_REJECTED_SURFACE, CHECK_RETURN_TYPE, CHECK_RETURN_VALUE,
    CHECK_THROW_TYPE, CHECK_TRY_HANDLER, CHECK_UNKNOWN_ENUM_MEMBER, CHECK_UNKNOWN_TYPE,
    CHECK_UNRESOLVED_CALL, CHECK_UNRESOLVED_IMPORT, CHECK_UNRESOLVED_NAME, CHECK_UNTYPED_VALUE,
    CheckDiagnostic, CheckReport, ConversionTarget, ConversionUnsupportedSourceDiagnostic,
    DiagnosticPayload, EnumDiagnostic, IO_READ, RejectedSurface, SCHEMA_DUPLICATE_ROOT_OWNER,
};
pub use driver::{
    ProjectSources, check_project, check_project_with_catalog, check_tests, check_tests_program,
    check_tests_with_sources, check_tests_with_sources_program,
};
pub use durable_path::{
    PathParseError, PathSegment, StoreLeafKind, display_path, identity_leaf_key_mismatch,
    parse_path,
};
pub use executable::{
    CheckedArg, CheckedArgMode, CheckedBinaryOp, CheckedBody, CheckedBuiltinCall,
    CheckedCallTarget, CheckedCatchClause, CheckedElseIf, CheckedEnumMemberRef, CheckedEnumRef,
    CheckedExpr, CheckedForBinding, CheckedFunctionRef, CheckedInterpolationPart,
    CheckedLiteralKind, CheckedMatchArm, CheckedParamMode, CheckedResourceConstructor,
    CheckedResourceConstructorField, CheckedResourceRef, CheckedRuntimeValueType,
    CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedLayer,
    CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace, CheckedSavedTerminal,
    CheckedStdCall, CheckedStmt, CheckedUnaryOp, checked_activation_root_places,
    checked_saved_root_place,
};
pub use facts::PresenceProofRead;
pub use facts::{
    CheckedFacts, CheckedType, DirectEffectFacts, EnumFact, EnumId, EnumMemberFact, EnumMemberId,
    FunctionFact, FunctionId, FutureEphemeralRootEffect, FutureEphemeralRootEffects, HostEffect,
    LocalFact, LocalId, ModuleFact, ModuleId, ResourceFact, ResourceId, ResourceMemberFact,
    ResourceMemberId, ResourceMemberKind, SavedPlaceEffect, StoreFact, StoreId,
    StoreIdentityKeyFact, StoreIndexFact, StoreIndexId, StoreIndexKeyFact, StoreIndexKeySource,
    StoredValueMeaning,
};
pub use facts::{
    PresenceProofFact, PresenceProofId, PresenceProofPlace, PresenceProofSource,
    PresenceProofStatus,
};
pub use marrow_catalog::{CatalogEntryKind, CatalogLifecycle};
pub use marrow_project::ProjectConfig;
pub use marrow_schema::{IndexSchema, ResourceSchema, StoreSchema, Type};
pub use program::{
    CheckedConst, CheckedEntryFunction, CheckedFunction, CheckedModule, CheckedParam,
    CheckedProgram, CheckedRuntimeConst, CheckedRuntimeFunction, CheckedRuntimeModule,
    CheckedRuntimeProgram, EvolveTransform, FileId, MarrowType, ProgramCatalog,
};
pub use resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};

pub(crate) use driver::{
    CheckedFile, TestResolutionSuppression, build_alias_map, builtin_return_type,
    check_file_source, check_tests_with_sources_analysis, conversion_return_type, enum_visibility,
    expand_alias, expand_module_alias, find_resource_schema, identity_type_for_store,
    is_builtin_call, is_resolved_import, module_of_file, module_path_error, push_schema_error,
    read_source, resolve_function_in_module, resolve_resource_schema_type, resolve_resource_type,
    resource_type_name, split_type_path, std_call_params, std_call_return_type,
};
pub(crate) use program::TypeNames;
pub(crate) use rejected_surface::check_rejected_surface;
