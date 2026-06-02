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
//! supports (`error`/`value`/`host`/`env`/`stdlib`/`collection`/`schema_query`).

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
