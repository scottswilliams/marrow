//! ProgramImage v0 representational bounds (design §E).
//!
//! These constants size the container to the T01 subset. The encoder refuses to
//! build a draft that exceeds them, and the independent verifier rechecks each
//! bound against the received bytes *before* it allocates, so a hostile image can
//! never drive unbounded work. Widening any of these is a later lane's decision,
//! recorded with its own known-answer coverage.

/// Whole-image byte ceiling.
pub const MAX_IMAGE_BYTES: usize = 256 * 1024;

/// Maximum string-pool entries and the byte length of any one entry. A wide
/// resource interns one string per declared field name, so the M-shaped workload's
/// thousands of fields drive the pool past a narrow count; this bounds the *count*
/// with headroom while [`MAX_IMAGE_BYTES`] remains the true allocation bound on the
/// pool's total bytes (§ law 9). The per-entry byte length is unchanged.
pub const MAX_STRINGS: usize = 8192;
pub const MAX_STRING_BYTES: usize = 4 * 1024;

/// Record types per image, and the top-level field width of one record type. A
/// durable resource's declared field set is bounded by [`MAX_RECORD_FIELDS`]: the
/// M-shaped workload declares thousands of mostly-sparse fields, so the width
/// admits thousands with headroom, staying within the u16 field-count encoding and
/// [`MAX_IMAGE_BYTES`]. This is deliberately distinct from the dense
/// inline-composite leaf count ([`MAX_STRUCT_LEAVES`]) and the index projection
/// width ([`MAX_INDEX_COMPONENTS`]): neither scales with the record field width.
pub const MAX_TYPES: usize = 64;
pub const MAX_RECORD_FIELDS: usize = 4096;

/// Dense inline-composite (`struct` value) leaf count: the flat leaves of one
/// materialized struct value, in declaration order. A dense composite is a value
/// shape, not a durable resource's field set, so it stays narrow and does NOT
/// scale with [`MAX_RECORD_FIELDS`]. The verifier and encoder recheck this bound
/// independently of the record field width.
pub const MAX_STRUCT_LEAVES: usize = 64;

/// Closed enum value types, variants per enum, and dense scalar payload fields
/// per variant. A flat enum's variants are its selectable members; each carries
/// at most `MAX_PAYLOAD_FIELDS` bare-scalar payload leaves in declaration order.
pub const MAX_ENUMS: usize = 64;
pub const MAX_VARIANTS: usize = 256;
pub const MAX_PAYLOAD_FIELDS: usize = 64;

/// Collection value types: distinct `List<T>` / `Map<K, V>` instantiations in the
/// COLLTYPES table. This bounds the number of *static* collection shapes an image
/// declares (each concrete instantiation is one row), not a collection's runtime
/// element count — the latter is a private VM bound (`MAX_COLLECTION_LEN`).
pub const MAX_COLLECTIONS: usize = 64;

/// Durable roots (0 or 1) and operation sites. The site table is emitted from the
/// durable graph, not per operation: one whole-payload site per keyed placement and
/// one field-leaf site per stored field (plus group, branch, and index sites), so
/// it scales with the graph's field width rather than with code. A wide resource
/// therefore mints a site per declared field, so [`MAX_SITES`] must admit at least a
/// wide resource's field set ([`MAX_RECORD_FIELDS`]) plus its group, branch, and
/// index sites. The value carries that with headroom; [`MAX_IMAGE_BYTES`] remains
/// the true byte bound on the emitted site paths.
pub const MAX_ROOTS: usize = 1;
pub const MAX_SITES: usize = 8192;

/// Managed indexes per durable root, and projected leaf components per index. Each
/// index projects a small ordered leaf set (top-level fields and identity keys) for
/// a narrow lookup. The component count is deliberately fixed and independent of the
/// widened record field width: widening [`MAX_RECORD_FIELDS`] must NOT let an index
/// project thousands of components. The value preserves the T01 admissible ceiling —
/// a handful of projected fields plus a full composite key tuple — and stays far
/// below any resource's declared field set, keeping the image and verifier index
/// decoders allocating within a fixed limit (§ law 9).
pub const MAX_INDEXES: usize = 32;
pub const MAX_INDEX_COMPONENTS: usize = 72;

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
/// fixed limit (§ law 9). Every top-level field is a member, so the member-tree
/// total must admit at least a wide resource's declared field set
/// ([`MAX_RECORD_FIELDS`]); the value carries that plus headroom for a resource's
/// groups, keyed branches, and their own members. It is a deliberate widen — the
/// M-shaped workload's members exceed a narrow tree — not a silent one.
pub const MAX_DURABLE_MEMBERS: usize = 8192;

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

