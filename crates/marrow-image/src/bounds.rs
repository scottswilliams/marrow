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
/// This width is the count guard. Field-leaf operation sites are emitted lazily — one
/// per field the code actually addresses — so a durable resource's image cost tracks its
/// *referenced* fields, not its declared width: a wide, mostly-sparse resource whose code
/// touches a handful of fields encodes a handful of field sites (each ~52 bytes). Declared
/// width therefore no longer drives image bytes, and [`MAX_IMAGE_BYTES`] is not the binder
/// at the full field guard. The durable-identity ledger
/// (`marrow_project::ids::MAX_IDS_ROWS` = 8192 anchor rows) admits the full 4096 field
/// width for a single wide resource (~4100 rows: one `Field` per field plus
/// application/product/root/key overhead) with headroom, so the binder at the full guard
/// is this field-count width itself, not the ledger or the byte ceiling. A bare record
/// *type* with no durable root reaches the full 4096 field width trivially.
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
/// family of roots rather than one.
///
/// `MAX_ROOTS` is an **occurrence** ceiling: root placement occurrences in one project.
/// It is justified on its own terms, not by the type population. It is the decode-time
/// allocation guard the verifier applies to the received `u16` root count before it
/// allocates the root vector, and the durable-identity ledger caps root occurrences
/// independently — `marrow_project::ids::MAX_IDS_ROWS` is 8192 anchor rows, of which a
/// project with any root already spends one on its application and one on the Product,
/// so even the most favourable census (every other term zero) admits at most 8,190 root
/// rows. 4096 is therefore the binding occurrence ceiling, and [`MAX_IMAGE_BYTES`]
/// remains the true byte bound on the emitted graph.
///
/// It is deliberately **not** derived from [`MAX_TYPES`], which is a type-population
/// ceiling — declared records, monomorphized instantiations, and one entry record per
/// durable Branch declaration. Neither bound proves the other in either direction: many
/// roots may occur over a single Product, contributing one record type between them,
/// while monomorphization and ordinary struct declarations grow the type population with
/// no durable root at all. A later widening of either must carry its own accepted-set,
/// identity, site, string, byte, and capacity evidence rather than inheriting the
/// other's.
///
/// Widening this from the T01 value of 1 is monotone: a bound is a decode-time
/// allocation guard, never a stored-format byte (the durable graph encodes its actual
/// `u16` root count), so every image the narrow bound accepted a wider one still accepts
/// byte-for-byte, and no image-container or profile version bump is required (see the
/// module header). The verifier rechecks `root_count <= MAX_ROOTS` against the received
/// bytes before it allocates the root vector, so a hostile image claiming more roots is
/// refused with a typed bound rejection, not misread.
///
/// The site table holds the eager per-node sites — one whole-payload site per keyed
/// placement plus group and index sites — and one field-leaf site per field the code
/// *references* (field-leaf sites are allocated lazily and deduplicated). Its size
/// therefore tracks referenced fields, not declared width. [`MAX_SITES`] is a conservative
/// worst-case ceiling that still admits a program addressing a wide resource's whole field
/// set ([`MAX_RECORD_FIELDS`]) plus its group, branch, and index sites; a program touching
/// few fields uses far fewer. [`MAX_IMAGE_BYTES`] remains the true byte bound on the
/// emitted site paths.
pub const MAX_ROOTS: usize = 4096;
pub const MAX_SITES: usize = 8192;

/// Root occurrences an [`crate::AdmittedGraphInputPlan`] can admit into construction.
///
/// **The admitted-intake rule.** A plan bounds what may be *handed* to the durable graph;
/// it does not decide what an image may *hold*. Each of its terms is therefore exactly one
/// past the bound whose refusal owner must keep its refusal — never at that bound, which
/// would move the refusal to the entry point, and never above it, which would leave the
/// intake unbounded again.
///
/// For roots that owner is the encoder. `Roots` is the sole nonblocking durable-graph
/// aggregate: construction publishes a complete traversable graph at root N+1 and the
/// encoder reports [`crate::ImageBuildError::TooManyRoots`] over the whole of it, so
/// diagnostics and invariants keep their exact precedence. A plan stopping at
/// [`MAX_ROOTS`] would truncate the graph the candidate is reported over.
pub const MAX_ADMITTED_ROOT_OCCURRENCES: usize = MAX_ROOTS + 1;

