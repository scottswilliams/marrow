//! The Marrow runtime: evaluate checked `.mw` functions.
//!
//! The evaluator runs functions over scalar, identity, resource, sequence, and
//! temporal values with locals, numeric and temporal arithmetic, string `+`,
//! comparison/logical operators, conditionals, `while`/`for` loops,
//! interpolation, and calls between functions. It reads saved data (fields and
//! keyed-leaf entries) and writes it through the managed-write layer
//! (`^books(id).field = …`, `delete`, `append`), groups writes in a
//! `transaction` (commit/rollback with read-your-writes), and provides the
//! `print`/`exists`/`nextId`/`append` builtins, the `?.` optional read and `??`
//! absence-default, the `std::assert`/`std::text`/`std::math` library helpers,
//! pure `std::clock` parse/format helpers, and the `std::clock::now()` and
//! `std::env` host capabilities. Whole-resource writes, index traversal, and
//! structured errors build on the same spine.
//!
//! The evaluator is carved into sibling modules along the call spine
//! (`expr`/`call`/`exec`/`read`/`write_dispatch`/`path`) plus its leaf
//! supports (`error`/`value`/`host`/`env`/`stdlib`/`collection`).
//! `ProjectSession` is the unstable project invocation boundary used by the CLI:
//! it checks a project, admits the configured store, fences and auto-applies run
//! evolution when allowed, and invokes entries for `marrow run` and `marrow test`
//! through one runtime path.

pub mod base64;
pub mod hex;
pub(crate) mod write;

mod activation;
mod call;
mod call_args;
mod collection;
mod durable_read;
mod entry;
mod env;
mod error;
pub mod evolution;
mod exec;
mod expr;
mod group_write;
mod host;
mod host_effects;
mod index_maintenance;
mod local_collection;
mod loop_exec;
mod neighbor;
mod path;
mod percent;
mod project_session;
mod range_expr;
mod read;
mod saved_iter;
mod statement;
mod std_audit;
mod std_csv;
mod std_error_helpers;
mod std_hash;
mod std_id;
mod std_json;
mod std_matrix;
mod std_pure;
mod std_random;
mod stdlib;
mod store;
mod surface;
mod transaction;
mod value;
mod write_dispatch;
mod write_plan;

pub use entry::{
    CheckedEntryCall, EntryArgument, EntryArgumentValue, EntryInvocation, EntryScalarArgument,
    evaluate_checked_read_only_expression, run_entry, run_entry_with_debugger, run_entry_with_host,
};
pub use error::{
    CALL_DEPTH_BUDGET, CallDepthFault, RUN_ABSENT, RUN_AMBIGUOUS_FUNCTION, RUN_ASSERT,
    RUN_CAPABILITY, RUN_DECIMAL_OVERFLOW, RUN_DEPTH, RUN_DIVIDE_BY_ZERO, RUN_ENTRY_ARGUMENT,
    RUN_ENTRY_SURFACE, RUN_NO_ENCLOSING_LOOP, RUN_NO_VALUE, RUN_OVERFLOW, RUN_PRIVATE_FUNCTION,
    RUN_STORE, RUN_TEMPORAL_OVERFLOW, RUN_TRAVERSAL, RUN_TYPE, RUN_UNBOUND_NAME,
    RUN_UNCAUGHT_THROW, RUN_UNKNOWN_FUNCTION, RUN_UNSUPPORTED, RuntimeError,
};
pub use host::{
    FixedNondeterminism, Frame, Host, LogSink, Nondeterminism, RunContext, StepHook,
    SystemNondeterminism,
};
pub use marrow_check::{
    ENTRY_PROTOCOL_TAG_VERSION, EntryArgumentShape, EntryDescriptor, EntryDescriptorError,
    EntryEnumMember, EntryIdentity, EntryIdentityKey, EntryParameter,
};
pub use project_session::{
    ProjectInvokeError, ProjectMode, ProjectOpen, ProjectSession, ProjectSessionError,
    ProjectSessionNotice, ProjectTestCase, SessionEntry, StoreStamp,
};
pub use surface::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_CONFLICT, SURFACE_CURSOR, SURFACE_INVALID_DATA,
    SURFACE_LIMIT, SURFACE_MAX_MATERIALIZED_BYTES, SURFACE_MAX_PAGE_LIMIT, SURFACE_MAX_VALUE_BYTES,
    SURFACE_REQUEST, SURFACE_STALE_CURSOR, SURFACE_STORE, SURFACE_WRITE, SurfaceCollectionPage,
    SurfaceCollectionPageRequest, SurfaceCollectionRead, SurfaceCollectionReadShape,
    SurfaceCursorBoundaryInputShape, SurfaceEnumValue, SurfaceError, SurfaceIdentityInputShape,
    SurfaceInputKeyShape, SurfaceNodeRead, SurfaceNodeReadShape, SurfacePageBoundary,
    SurfacePageCursor, SurfaceReadError, SurfaceReadField, SurfaceReadIdentity, SurfaceReadInput,
    SurfaceReadOperation, SurfaceReadOperationRef, SurfaceReadRecord, SurfaceUpdate,
    SurfaceUpdateField, SurfaceUpdateInput, SurfaceValue, read_surface_point,
    read_surface_singleton,
};
pub use value::{IdentityValue, RunOutput, RunOutputSink, Value};
pub use write_plan::{WriteDataSegment, WriteOp, WriteTarget};
