//! ProgramImage v0 representational bounds (design §E).
//!
//! These constants size the container to the T01 subset. The encoder refuses to
//! build a draft that exceeds them, and the independent verifier rechecks each
//! bound against the received bytes *before* it allocates, so a hostile image can
//! never drive unbounded work. Widening any of these is a later lane's decision,
//! recorded with its own known-answer coverage.
//!
//! A bound is a decode-time allocation guard, not a stored-format byte: the profile
//! descriptor and the image byte layout encode *actual* `u16` counts, never a bound
//! constant (see `marrow_kernel::durable::profile`). Widening a bound therefore
//! needs no image-container or profile version bump today, even though it enlarges
//! the accepted-image set. The widen is safe because it is **monotone** — every
//! image a narrower bound accepted a wider one still accepts, byte-for-byte — so an
//! old toolchain meeting a new image either accepts it unchanged or refuses it with
//! a typed bound rejection (it never misreads bytes), and no released consumer pins
//! these v0 values. Forward note: once images cross a trust or version boundary
//! (signed artifacts, cross-node acceptance, a capability-gated profile), a bound
//! widen becomes an acceptance-set change that a version or capability descriptor
//! must record, because a peer's acceptance can no longer be assumed to track this
//! toolchain's.

/// Whole-image byte ceiling. Sized to admit a [`MAX_RECORD_FIELDS`]-width durable
/// resource with headroom: at a measured 83.82 bytes per declared durable field, the
/// widest resource the durable identity ledger permits (~4091 fields) encodes to
/// ~343 KB, so 512 KiB carries it with ~1.5× headroom. Widening from the v0 256 KiB
/// waypoint is monotone — every image the narrower ceiling accepted the wider one
/// still accepts, byte-for-byte.
pub const MAX_IMAGE_BYTES: usize = 512 * 1024;

/// Maximum string-pool entries and the byte length of any one entry. A wide
/// resource interns one string per declared field name, so the M-shaped workload's
/// thousands of fields drive the pool past a narrow count; this bounds the *count*
/// with headroom while [`MAX_IMAGE_BYTES`] remains the true allocation bound on the
/// pool's total bytes (§ law 9). The per-entry byte length is unchanged.
pub const MAX_STRINGS: usize = 8192;
pub const MAX_STRING_BYTES: usize = 4 * 1024;

/// Record types per image, and the top-level field width of one record type. The
/// record-type count admits a production-representative type population: a large
/// deployment carries thousands of distinct records (resources, dense `struct`
/// values, `group` sub-records, and monomorphized generic instantiations all consume
/// a slot), so the count sits at the top of the u16-encoded family rather than the
/// v0 waypoint of 64. A durable resource's declared field set is bounded by
/// [`MAX_RECORD_FIELDS`]: the M-shaped workload declares thousands of mostly-sparse
/// fields, so the width admits the full count with headroom, staying within the u16
/// field-count encoding. This is deliberately distinct from the dense inline-composite
/// leaf count ([`MAX_STRUCT_LEAVES`]) and the index projection width
/// ([`MAX_INDEX_COMPONENTS`]): neither scales with the record field width.
///
/// This width is the count guard; [`MAX_IMAGE_BYTES`] is the binding ceiling for a
/// *durable* resource. Because the compiler emits one operation site per stored
/// field, a durable root costs ~84 image bytes per declared field (measured 83.82
/// bytes/field, stable across 2000–4091 fields), so the 512 KiB whole-image ceiling
/// admits ~6200 durable fields — comfortable headroom over this width guard. The
/// widest durable resource that actually compiles is bounded first by the durable
/// identity ledger (`marrow_project::ids::MAX_IDS_ROWS` = 4096 anchor rows): a
/// resource of ~4091 fields uses ~4095 ledger rows and encodes to ~343 KB, admitted
/// with headroom. A bare record *type* with no durable root (no per-field sites)
/// reaches the full 4096 field width far below the byte ceiling. The per-field image
/// cost — eager per-field site emission — is the coupling a later representation lane
/// can retire to lift the durable ceiling further.
pub const MAX_TYPES: usize = 4096;
pub const MAX_RECORD_FIELDS: usize = 4096;

/// Dense inline-composite (`struct` value) leaf count: the flat leaves of one
/// materialized struct value, in declaration order. A dense composite is a value
/// shape, not a durable resource's field set, so it stays narrow and does NOT
/// scale with [`MAX_RECORD_FIELDS`]. The verifier and encoder recheck this bound
/// independently of the record field width.
pub const MAX_STRUCT_LEAVES: usize = 64;

/// Closed enum value types, variants per enum, and dense scalar payload fields
/// per variant. A flat enum's variants are its selectable members; each carries
/// at most `MAX_PAYLOAD_FIELDS` bare-scalar payload leaves in declaration order. The
/// enum-type count joins the record-type count at the top of the u16 family: user
/// enums plus every monomorphized `Option`/`Result`/generic-enum instantiation
/// consume a slot, so a production type population needs thousands.
pub const MAX_ENUMS: usize = 4096;
pub const MAX_VARIANTS: usize = 256;
pub const MAX_PAYLOAD_FIELDS: usize = 64;