/// Product declarations an [`crate::AdmittedGraphInputPlan`] can admit into construction.
///
/// Derived from [`MAX_ADMITTED_ROOT_OCCURRENCES`], not chosen: a Product declaration with
/// no root occurrence projects nothing into an image, so admitted declarations cannot
/// exceed admitted occurrences.
pub const MAX_ADMITTED_PRODUCT_DECLARATIONS: usize = MAX_ADMITTED_ROOT_OCCURRENCES;

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

/// Member commands one Product declaration may be handed, and so the widest command
/// vector an [`crate::AdmittedGraphInputPlan`] can admit.
///
/// Derived, not chosen: it is exactly one command past [`MAX_DURABLE_MEMBERS`], by the
/// admitted-intake rule stated on [`MAX_ADMITTED_ROOT_OCCURRENCES`]. A declaration at the
/// member bound encodes, and a declaration one command wider must still reach the
/// encoder's [`crate::ImageBuildError::TooManyDurableMembers`] refusal rather than being
/// masked at the construction entry point — admitting fewer would hand the encoder a
/// silently truncated declaration instead of refusing the over-wide one.
pub const MAX_ADMITTED_DECLARATION_COMMANDS: usize = MAX_DURABLE_MEMBERS + 1;

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

/// Exported functions per image: the public entry surface `marrow check`/`run`/`test`
/// and the generated client address by stable id. A production application's public
/// surface is a family of exports across its modules — a measured five-module teaching
/// ensemble declared 43 natural exports — so the count admits that surface with
/// production-shaped headroom rather than the T01 waypoint of 32. Widening from 32 is
/// monotone: an export table is a decode-time allocation guard, never a stored-format
/// byte (the image encodes its actual `u16` export count), so every image the narrow
/// bound accepted a wider one still accepts byte-for-byte, and no image-container or
/// profile version bump is required (see the module header). The verifier rechecks
/// `count <= MAX_EXPORTS` against the received bytes before it allocates the export
/// vector (`marrow_verify::verify::code_tables`), so a hostile image claiming more
/// exports is refused with a typed bound rejection, not misread. Each export targets a
/// distinct function (the v0 one-export-per-function invariant), so [`MAX_FUNCTIONS`]
/// bounds this count from above; [`MAX_IMAGE_BYTES`] remains the true byte bound on the
/// emitted export table.
pub const MAX_EXPORTS: usize = 256;

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
// Every count these bounds guard is spelled in the image bytes (and in the identity
// preimages that mirror them) as a fixed-width big-endian integer, and the encoder
// narrows the `usize` row count (and each owned wide logical ordinal) to that width
// through the measure core's checked policy-clean path after the policy walk has
// already refused a draft above the bound. Such a conversion is lossless exactly when
// the bound itself fits the width it is spelled in, so the derivation is asserted once
// here rather than re-argued at each conversion. Widening a bound past its encoded
// width fails the build here instead of silently truncating a count in an emitted
// image.
//
// The two platform assertions carry the rest of the crate's conversions: an index or
// id widened with `as usize` is lossless because `usize` is at least 32 bits, and a
// byte length narrowed with `as u64` into a length-delimited identity preimage is
// lossless because `usize` is at most 64 bits.
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

// ---- What holding a durable contract graph costs while it is built.
//
// A bound above says what an image may *hold*; the charges below say what holding it
// *costs* the owner that builds it. They are published because the two admission owners —
// the compiler, bounded by its identity-ledger census, and the independent verifier,
// bounded by the received bytes — each derive their own maximum-live equation from this
// one representation instead of sampling a fixture.
//
// The invariant is that no admissible input can exceed a figure derived this way. A measured
// term states what one input cost, and a denser admissible input then exceeds it — so it was
// never a bound. An equation over the representation charges every element the representation
// has, and widening a row moves every equation that consumes it, at the place that equation
// is asserted.
//
// No ceiling is asserted here. This crate takes intake from whoever holds a plan, so the
// admitted-intake ceilings above are what an arbitrary caller may hand over, not a peak any
// admission owner reaches. The peaks are the owners' own, and each asserts its own.

/// A growing `Vec` is live at three times its admitted length: the amortized capacity
/// slack, plus the buffer it still holds while it copies into its successor. Charged on
/// every collection that grows one element at a time as construction proceeds.
pub const GROWTH_AND_COPY: u64 = 3;