/// The largest `at most N` bound a bounded durable traversal
/// (`DurIterateBounded`) may declare. `N` is the compile-time count of immediate
/// keys frozen per acquisition; the verifier rejects a zero or larger bound before
/// the runtime allocates the frozen key list (§ law 9). It caps the element *count*
/// at the VM's private `MAX_COLLECTION_LEN`. The frozen keys materialize as one
/// ordinary bounded `List[K]`, so they are additionally subject to the same
/// aggregate-byte ceiling (`MAX_AGGREGATE_BYTES`) every list obeys — a traversal over
/// wide keys can reach that byte ceiling (a deterministic `run.collection_limit`
/// fault) at fewer than `N` keys. There is one collection ceiling, not a second
/// traversal-specific one.
pub const MAX_TRAVERSAL_BOUND: u32 = 65_536;

/// The node budget for structurally expanding one export's wire transfer graph
/// (`crate::interface`). A record field or enum payload may itself be a record or
/// enum, so a *verified acyclic* value graph can still expand exponentially (a
/// diamond of many-fielded records). The `InterfaceId` derivation expands each
/// signature into its full structural transfer shape, so it bounds the total
/// expanded node count before it allocates (§ law 9) and rejects a signature that
/// exceeds this with a typed error rather than materializing an exponential tree.
pub const MAX_INTERFACE_TRANSFER_NODES: usize = 4096;

#[cfg(test)]
mod tests {
    //! Width-bound known-answer tests (WR01). These pin each width constant to its
    //! chosen value and, critically, pin the *decoupling*: widening the top-level
    //! record field width must never drag the dense inline-composite leaf count or
    //! the index projection width up with it. A future edit that re-couples them (or
    //! silently bumps the narrow bounds) fails here.
    use super::*;

    /// The M-shaped record field width admits thousands of declared fields with
    /// headroom, and stays inside the u16 field-count encoding every table uses.
    #[test]
    fn record_field_width_admits_thousands_within_the_u16_encoding() {
        assert_eq!(MAX_RECORD_FIELDS, 4096);
        assert!(MAX_RECORD_FIELDS >= 2000, "admits the M-shaped declared width");
        assert!(
            MAX_RECORD_FIELDS <= u16::MAX as usize,
            "the field count is u16-encoded in every table",
        );
    }

    /// The dense inline-composite (`struct` value) leaf count stays narrow and did
    /// NOT widen with the record field width: it is a value shape, not a resource's
    /// declared field set.
    #[test]
    fn struct_leaf_count_stayed_narrow_and_decoupled() {
        assert_eq!(MAX_STRUCT_LEAVES, 64);
        assert!(
            MAX_STRUCT_LEAVES < MAX_RECORD_FIELDS,
            "a dense composite leaf count must not scale with the record field width",
        );
    }

    /// The index projection width is deliberately fixed and did NOT widen with the
    /// record field width: an index projects a small ordered leaf set, never
    /// thousands of components.
    #[test]
    fn index_component_width_stayed_narrow_and_decoupled() {
        assert_eq!(MAX_INDEX_COMPONENTS, 72);
        assert!(
            MAX_INDEX_COMPONENTS < MAX_RECORD_FIELDS,
            "an index projection must not scale with the record field width",
        );
        assert!(
            MAX_INDEX_COMPONENTS >= MAX_KEY_COLUMNS,
            "a projection may still combine a full composite key tuple",
        );
    }

    /// The durable member-tree total and the operation-site table both scale with the
    /// graph, so both must admit at least a wide resource's field set (every top-level
    /// field is one member and one field-leaf site).
    #[test]
    fn member_tree_and_site_table_admit_a_wide_field_set() {
        assert!(MAX_DURABLE_MEMBERS >= MAX_RECORD_FIELDS);
        assert!(MAX_SITES >= MAX_RECORD_FIELDS);
    }

    /// The string pool admits one interned name per declared field of a wide
    /// resource, plus headroom for type, function, and module names.
    #[test]
    fn string_pool_admits_a_wide_resources_field_names() {
        assert!(MAX_STRINGS > MAX_RECORD_FIELDS);
    }
}