/// Collection value types: distinct `List<T>` / `Map<K, V>` instantiations in the
/// COLLTYPES table. This bounds the number of *static* collection shapes an image
/// declares (each concrete instantiation is one row), not a collection's runtime
/// element count — the latter is a private VM bound (`MAX_COLLECTION_LEN`). The count
/// joins the widened type family; a production program's distinct collection shapes
/// scale with its type population.
pub const MAX_COLLECTIONS: usize = 4096;

/// Durable roots per project and operation sites. A project declares a store root per
/// durable resource it serves — a multi-global application (its ledger root beside its
/// counter root beside its catalog root) is the ordinary shape — so the count admits a
/// family of roots rather than one. The value tracks the type family
/// ([`MAX_TYPES`]/[`MAX_ENUMS`]/[`MAX_FUNCTIONS`]): each root's resource is a record
/// type, so [`MAX_TYPES`] already bounds the root count from above (the `MAX_ROOTS <=
/// MAX_TYPES` invariant below), and matching it keeps one obvious ceiling rather than a
/// second arbitrary one — so raising the type family raises this in lockstep.
/// [`MAX_IMAGE_BYTES`] remains the true byte bound on the emitted graph.
///
/// Widening this from the T01 value of 1 is monotone: a bound is a decode-time
/// allocation guard, never a stored-format byte (the durable graph encodes its actual
/// `u16` root count), so every image the narrow bound accepted a wider one still accepts
/// byte-for-byte, and no image-container or profile version bump is required (see the
/// module header). The verifier rechecks `root_count <= MAX_ROOTS` against the received
/// bytes before it allocates the root vector, so a hostile image claiming more roots is
/// refused with a typed bound rejection, not misread.
///
/// The site table is emitted from the durable graph, not per operation: one
/// whole-payload site per keyed placement and one field-leaf site per stored field (plus
/// group, branch, and index sites), so it scales with the graph's field width rather
/// than with code. A wide resource therefore mints a site per declared field, so
/// [`MAX_SITES`] must admit at least a wide resource's field set ([`MAX_RECORD_FIELDS`])
/// plus its group, branch, and index sites. The value carries that with headroom;
/// [`MAX_IMAGE_BYTES`] remains the true byte bound on the emitted site paths.
pub const MAX_ROOTS: usize = 4096;
pub const MAX_SITES: usize = 8192;

/// Managed indexes per durable root, and projected leaf components per index. Each
/// index projects a small ordered leaf set (top-level fields and identity keys) for
/// a narrow lookup. The component count is deliberately fixed and independent of the
/// widened record field width: widening [`MAX_RECORD_FIELDS`] must NOT let an index
/// project thousands of components. The value (72) preserves the T01 admissible
/// ceiling — up to 64 projected leaf components plus a full [`MAX_KEY_COLUMNS`]
/// (8-column) key tuple, the previous `64 + MAX_KEY_COLUMNS` ceiling kept
/// deliberately — and stays far below any resource's declared field set, keeping the
/// image and verifier index decoders allocating within a fixed limit (§ law 9).
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
/// The function count admits a production-representative program: monomorphic
/// functions, test entries, and every monomorphized generic-function instance share
/// this table, so a large deployment carries thousands. Params, locals, and code
/// bytes per function are unchanged single-function shape bounds.
pub const MAX_FUNCTIONS: usize = 4096;
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
        MAX_RECORD_FIELDS <= u16::MAX as usize,
        "the field count is u16-encoded in every table",
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
        "every stored field mints a site; the site table must admit a wide field set",
    );
    assert!(
        MAX_STRINGS > MAX_RECORD_FIELDS,
        "each field interns a name; the string pool must admit a wide field set",
    );
    assert!(
        MAX_ROOTS <= MAX_TYPES,
        "each root's resource is a record type; the type table bounds the root count",
    );
};

#[cfg(test)]
mod tests {
    //! Width-bound known-answer tests: the chosen value of each widened or
    //! re-derived width constant. The *decoupling* relationships are enforced at
    //! compile time by the `const _` block above.
    use super::*;

    #[test]
    fn width_constants_hold_their_chosen_values() {
        assert_eq!(MAX_RECORD_FIELDS, 4096, "top-level record field width");
        assert_eq!(MAX_STRUCT_LEAVES, 64, "dense inline-composite leaf count");
        assert_eq!(MAX_INDEX_COMPONENTS, 72, "index projection width");
        assert_eq!(MAX_DURABLE_MEMBERS, 8192, "durable member-tree total");
        assert_eq!(MAX_SITES, 8192, "operation-site table");
        assert_eq!(MAX_STRINGS, 8192, "string-pool entries");
        assert_eq!(MAX_ROOTS, 4096, "durable roots per project");
    }

    /// The widened type/function family: the evidence-widened scale floor raised each
    /// count from the v0 waypoint of 64 to the top of the u16-encoded family, and the
    /// whole-image byte ceiling in lockstep to admit a wide durable resource.
    #[test]
    fn scale_floor_family_holds_its_widened_values() {
        assert_eq!(MAX_TYPES, 4096, "record types per image");
        assert_eq!(MAX_ENUMS, 4096, "enum types per image");
        assert_eq!(MAX_FUNCTIONS, 4096, "functions per image");
        assert_eq!(MAX_COLLECTIONS, 4096, "collection value types per image");
        assert_eq!(
            MAX_ROOTS, MAX_TYPES,
            "root count tracks the record-type count"
        );
        assert_eq!(MAX_IMAGE_BYTES, 512 * 1024, "whole-image byte ceiling");
    }
}