/// The live bytes one flat member command occupies in the vector handed to
/// [`crate::ImageDraft::declare_product`].
pub const DURABLE_COMMAND_BYTES: u64 = crate::product::DECLARATION_COMMAND_BYTES;

/// The live bytes one materialized member row occupies in a Product's flat member graph.
pub const DURABLE_MEMBER_ROW_BYTES: u64 = crate::product::DECLARATION_ROW_BYTES;

/// The live bytes one Product declaration row occupies beside its member rows.
pub const DURABLE_PRODUCT_ROW_BYTES: u64 = crate::product::PRODUCT_DECLARATION_ROW_BYTES;

/// The live bytes one root-occurrence row occupies, at the widest key tuple the image
/// admits. Its managed indexes are charged per index, not per occurrence: an occurrence
/// declaring none pays for none.
pub const DURABLE_OCCURRENCE_ROW_BYTES: u64 = crate::product::ROOT_OCCURRENCE_ROW_BYTES;

/// The live bytes one managed-index declaration occupies apart from its projection.
pub const DURABLE_INDEX_SHAPE_BYTES: u64 = crate::product::MANAGED_INDEX_SHAPE_BYTES;

/// The live bytes one projected index component occupies.
pub const DURABLE_INDEX_COMPONENT_BYTES: u64 = crate::product::MANAGED_INDEX_COMPONENT_BYTES;

/// The live bytes one managed-index declaration occupies, at the widest leaf projection
/// the image admits: its shape plus a full projection of components.
pub const DURABLE_INDEX_BYTES: u64 = crate::product::MANAGED_INDEX_BYTES;

/// The live bytes one interned value-shape node occupies in the graph's one arena, apart
/// from the references it holds.
pub const DURABLE_VALUE_NODE_BYTES: u64 = crate::value_dag::VALUE_SHAPE_NODE_BYTES;

/// The live bytes one reference inside a value shape occupies: a dense composite's leaf, an
/// enum's member, or one of that member's payload leaves. Charged at the widest of the
/// three, since an owner states how many references it admits and not which kind.
pub const DURABLE_VALUE_REFERENCE_BYTES: u64 = crate::value_dag::VALUE_SHAPE_REFERENCE_BYTES;

/// The live bytes a durable contract graph and the plan admitting input into it occupy
/// before either holds a row.
pub const DURABLE_GRAPH_FIXED_BYTES: u64 =
    crate::product::CONTRACT_GRAPH_FIXED_BYTES + size_of::<crate::AdmittedGraphInputPlan>() as u64;

/// The live bytes a durable graph reconstructed from received image bytes may occupy per
/// received byte — the rate a decoder sizes its own maximum-live equation against.
///
/// A decoder cannot state its populations the way an admission owner can: it holds bytes,
/// and what those bytes decode to is the attacker's choice. What it can state is the widest
/// rate any one element pays, because every element the graph holds is spelled in the
/// DURABLE section and so consumes wire bytes of its own. Each rate below is that element's
/// published live charge over the fewest bytes the grammar can spell it in:
///
/// | element | live | fewest wire bytes | rate |
/// |---|---|---|---|
/// | member row | 56 | 17 — a tag byte and a ledger id | 4 |
/// | root occurrence | 232 | 34 — a name, a placement id, and a Product id | 7 |
/// | managed index | 48 | 19 — a ledger id, a unique flag, and a component count | 3 |
/// | index component | 17 | 17 — a kind byte and a ledger id | 1 |
/// | value shape node | 92 | 2 — a shape tag and a scalar tag | 46 |
/// | value shape reference | 40 | 2 — a leaf's own spelling | 20 |
///
/// The value-shape node is the widest and therefore the rate: a decoder that met a section
/// of nothing but distinct two-byte scalar shapes would mint one interned node per two
/// bytes. That it dominates is a fact about the arena's node representation, not about the
/// graph's rows, and it is published rather than averaged away so the decoder's ceiling
/// names the term that actually sizes it.
///
/// Each floor is the *fewest* bytes the grammar can spell that element in, read off the
/// grammar rather than rounded down for safety. A floor stated below the real one inflates
/// the rate, which looks conservative and is: it also hides how much of the decoder's
/// ceiling is headroom, so a later tightening would inherit the padding instead of the
/// fact. The smallest value shape is a scalar, and a scalar is two bytes — its shape tag
/// and its scalar tag — never one.
///
/// Rounded up at every element, so the rate is an over-estimate of any real mixture: a
/// section is one sequence of elements, and no sequence pays more than its costliest member
/// pays throughout.
pub const DURABLE_LIVE_BYTES_PER_WIRE_BYTE: u64 = {
    let rates = [
        DURABLE_MEMBER_ROW_BYTES.div_ceil(17),
        DURABLE_OCCURRENCE_ROW_BYTES.div_ceil(34),
        DURABLE_INDEX_SHAPE_BYTES.div_ceil(19),
        DURABLE_INDEX_COMPONENT_BYTES.div_ceil(17),
        DURABLE_VALUE_NODE_BYTES.div_ceil(2),
        DURABLE_VALUE_REFERENCE_BYTES.div_ceil(2),
    ];
    let mut widest = 0;
    let mut index = 0;
    while index < rates.len() {
        if rates[index] > widest {
            widest = rates[index];
        }
        index += 1;
    }
    widest
};

