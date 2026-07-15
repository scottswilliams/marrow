//! ProgramImage v0 representational bounds (design §E).
//!
//! These constants size the container to the T01 subset. The encoder refuses to
//! build a draft that exceeds them, and the independent verifier rechecks each
//! bound against the received bytes *before* it allocates, so a hostile image can
//! never drive unbounded work. Widening any of these is a later lane's decision,
//! recorded with its own known-answer coverage.

/// Whole-image byte ceiling.
pub const MAX_IMAGE_BYTES: usize = 256 * 1024;

/// Maximum string-pool entries and the byte length of any one entry.
pub const MAX_STRINGS: usize = 1024;
pub const MAX_STRING_BYTES: usize = 4 * 1024;

/// Record types and fields per record. A project's type table holds its dense
/// `struct` value types alongside the optional durable resource record; the
/// durable graph still admits at most one root (`MAX_ROOTS`), which references
/// exactly one of these types.
pub const MAX_TYPES: usize = 64;
pub const MAX_FIELDS: usize = 64;

/// Closed enum value types, variants per enum, and dense scalar payload fields
/// per variant. A flat enum's variants are its selectable members; each carries
/// at most `MAX_PAYLOAD_FIELDS` bare-scalar payload leaves in declaration order.
pub const MAX_ENUMS: usize = 64;
pub const MAX_VARIANTS: usize = 256;
pub const MAX_PAYLOAD_FIELDS: usize = 64;

/// Durable roots (0 or 1) and operation sites.
pub const MAX_ROOTS: usize = 1;
pub const MAX_SITES: usize = 64;

/// Constant-pool entries.
pub const MAX_CONSTS: usize = 1024;

/// Functions, params per function, locals per frame, and code bytes per function.
pub const MAX_FUNCTIONS: usize = 64;
pub const MAX_PARAMS: usize = 16;
pub const MAX_LOCALS: usize = 256;
pub const MAX_CODE_BYTES: usize = 64 * 1024;

/// Exports.
pub const MAX_EXPORTS: usize = 32;

/// Test entries (the closed non-wire TEST-ENTRY table). A test entry names a
/// storeless zero-argument function `marrow test` runs; it is never an export,
/// interface, or durable identity.
pub const MAX_TEST_ENTRIES: usize = 256;

/// The computed operand-stack depth ceiling (verifier-sealed, never read from
/// the image).
pub const MAX_STACK_DEPTH: usize = 256;

/// Text-concatenation result ceiling (runtime bound, design §D `TextConcat`).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
