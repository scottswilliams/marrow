//! ProgramImage v0 representational bounds.
//!
//! The encoder refuses a draft that exceeds them, and the independent verifier
//! rechecks each against the received bytes *before* it allocates, so a hostile image
//! cannot drive unbounded work.
//!
//! A bound is a decode-time allocation guard, never a stored-format byte: the image
//! encodes actual counts. Widening one is therefore monotone — every image a narrower
//! bound accepted a wider one still accepts, byte for byte — and needs no container or
//! profile version bump (see `docs/implementation/README.md` for the boundary that
//! would change that).

/// Whole-image byte ceiling. Sized to admit a [`MAX_RECORD_FIELDS`]-width durable
/// resource with headroom: at ~84 bytes per declared field the widest resource the
/// identity ledger permits encodes to ~343 KB.
pub const MAX_IMAGE_BYTES: usize = 512 * 1024;

/// Maximum string-pool entries and the byte length of any one entry. A wide resource
/// interns one string per declared field name; [`MAX_IMAGE_BYTES`] remains the true
/// bound on the pool's total bytes.
pub const MAX_STRINGS: usize = 8192;
pub const MAX_STRING_BYTES: usize = 4 * 1024;

/// Record types per image, and the top-level field width of one record type. Records,
/// `group` sub-records and monomorphized instantiations share the type count, so it
/// sits at the top of the u16-encoded family. Field-leaf sites are emitted per
/// *referenced* field, so declared width does not drive image bytes.
pub const MAX_TYPES: usize = 4096;
pub const MAX_RECORD_FIELDS: usize = 4096;

/// Dense inline-composite (`struct` value) leaf count: the flat leaves of one
/// materialized struct value. A value shape, not a resource's field set, so it
/// deliberately does not scale with [`MAX_RECORD_FIELDS`].
pub const MAX_STRUCT_LEAVES: usize = 64;

/// Closed enum value types, variants per enum, and dense payload leaves per
/// variant. User enums and every monomorphized `Option`/`Result`/generic instance
/// share the enum count.
pub const MAX_ENUMS: usize = 4096;
pub const MAX_VARIANTS: usize = 256;
pub const MAX_PAYLOAD_FIELDS: usize = 64;

/// Distinct `List<T>` / `Map<K, V>` instantiations in the COLLTYPES table — static
/// shapes, not a collection's runtime element count (a private VM bound).
pub const MAX_COLLECTIONS: usize = 4096;

/// Root placement *occurrences* per project, and operation sites.
///
/// `MAX_ROOTS` is deliberately not derived from [`MAX_TYPES`]: many roots may occur
/// over one Product, and monomorphization grows the type population with no root at
/// all. The identity ledger caps occurrences independently at 8192 anchor rows, so
/// 4096 binds. The site table holds the eager per-node sites plus one field-leaf site
/// per field the code *references*, so it tracks referenced fields, not declared
/// width.
pub const MAX_ROOTS: usize = 4096;
pub const MAX_SITES: usize = 8192;

/// Root occurrences an [`crate::AdmittedGraphInputPlan`] can admit into construction.
///
/// **The admitted-intake rule.** A plan bounds what may be *handed* to the durable
/// graph, so each term is exactly one past the bound whose refusal owner must keep its
/// refusal: construction publishes a complete graph at root N+1 and the encoder reports
/// [`crate::ImageBuildError::TooManyRoots`] over the whole of it.
pub const MAX_ADMITTED_ROOT_OCCURRENCES: usize = MAX_ROOTS + 1;

/// Product declarations an [`crate::AdmittedGraphInputPlan`] can admit. Derived: a
/// declaration with no root occurrence projects nothing, so admitted declarations
/// cannot exceed admitted occurrences.
pub const MAX_ADMITTED_PRODUCT_DECLARATIONS: usize = MAX_ADMITTED_ROOT_OCCURRENCES;

/// Managed indexes per durable root, and projected leaf components per index. The
/// component count is fixed and independent of [`MAX_RECORD_FIELDS`]: 64 projected
/// leaves plus a full [`MAX_KEY_COLUMNS`] key tuple.
pub const MAX_INDEXES: usize = 32;
pub const MAX_INDEX_COMPONENTS: usize = 72;

/// Steps in one operation site's semantic path: the application step, the root
/// placement step, and up to [`MAX_DURABLE_DEPTH`] nested member steps. A path shorter
/// than two steps names no graph node.
pub const MIN_SITE_PATH_STEPS: usize = 2;
pub const MAX_SITE_PATH_STEPS: usize = 2 + MAX_DURABLE_DEPTH;

/// Key columns per durable root or branch placement. A singleton root has zero; a
/// keyed placement has an ordered tuple of one or more.
pub const MAX_KEY_COLUMNS: usize = 8;