/// The maximum live bytes of building one durable contract graph of `products` Product
/// declarations, `occurrences` root occurrences, `members` member rows across those
/// declarations, and `value_nodes` distinct value shapes.
///
/// The terms are the real simultaneous owners, each charged at its own overlap:
///
/// - the flat command vector the caller still holds while the rows are materialized;
/// - the rows one unpublished builder materializes before publication moves them;
/// - the published tables — declarations, their member rows, occurrences, and the value
///   arena — each still growing, so each charged with its copy-into-successor overlap;
/// - the fixed graph and the plan admitting input into it.
///
/// The builder's rows and the published member rows are charged separately although
/// publication moves rather than copies them: an equation that assumed the allocator reused
/// that buffer would be assuming the thing it is meant to bound.
///
/// Evaluated in a `const` context by its callers, so an overflow in any product or sum is a
/// compile-time error rather than a wrap an assertion could then be "proven" on.
///
/// It is published because it is *the* equation: every admission owner that states a
/// maximum-live figure derives it here, from the charges this module publishes, rather than
/// writing its own arithmetic over the same rows. A second equation over one representation
/// is two answers to one question.
pub const fn max_live_durable_graph_bytes(counts: DurableGraphCounts) -> u64 {
    let DurableGraphCounts {
        products,
        occurrences,
        indexes,
        members,
        value_nodes,
        value_references,
    } = counts;

    let commands = GROWTH_AND_COPY * members * DURABLE_COMMAND_BYTES;
    let builder = GROWTH_AND_COPY * members * DURABLE_MEMBER_ROW_BYTES;
    let published = GROWTH_AND_COPY
        * (products * DURABLE_PRODUCT_ROW_BYTES
            + occurrences * DURABLE_OCCURRENCE_ROW_BYTES
            + indexes * DURABLE_INDEX_BYTES
            + members * DURABLE_MEMBER_ROW_BYTES
            + value_nodes * DURABLE_VALUE_NODE_BYTES
            + value_references * DURABLE_VALUE_REFERENCE_BYTES);

    commands + builder + published + DURABLE_GRAPH_FIXED_BYTES
}

