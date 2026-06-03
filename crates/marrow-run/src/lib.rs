//! The Marrow runtime: evaluate checked `.mw` functions.
//!
//! The evaluator runs functions over scalar values (integers, booleans,
//! strings) with locals, arithmetic/comparison/logical/`_` operators,
//! conditionals, `while`/`for` loops, interpolation, and calls between
//! functions. It reads saved data (fields and keyed-leaf entries) and writes it
//! through the managed-write layer (`^books(id).field = …`, `delete`, `append`),
//! groups writes in a `transaction` (commit/rollback with read-your-writes), and
//! provides the
//! `print`/`write`/`exists`/`nextId`/`append` builtins, the `?.` optional read
//! and `??` absence-default, the
//! `std::assert`/`std::text`/`std::math` library helpers, and the
//! `std::clock::now()` and `std::env` host capabilities. Whole-resource writes,
//! index traversal, and structured errors build on the same spine.
//!
//! The evaluator is carved into sibling modules along the call spine
//! (`expr`/`call`/`exec`/`read`/`write_dispatch`/`path`) plus its leaf
//! supports (`error`/`value`/`host`/`env`/`stdlib`/`collection`).

pub mod base64;
pub(crate) mod write;

mod activation;
mod call;
mod call_args;
mod collection;
mod durable_read;
mod entry;
mod env;
mod error;
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
mod read;
mod saved_iter;
mod statement;
mod std_pure;
mod stdlib;
mod store;
mod transaction;
mod value;
mod write_dispatch;
mod write_plan;

pub use entry::{CheckedEntryCall, run_entry, run_entry_with_debugger, run_entry_with_host};
pub use error::{
    RUN_ABSENT, RUN_AMBIGUOUS_FUNCTION, RUN_ASSERT, RUN_CAPABILITY, RUN_DECIMAL_OVERFLOW,
    RUN_DIVIDE_BY_ZERO, RUN_NO_ENCLOSING_LOOP, RUN_NO_VALUE, RUN_OVERFLOW, RUN_PRIVATE_FUNCTION,
    RUN_STORE, RUN_TRAVERSAL, RUN_TYPE, RUN_UNBOUND_NAME, RUN_UNCAUGHT_THROW, RUN_UNKNOWN_FUNCTION,
    RUN_UNSUPPORTED, RuntimeError,
};
pub use host::{Frame, Host, StepHook};
pub use value::{RunOutput, Value};
pub use write_plan::{WriteDataSegment, WriteOp, WriteTarget};
