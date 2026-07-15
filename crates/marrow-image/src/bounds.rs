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

/// Collection value types: distinct `List[T]` / `Map[K, V]` instantiations in the
/// COLLTYPES table. This bounds the number of *static* collection shapes an image
/// declares (each concrete instantiation is one row), not a collection's runtime
/// element count — the latter is a private VM bound (`MAX_COLLECTION_LEN`).
pub const MAX_COLLECTIONS: usize = 64;

/// Durable roots (0 or 1) and operation sites.
pub const MAX_ROOTS: usize = 1;
pub const MAX_SITES: usize = 64;

/// Managed indexes per durable root, and projected leaf components per index. Each
/// index projects an ordered leaf set (top-level fields and identity keys); the
/// component count is bounded independently of `MAX_KEY_COLUMNS` and `MAX_FIELDS`
/// since a projection may combine both. Both bounds keep the image and verifier
/// index decoders allocating within a fixed limit (§ law 9), comfortably above any
/// narrow index a personal-product journey needs.
pub const MAX_INDEXES: usize = 32;
pub const MAX_INDEX_COMPONENTS: usize = MAX_FIELDS + MAX_KEY_COLUMNS;

/// Steps in one operation site's semantic path: the application step, the root
/// placement step, and up to `MAX_DURABLE_DEPTH` nested member steps down to the
/// addressed node. The bound keeps the image and verifier site-path decoders
/// allocating within a fixed limit (§ law 9); a path shorter than two steps names
/// no graph node.
pub const MIN_SITE_PATH_STEPS: usize = 2;
pub const MAX_SITE_PATH_STEPS: usize = 2 + MAX_DURABLE_DEPTH;

/// Key columns per durable root or branch placement. A singleton root has zero;
/// a keyed placement has an ordered tuple of one or more columns. The bound keeps
/// every key-tuple decoder (image, verifier) allocating within a fixed limit
/// (§ law 9); eight columns is far above any composite key a personal-product
/// journey needs.
pub const MAX_KEY_COLUMNS: usize = 8;

/// Total durable-graph member records (fields, groups, and keyed branches, at
/// every nesting level) one root's identity tree may carry. A resource's durable
/// shape is a member tree — top-level fields plus static `group` namespaces and
/// keyed `branch` placements, each recursively holding its own members — and this
/// bound keeps the image and verifier member-tree decoders allocating within a
/// fixed limit (§ law 9), independently of `MAX_FIELDS` (which bounds one
/// materialized record's flat field list).
pub const MAX_DURABLE_MEMBERS: usize = 256;

/// Nesting depth of a durable field's stored value shape: a top-level field value
/// is depth 1, a struct leaf or an enum member payload leaf one deeper. The bound
/// stops a hostile image from driving unbounded recursion in the value-shape
/// decoder before it allocates (§ law 9), comfortably above any source-shaped
/// value nesting the checker's own acyclic value graph admits.
pub const MAX_DURABLE_VALUE_DEPTH: usize = 32;

/// Nesting depth of the durable-graph member tree: a top-level member is depth 1,
/// a member of a group or branch is one deeper. The bound stops a hostile or
/// divergent image from driving unbounded recursion in the member-tree decoder
/// before it allocates (§ law 9), comfortably above any source-shaped nesting the
/// parser's own depth limit admits.
pub const MAX_DURABLE_DEPTH: usize = 16;

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

/// The node budget for structurally expanding one export's wire transfer graph
/// (`crate::interface`). A record field or enum payload may itself be a record or
/// enum, so a *verified acyclic* value graph can still expand exponentially (a
/// diamond of many-fielded records). The `InterfaceId` derivation expands each
/// signature into its full structural transfer shape, so it bounds the total
/// expanded node count before it allocates (§ law 9) and rejects a signature that
/// exceeds this with a typed error rather than materializing an exponential tree.
pub const MAX_INTERFACE_TRANSFER_NODES: usize = 4096;