/// The counts one durable contract graph is built from — every population the
/// representation charges per element, named rather than positional so an admission owner
/// cannot transpose two of them.
///
/// An owner states the extrema its own admission permits; this crate states what each one
/// costs. A count no owner can reach is still a count some caller may hand over, so the
/// equation charges what it is given and the ceiling belongs to whoever states the counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableGraphCounts {
    /// Distinct Product declarations.
    pub products: u64,
    /// Root occurrences over those declarations.
    pub occurrences: u64,
    /// Managed index declarations across those occurrences.
    pub indexes: u64,
    /// Member rows across those declarations.
    pub members: u64,
    /// Distinct value shapes interned in the graph's one arena.
    pub value_nodes: u64,
    /// References those value shapes hold: a dense composite's leaves, an enum's members,
    /// and each member's own payload leaves. Charged apart from the nodes because a node's
    /// own width is not fixed, so an arena of `n` nodes has no single cost.
    pub value_references: u64,
}

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
        assert_eq!(MAX_IMAGE_BYTES, 512 * 1024, "whole-image byte ceiling");
    }

    /// The exported-function surface bound: widened from the T01 waypoint of 32 to admit a
    /// production application's multi-module export family (a measured 43-export teaching
    /// ensemble) with production-shaped headroom. The `MAX_EXPORTS <= MAX_FUNCTIONS`
    /// relationship (each export targets a distinct function) is enforced at compile time
    /// by the `const _` block above.
    #[test]
    fn export_bound_holds_its_widened_value() {
        assert_eq!(MAX_EXPORTS, 256, "exported functions per image");
    }

    /// The published per-element charges of the durable contract graph's representation.
    ///
    /// These are the terms both admission owners' maximum-live equations are derived from,
    /// so they are pinned exactly rather than only bounded: an element that grows is an
    /// observable change to what building a graph costs, and it must be re-derived where it
    /// is consumed rather than absorbed here. Each element's own field set is held complete
    /// by the destructuring gates beside the rows themselves.
    #[test]
    fn the_durable_graph_charges_hold_their_measured_widths() {
        assert_eq!(DURABLE_COMMAND_BYTES, 56, "one flat member command");
        assert_eq!(DURABLE_MEMBER_ROW_BYTES, 64, "one materialized member row");
        assert_eq!(DURABLE_PRODUCT_ROW_BYTES, 72, "one Product declaration row");
        assert_eq!(
            DURABLE_OCCURRENCE_ROW_BYTES, 232,
            "one root occurrence at the widest key tuple, plus the shared index list's control block"
        );
        assert_eq!(
            DURABLE_INDEX_BYTES, 1_272,
            "one managed index at the widest leaf projection"
        );
        assert_eq!(DURABLE_VALUE_NODE_BYTES, 92, "one interned value shape");
        assert_eq!(
            DURABLE_VALUE_REFERENCE_BYTES, 40,
            "one reference inside a value shape"
        );
        assert_eq!(
            DURABLE_GRAPH_FIXED_BYTES, 232,
            "the empty graph and its plan"
        );
    }

    /// One count of each population, for the charge tests below.
    const fn one_of_each() -> DurableGraphCounts {
        DurableGraphCounts {
            products: 1,
            occurrences: 1,
            indexes: 1,
            members: 1,
            value_nodes: 1,
            value_references: 1,
        }
    }

    /// The equation charges every population it names, and is linear in each.
    ///
    /// A term that stopped being charged — a table that started sharing, a vector whose
    /// growth stopped being counted — would show here as a population the total no longer
    /// moves with, which is exactly the silent under-accounting the equation exists to
    /// prevent.
    #[test]
    fn every_population_of_the_maximum_live_equation_is_charged() {
        const NONE: DurableGraphCounts = DurableGraphCounts {
            products: 0,
            occurrences: 0,
            indexes: 0,
            members: 0,
            value_nodes: 0,
            value_references: 0,
        };
        let empty = max_live_durable_graph_bytes(NONE);
        assert_eq!(empty, DURABLE_GRAPH_FIXED_BYTES);

        for (name, counts) in [
            (
                "products",
                DurableGraphCounts {
                    products: 1,
                    ..NONE
                },
            ),
            (
                "occurrences",
                DurableGraphCounts {
                    occurrences: 1,
                    ..NONE
                },
            ),
            ("indexes", DurableGraphCounts { indexes: 1, ..NONE }),
            ("members", DurableGraphCounts { members: 1, ..NONE }),
            (
                "value nodes",
                DurableGraphCounts {
                    value_nodes: 1,
                    ..NONE
                },
            ),
            (
                "value references",
                DurableGraphCounts {
                    value_references: 1,
                    ..NONE
                },
            ),
        ] {
            assert!(
                max_live_durable_graph_bytes(counts) > empty,
                "the equation does not charge {name}"
            );
        }

        // Linear, not merely monotone: doubling every population doubles every charged term.
        let single = max_live_durable_graph_bytes(one_of_each()) - DURABLE_GRAPH_FIXED_BYTES;
        let one = one_of_each();
        let doubled = DurableGraphCounts {
            products: 2 * one.products,
            occurrences: 2 * one.occurrences,
            indexes: 2 * one.indexes,
            members: 2 * one.members,
            value_nodes: 2 * one.value_nodes,
            value_references: 2 * one.value_references,
        };
        let double = max_live_durable_graph_bytes(doubled) - DURABLE_GRAPH_FIXED_BYTES;
        assert_eq!(
            double,
            2 * single,
            "the equation is linear in its populations"
        );
    }
}