/// Total durable-graph member records — fields, groups and keyed branches at every
/// nesting level — one root's identity tree may carry. Every top-level field is a
/// member, so it admits a wide resource's field set plus its groups and branches.
pub const MAX_DURABLE_MEMBERS: usize = 8192;

/// Member commands one Product declaration may be handed. Derived: exactly one past
/// [`MAX_DURABLE_MEMBERS`], by the admitted-intake rule on
/// [`MAX_ADMITTED_ROOT_OCCURRENCES`], so an over-wide declaration reaches the
/// encoder's own refusal instead of being masked at the entry point.
pub const MAX_ADMITTED_DECLARATION_COMMANDS: usize = MAX_DURABLE_MEMBERS + 1;

/// Nesting depth of a durable field's stored value shape: a top-level field value is
/// depth 1, a struct leaf or an enum payload leaf one deeper. Stops a hostile image
/// from driving unbounded recursion in the value-shape decoder before it allocates.
pub const MAX_DURABLE_VALUE_DEPTH: usize = 32;

/// Nesting depth of the durable-graph member tree: a top-level member is depth 1, a
/// member of a group or branch one deeper. Stops unbounded recursion in the
/// member-tree decoder before it allocates.
pub const MAX_DURABLE_DEPTH: usize = 16;

/// Constant-pool entries.
pub const MAX_CONSTS: usize = 1024;

/// Functions, params per function, locals per frame, and code bytes per function.
/// Monomorphic functions, test entries and every monomorphized generic instance share
/// the function table.
pub const MAX_FUNCTIONS: usize = 4096;
pub const MAX_PARAMS: usize = 16;
pub const MAX_LOCALS: usize = 256;
pub const MAX_CODE_BYTES: usize = 64 * 1024;

/// Exported functions per image: the public entry surface `marrow check`/`run`/`test`
/// and the generated client address by stable id. Each export targets a distinct
/// function, so [`MAX_FUNCTIONS`] bounds it from above.
pub const MAX_EXPORTS: usize = 256;

/// Test entries (the closed non-wire TEST-ENTRY table). A test entry names a storeless
/// zero-argument function `marrow test` runs; it is never an export, interface, or
/// durable identity.
pub const MAX_TEST_ENTRIES: usize = 256;

/// The computed operand-stack depth ceiling (verifier-sealed, never read from the
/// image).
pub const MAX_STACK_DEPTH: usize = 256;

/// Text-concatenation result ceiling (a runtime bound).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// The largest `at most N` a bounded durable traversal may declare: the compile-time
/// count of immediate keys frozen per acquisition. The frozen keys materialize as one
/// ordinary `List[K]`, so they also obey the single collection aggregate-byte ceiling.
pub const MAX_TRAVERSAL_BOUND: u32 = 65_536;

/// The node budget for structurally expanding one export's wire transfer graph. A
/// verified acyclic value graph can still expand exponentially (a diamond of
/// many-fielded records), so `InterfaceId` derivation bounds the expanded node count
/// before it allocates.
pub(crate) const MAX_INTERFACE_TRANSFER_NODES: usize = 4096;

// Width-bound decoupling invariants, enforced at compile time: a future edit
// that re-couples the narrow bounds to the widened record field width — or drops a
// graph-scaled bound below it — fails the build here, not silently at runtime. The
// dense inline-composite leaf count and the index projection width must NOT scale
// with the record field width; the member-tree, site-table, and string-pool bounds
// must admit at least a wide resource's field set.
const _: () = {
    assert!(
        MAX_RECORD_FIELDS >= 2000,
        "record field width must admit the M-shaped declared width",
    );
    assert!(
        MAX_STRUCT_LEAVES < MAX_RECORD_FIELDS,
        "a dense composite leaf count must not scale with the record field width",
    );
    assert!(
        MAX_INDEX_COMPONENTS < MAX_RECORD_FIELDS,
        "an index projection must not scale with the record field width",
    );
    assert!(
        MAX_INDEX_COMPONENTS >= MAX_KEY_COLUMNS,
        "an index projection may still combine a full composite key tuple",
    );
    assert!(
        MAX_DURABLE_MEMBERS >= MAX_RECORD_FIELDS,
        "every top-level field is a member; the member tree must admit a wide field set",
    );
    assert!(
        MAX_SITES >= MAX_RECORD_FIELDS,
        "a program may reference every field of a wide resource, minting a leaf site each; \
         the site table must admit a wide field set as its worst case",
    );
    assert!(
        MAX_STRINGS > MAX_RECORD_FIELDS,
        "each field interns a name; the string pool must admit a wide field set",
    );
    assert!(
        MAX_EXPORTS <= MAX_FUNCTIONS,
        "each export targets a distinct function; the function table bounds the export count",
    );
};

