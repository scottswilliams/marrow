//! The Marrow runtime: evaluate checked `.mw` functions.
//!
//! The evaluator runs functions over scalar values (integers, booleans,
//! strings) with locals, arithmetic/comparison/logical/`_` operators,
//! conditionals, `while`/`for` loops, interpolation, and calls between
//! functions. It reads saved data (fields and keyed-leaf entries) and writes it
//! through the managed-write layer (`^books(id).field = …`, `delete`, `append`),
//! groups writes in a `transaction` (commit/rollback with read-your-writes),
//! guards a block with `lock` (a scope released on every exit under the
//! single-writer profile), and provides the
//! `print`/`write`/`exists`/`nextId`/`append` builtins, the `?.` optional read
//! and `??` absence-default, the
//! `std::assert`/`std::text`/`std::math` library helpers, and the
//! `std::clock::now()` and `std::env` host capabilities. Whole-resource writes,
//! `merge`, index traversal, and structured errors build on the same spine.
//!
//! The evaluator is carved into sibling modules along the call spine
//! (`expr`/`call`/`exec`/`read`/`write_dispatch`/`path`) plus its leaf
//! supports (`error`/`value`/`host`/`env`/`stdlib`/`collection`/`schema_query`).
//! Every cross-module reference is a plain crate-internal call; the modules are
//! flattened back into the crate root so each one reaches the rest through a
//! single glob.

pub(crate) use std::cell::RefCell;
pub(crate) use std::cmp::Ordering;
pub(crate) use std::collections::HashMap;
pub(crate) use std::rc::Rc;
pub(crate) use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) use marrow_check::{
    CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, Def, DefItem, FileId, MarrowType,
    Resolution, ResolvableKind, resolve,
};
pub(crate) use marrow_schema::stdlib::Capability;
pub(crate) use marrow_schema::{
    Element, EnumSchema, IndexSchema, KeyDef, MemberPathResolution, Node, ResourceSchema, Type,
};
pub(crate) use marrow_store::Decimal;
pub(crate) use marrow_store::backend::{Backend, Presence, StoreError};
pub(crate) use marrow_store::mem::MemStore;
pub(crate) use marrow_store::path::{
    ChildSegment, PathSegment, SavedKey, decode_path, encode_path,
};
pub(crate) use marrow_store::value::{
    SavedValue, ScalarType, ValueError, decode_value, encode_value,
};
pub(crate) use marrow_syntax::{
    ArgMode, Argument, BinaryOp, Block, Expression, ForBinding, FunctionDecl, InterpolationPart,
    LiteralKind, MatchArm, ParamMode, SourceSpan, Statement, UnaryOp, duration_unit_seconds,
};
pub(crate) use write::{
    ResourceValue, SuppliedIdentity, WRITE_RAW_DECLARED_FIELD, WRITE_RAW_REQUIRES_MAINTENANCE,
    WRITE_REQUIRED_FIELD, WRITE_REQUIRES_MAINTENANCE, WriteError, WritePlan, decode_identity,
    next_id, next_layer_pos, plan_field_delete, plan_field_write, plan_identity_field_write,
    plan_layer_group_write, plan_layer_identity_leaf_write, plan_layer_leaf_write,
    plan_layer_merge, plan_nested_field_write, plan_nested_identity_field_write,
    plan_resource_delete, plan_resource_merge, plan_resource_write,
    validate_required_fields_after_field_write,
};

pub mod base64;
pub(crate) mod write;
#[cfg(test)]
mod write_tests;

mod call;
mod collection;
mod env;
mod error;
mod exec;
mod expr;
mod host;
mod path;
mod read;
mod schema_query;
mod stdlib;
mod value;
mod write_dispatch;

pub(crate) use call::*;
pub(crate) use collection::*;
pub(crate) use env::*;
pub(crate) use error::*;
pub(crate) use exec::*;
pub(crate) use expr::*;
pub(crate) use path::*;
pub(crate) use read::*;
pub(crate) use schema_query::*;
pub(crate) use stdlib::*;
pub(crate) use value::*;
pub(crate) use write_dispatch::*;

pub use call::{evaluate_function, run_entry, run_entry_with_debugger, run_entry_with_host};
pub use error::{
    RUN_ABSENT, RUN_ASSERT, RUN_CAPABILITY, RUN_DECIMAL_OVERFLOW, RUN_DIVIDE_BY_ZERO,
    RUN_NO_ENCLOSING_LOOP, RUN_NO_VALUE, RUN_OVERFLOW, RUN_PRIVATE_FUNCTION, RUN_STORE,
    RUN_TRAVERSAL, RUN_TYPE, RUN_UNBOUND_NAME, RUN_UNCAUGHT_THROW, RUN_UNKNOWN_FUNCTION,
    RUN_UNSUPPORTED, RuntimeError,
};
pub use host::{Frame, Host, StepHook};
pub use schema_query::{SavedPathClass, classify_saved_path, identity_leaf_key_mismatch};
pub use value::{RunOutput, Value};
pub use write::{WriteOp, decode_identity_arity};