// Encoded-width derivations, enforced at compile time.
//
// Every count these bounds guard is spelled in the image bytes as a fixed-width
// big-endian integer, and the encoder narrows the `usize` row count to that width after
// the policy walk has refused a draft above the bound. The narrowing is lossless exactly
// when the bound fits the width it is spelled in, so widening a bound past that width
// fails the build here instead of truncating a count in an emitted image.
//
// The two platform assertions carry the rest of the crate's conversions.
const _: () = {
    assert!(
        usize::BITS >= u32::BITS,
        "a u16/u32 index or id widens to usize losslessly",
    );
    assert!(
        usize::BITS <= u64::BITS,
        "a byte length narrows to the u64 length delimiter of an identity preimage losslessly",
    );

    // Counts spelled `u16` in the image tables, the durable graph, or an identity
    // preimage.
    assert!(MAX_STRINGS <= u16::MAX as usize, "STRINGS count is u16");
    assert!(
        MAX_STRING_BYTES <= u16::MAX as usize,
        "a pool entry's byte length is u16",
    );
    assert!(MAX_CONSTS <= u16::MAX as usize, "CONSTS count is u16");
    assert!(MAX_TYPES <= u16::MAX as usize, "TYPES count is u16");
    assert!(
        MAX_RECORD_FIELDS <= u16::MAX as usize,
        "the field count is u16-encoded in every table",
    );
    assert!(MAX_ENUMS <= u16::MAX as usize, "ENUMS count is u16");
    assert!(
        MAX_VARIANTS <= u16::MAX as usize,
        "an enum's variant count is u16",
    );
    assert!(
        MAX_COLLECTIONS <= u16::MAX as usize,
        "COLLTYPES count is u16",
    );
    assert!(
        MAX_ROOTS <= u16::MAX as usize,
        "the durable root-occurrence count is u16",
    );
    assert!(
        MAX_DURABLE_MEMBERS <= u16::MAX as usize,
        "a member run's count is u16",
    );
    // The intake ceiling, not the bound, is what a member ordinal is checked against, so
    // it is the value that must fit the carrier.
    assert!(
        MAX_ADMITTED_DECLARATION_COMMANDS <= u16::MAX as usize,
        "the admitted member-command intake ceiling is u16",
    );
    assert!(
        MAX_KEY_COLUMNS <= u16::MAX as usize,
        "a key tuple's column count is u16",
    );
    assert!(
        MAX_INDEXES <= u16::MAX as usize,
        "a root's managed-index count is u16",
    );
    assert!(
        MAX_INDEX_COMPONENTS <= u16::MAX as usize,
        "an index's projected-component count is u16",
    );
    assert!(MAX_SITES <= u16::MAX as usize, "the site-row count is u16");
    assert!(
        MAX_FUNCTIONS <= u16::MAX as usize,
        "FUNCTIONS count is u16, and a function index is a u16 operand",
    );
    assert!(MAX_EXPORTS <= u16::MAX as usize, "EXPORTS count is u16");
    assert!(
        MAX_TEST_ENTRIES <= u16::MAX as usize,
        "TEST-ENTRY count is u16",
    );
    assert!(
        MAX_LOCALS <= u16::MAX as usize,
        "a frame's local count is u16, and a local slot is a u16 operand",
    );

    // Counts spelled in a single byte.
    assert!(
        MAX_PARAMS <= u8::MAX as usize,
        "a function's parameter count is one byte",
    );
    assert!(
        MAX_PAYLOAD_FIELDS <= u8::MAX as usize,
        "a variant's payload-leaf count is one byte in the ENUMS table",
    );
    assert!(
        MAX_SITE_PATH_STEPS <= u8::MAX as usize,
        "a site path's step count is one byte",
    );
};

/// A growing `Vec` is live at three times its admitted length: the amortized capacity
/// slack, plus the buffer it still holds while it copies into its successor. Stated
/// once here so every accounting that charges growth reads one number.
pub const GROWTH_AND_COPY: u64 = 3;

#[cfg(test)]
mod tests {
    use super::MAX_IMAGE_BYTES;

    /// The byte-exact ceiling corpora in `tests/ceiling_boundary.rs` are stated in the
    /// constant and move with it, so this literal is the one visible diff a changed
    /// ceiling must make. The table counts are pinned by literal corpora in
    /// `tests/bound_boundaries.rs`.
    #[test]
    fn the_image_byte_ceiling_is_its_published_value() {
        assert_eq!(MAX_IMAGE_BYTES, 512 * 1024);
    }
}
