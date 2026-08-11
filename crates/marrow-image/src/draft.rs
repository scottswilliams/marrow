//! The typed validating `ImageDraft` (design §C).
//!
//! The compiler builds an image through this owner: it interns strings and
//! constants, adds record types, roots, sites, functions, and exports, and calls
//! [`ImageDraft::encode`] to produce canonical container bytes with a computed
//! digest. Building works in logical intern ids (`StrId`, `ConstId`); the encoder
//! sorts the string and constant pools into their canonical order and rewrites
//! every reference, so the compiler never reasons about final pool positions.
//!
//! The draft enforces the §E operation-site bound as it is built: sites are minted
//! through one bounded [`SiteDemandPlan`] that checks vacant capacity before it mints an
//! id, so no site id is ever a narrowed table length. A site is named by binding a live
//! root occurrence to a live canonical declaration path, so a producer cannot address a
//! node the graph does not contain, and appending a function validates every site operand
//! its code carries. The remaining `add_*` owners still append unconditionally on the
//! string, constant, type, enum, collection, export, and test-entry paths and are bounded
//! only by the encoder's recheck. The independent verifier rechecks every bound against
//! the received bytes; the draft's checks are a producer-side guard, not the trust
//! boundary.
//!
//! Those unconditional owners mint a logical id by narrowing the table's current length
//! to the `u16` the id newtype carries, so a table pushed past `u16::MAX` rows would
//! wrap an id onto an earlier one. No such draft has an image: every one of those tables
//! has a §E bound (`MAX_STRINGS`, `MAX_CONSTS`, `MAX_TYPES`, `MAX_ENUMS`,
//! `MAX_COLLECTIONS`, `MAX_FUNCTIONS`) an order of magnitude below `u16::MAX`, and
//! [`ImageDraft::encode`] refuses the draft against that bound with the table's typed
//! `TooMany*` error before any id is spelled into bytes. The `const _` encoded-width
//! block in [`crate::bounds`] fails the build if a later widening lifts one of those
//! bounds past the id width, at which point these mints need typed refusals rather than
//! this derivation.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::bounds;
use crate::durable_id::{
    DurableContractView, DurableIndexShape, DurableProductIdentity, LedgerIdBytes,
    RootPlacementIdentity,
};
use crate::export_id::ExportId;
use crate::instr::Instr;
use crate::product::{
    CanonicalDeclarationPathSelector, DeclarationMember, DeclarationMemberDef, OccurrenceGraph,
    ProductClaimConflict, ProductDeclaration, ProductDeclarationTable, ProductEntryRecordClaim,
    RootOccurrence, RootOccurrenceRow, RootOccurrenceSelector, RootOccurrenceTable,
};
use crate::semantic::{SemanticPath, SemanticTarget};
use crate::site_plan::{
    LegacyDraftSiteOperand, OccurrenceSiteHandle, SiteDemandPlan, SitePlanState,
    SitePlanStateError, SitePolicyReceipt,
};
use crate::ty::{ImageType, Scalar};
use crate::value_dag::CanonicalValueShapeDag;

/// The strong identity of one draft and its site demand plan.
///
/// Every selector, handle, and site operand carries the identity of the draft that
/// answered for it, so a value minted by one draft cannot authenticate a place in
/// another. It is minted once per draft from a process-wide counter and is never derived
/// from an address, a length, or anything a caller supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DraftIdentity(u64);

impl DraftIdentity {
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The durable-graph construction budget one compile is admitted for.
///
/// The draft's flat construction entry points are the one path into the durable graph,
/// and every one of them requires this plan: no plan, no construction. It carries the
/// counts an admission owner froze *before* construction began — how many Product
/// declarations, how many root occurrences, and how wide one declaration's command vector
/// may be — so a caller cannot hand the draft an unbounded, uncounted, unadmitted command
/// stream and have it discovered only by the encoder afterwards.
///
/// It is unforgeable on the compiler's `SignaturesComplete` idiom: the counts are private
/// and [`Self::admit`] is the only constructor, so it has no literal form outside this
/// module and its existence is proof that an admission owner checked its counts against
/// what a ProgramImage can hold. A plan carrying unadmitted counts does not exist.
///
/// It is a budget, not permission to reach it. Every table the entry points append to
/// still rechecks its own bound, the declaration graph's own command validation remains the
/// one structural validator of a command vector, and the independent verifier rechecks
/// every bound against received bytes. The plan bounds *intake*; it classifies nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedGraphInputPlan {
    products: usize,
    roots: usize,
    commands: usize,
}

impl AdmittedGraphInputPlan {
    /// The plan a storeless compile carries: no durable construction is admitted at all.
    ///
    /// A storeless image declares no Product, occurs no root, and states no member, so its
    /// budget is zero rather than absent. The construction entry points refuse under it
    /// because they are over budget, and the reading ones answer nothing because a draft
    /// built under it holds nothing — neither needs a second, planless spelling.
    pub const EMPTY: Self = Self {
        products: 0,
        roots: 0,
        commands: 0,
    };

    /// The largest intake the durable graph accepts from anyone: every term at the ceiling
    /// [`Self::admit`] checks.
    ///
    /// It grants nothing `admit` would not grant for the same numbers, and it is not a way
    /// around a census. It exists so an admission owner whose census is *larger* than any
    /// image could carry has a total answer — the graph takes what it can take, and the
    /// owner that reports the overrun still reports it over a complete graph.
    pub const MAXIMUM: Self = Self {
        products: bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
        roots: bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
        commands: bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    };

    /// Admit a construction budget of `products` Product declarations, `roots` root
    /// occurrences, and `commands` member commands in any one declaration.
    ///
    /// Refused when a term is beyond what may be handed to the durable graph at all: an
    /// intake no image could carry is not a budget, and admitting one would leave the
    /// unbounded, uncounted command stream this type exists to close. Each ceiling follows
    /// the admitted-intake rule stated on [`bounds::MAX_ADMITTED_ROOT_OCCURRENCES`] — one
    /// past the bound whose refusal owner keeps its refusal — so an over-wide declaration
    /// still reaches [`ImageBuildError::TooManyDurableMembers`] and an over-count graph
    /// still reaches [`ImageBuildError::TooManyRoots`] over a complete graph.
    ///
    /// An admission owner mints its terms from its own census, never from a caller's wish:
    /// the compiler's from the store-declaration census it takes before the first store is
    /// built, and the identity ledger binds a Product narrower still — every admitted
    /// member anchors one live ledger row, so `MAX_IDS_ROWS - 3` members is the real
    /// per-Product ceiling, asserted where both constants are in scope.
    pub fn admit(products: usize, roots: usize, commands: usize) -> Option<Self> {
        if products > bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS
            || roots > bounds::MAX_ADMITTED_ROOT_OCCURRENCES
            || commands > bounds::MAX_ADMITTED_DECLARATION_COMMANDS
        {
            return None;
        }
        Some(Self {
            products,
            roots,
            commands,
        })
    }

    /// Product declarations this plan admits.
    pub(crate) fn products(&self) -> usize {
        self.products
    }

    /// Root occurrences this plan admits.
    pub(crate) fn roots(&self) -> usize {
        self.roots
    }

    /// Member commands this plan admits in any one declaration.
    pub(crate) fn commands(&self) -> usize {
        self.commands
    }
}

/// The stamp one appended row carries.
///
/// Row ordinals are reused deterministically — a proof pass appends rows and the rewind
/// discards them, and the next append lands on the same ordinal — so an ordinal alone
/// cannot say whether the row a selector, handle, or operand was minted against is still
/// the row that is there. The stamp is drawn from a per-draft counter that the rewind
/// deliberately does **not** restore, so a re-minted row is never mistaken for the row it
/// replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowStamp(u64);

impl RowStamp {
    /// The stamp for one draw of a per-owner monotone counter.
    pub(crate) fn from_counter(count: u64) -> Self {
        Self(count)
    }
}

/// A logical string-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrId(u16);

/// A logical constant-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstId(pub(crate) u16);

impl ConstId {
    /// The raw logical index, as carried in a `ConstLoad` operand until the encoder
    /// rewrites it to the final sorted pool position.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A record-type index (also the final container index; types keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub(crate) u16);

impl TypeId {
    /// The record-type index at `index`.
    ///
    /// A record-type index is a container-table position, not a capability, for the same
    /// reason [`StrId::from_index`] is: the independent verifier reads one from received
    /// bytes and every owner that resolves one range-checks it.
    pub fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// The raw record-type index, as carried in a `RecordNew`/`FieldGet` operand and
    /// in an `ImageType::Record`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// An enum-type index (also the final container index; enums keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub(crate) u16);

impl EnumId {
    /// The raw enum-type index, as carried in `EnumConstruct`/`EnumTag`/
    /// `EnumPayloadGet`/`EqEnum` operands and in an `ImageType::Enum`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A collection-type index (also the final container index; collection types keep
/// insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollTypeId(pub(crate) u16);

impl CollTypeId {
    /// The raw collection-type index, as carried in `ListNew`/`MapNew` operands and
    /// in an `ImageType::Collection`.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A function index (also the final container index; functions keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuncId(pub(crate) u16);

impl FuncId {
    /// The raw function index, as carried in a `Call` operand and an export.
    pub fn index(self) -> u16 {
        self.0
    }
}

/// A durable operation-site index (also the final container index).
///
/// It is crate-private: outside this crate a site is named by the opaque
/// [`LegacyDraftSiteOperand`] the plan mints, never by a number a caller can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SiteId(u16);

impl SiteId {
    /// The id of the site row at `ordinal`. Minted only by the site demand plan, which
    /// checks vacant capacity first, so every value that reaches here is inside the site
    /// table's capacity.
    pub(crate) fn from_ordinal(ordinal: u16) -> Self {
        Self(ordinal)
    }

    /// The raw site index, as carried in a `Dur*` operand.
    pub(crate) fn index(self) -> u16 {
        self.0
    }
}

/// One record field. Its type is a bare (non-optional) [`ImageType`]: a scalar
/// for a durable-storable field, or a closed enum (`Option`/`Result`/a user
/// `enum`) for a local-only value field. Sparseness is the field's `required`
/// flag, not an optional wrapper on the type.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: StrId,
    pub ty: ImageType,
    pub required: bool,
}

/// A record type: an ordered field list. Field order is the declaration order.
#[derive(Debug, Clone)]
pub struct RecordTypeDef {
    pub name: StrId,
    pub fields: Vec<FieldDef>,
}

/// One enum variant: a member name, a `category` flag reserving the hierarchy
/// seam (always a leaf on the current flat line — the checker rejects category
/// members), and its ordered dense payload (empty for a payloadless member). Each
/// payload leaf is a bare (non-optional) [`ImageType`]: a user `enum` member
/// carries only bare scalars, while a built-in `Option`/`Result` instantiation
/// carries whatever concrete type its argument monomorphized to (a scalar, a
/// record, or another enum). Payload order is the declaration order — the
/// canonical product-leaf order the checker owns.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: StrId,
    pub category: bool,
    pub payload: Vec<ImageType>,
}

/// A closed enum type: an ordered variant list in declaration order.
#[derive(Debug, Clone)]
pub struct EnumTypeDef {
    pub name: StrId,
    pub variants: Vec<VariantDef>,
}

/// One collection value type: a finite `List<T>` or ordered `Map<K, V>`. The
/// element/key/value types are bare (non-optional) [`ImageType`]s and may
/// themselves be `Collection` references, so a nested collection reaches its inner
/// shape through the COLLTYPES table. A `Map` key is a bare scalar key type
/// (`int`/`bool`/`string`/`bytes`; a nominal key is int-shaped), the one durable-key
/// scalar family the ordered map compares over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionTypeDef {
    List { elem: ImageType },
    Map { key: ImageType, value: ImageType },
}

/// One key column of a durable root or branch placement: its orderable durable-key
/// scalar and the entropy-minted ledger id anchored at `<placement>.<column>`.
/// Column order is the declared tuple order and is part of the durable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyColumn {
    pub scalar: Scalar,
    pub id: LedgerIdBytes,
}

/// One durable root occurrence to admit: the occurrence facts of one `store` root.
///
/// A singleton root has an empty key tuple; a keyed root has one or more ordered
/// [`KeyColumn`]s drawn from the closed orderable durable-key scalar set. The member
/// graph and the entry record are **not** here: they are Product declaration facts,
/// declared once by [`ImageDraft::declare_product`] however many roots occur over them.
#[derive(Debug, Clone)]
pub struct RootOccurrenceDef {
    pub name: StrId,
    pub keys: Vec<KeyColumn>,
    pub placement: LedgerIdBytes,
    /// The root's narrow compiler-maintained managed indexes, in source declaration
    /// order. Each projects an ordered leaf reference set from this root; it stores no
    /// data of its own and contributes only its identity and projection to the durable
    /// contract. They are occurrence facts: two roots over one Product may carry
    /// different index shapes.
    pub indexes: Vec<DurableIndexShape>,
}

/// What an admitted root occurrence publishes: the selector naming the occurrence row,
/// the wire RootId an entry identity `Id(^root)` carries, and the canonical path
/// selectors the row itself owns — its own placement, and one per managed index in
/// declaration order.
///
/// The Product's member paths are not here: they belong to the declaration, are shared by
/// every occurrence over it, and are read navigationally through
/// [`ImageDraft::product_members`].
#[derive(Debug, Clone)]
pub struct AdmittedRoot {
    occurrence: RootOccurrenceSelector,
    root_id: u16,
    placement: CanonicalDeclarationPathSelector,
    indexes: Vec<CanonicalDeclarationPathSelector>,
}

impl AdmittedRoot {
    /// The selector naming this occurrence row.
    pub fn occurrence(&self) -> &RootOccurrenceSelector {
        &self.occurrence
    }

    /// The DURABLE-table index of this occurrence — the discriminant an entry identity
    /// `Id(^root)` carries on the wire. It is a wire fact the compiler emits into
    /// identity instructions, not a way to name the occurrence row: naming it is what the
    /// selector is for.
    pub fn root_id(&self) -> u16 {
        self.root_id
    }

    /// The canonical path of this root's own keyed placement.
    pub fn placement_path(&self) -> &CanonicalDeclarationPathSelector {
        &self.placement
    }

    /// The canonical paths of this root's managed indexes, in declaration order.
    pub fn index_paths(&self) -> &[CanonicalDeclarationPathSelector] {
        &self.indexes
    }
}

/// A durable operation site as the encoder writes it: the [`SemanticPath`] of the graph
/// node it addresses plus the closed [`SemanticTarget`] kind it performs there.
///
/// The path — not a container-table index — names the node, so the site follows the
/// durable graph's ledger ids; the verifier resolves the path against its own
/// reconstructed node set and rejects a path that names no node or whose target kind
/// disagrees with the resolved node's kind.
///
/// It is a **projection**, minted transiently at encode from the demand key the plan
/// retains. Nothing retains it: a retained path is a second copy of the graph that can
/// drift from the rows the wire and the contract id are written from.
#[derive(Debug, Clone)]
pub(crate) struct SiteDef {
    pub(crate) path: SemanticPath,
    pub(crate) target: SemanticTarget,
}

/// A source-position mapping for one instruction. The encoder converts the
/// instruction index to its container byte offset.
#[derive(Debug, Clone)]
pub struct SpanEntry {
    pub instr_index: u32,
    pub line: u32,
    pub column: u32,
}

/// A function body.
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: StrId,
    pub source: StrId,
    /// Parameter types in declaration order. Each is a bare scalar or a bare record
    /// (a dense `struct` value); the verifier rechecks the same restriction.
    pub params: Vec<ImageType>,
    pub ret: ImageType,
    /// Total local slots, including params (which occupy slots `0..params.len()`).
    pub local_count: u16,
    pub code: Vec<Instr>,
    pub spans: Vec<SpanEntry>,
}

/// An export: a stable [`ExportId`] bound to a function. The image carries the id,
/// never the source name — the VM looks an export up by its verified id, so no
/// human-readable name crosses the trust boundary.
#[derive(Debug, Clone)]
struct ExportDef {
    id: ExportId,
    func: FuncId,
}

/// A test entry: a report-name string bound to a storeless zero-argument function
/// `marrow test` runs. Unlike an export it carries no wire identity — the name is a
/// human report label only, never an interface, demand, or durable identity.
#[derive(Debug, Clone)]
struct TestEntryDef {
    name: StrId,
    func: FuncId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ConstValue {
    Int(i64),
    Bool(bool),
    Text(StrId),
    /// A temporal scalar folded from a compile-time-validated canonical text
    /// literal: a `date` (days since the Unix epoch), an `instant` (signed
    /// nanoseconds since the epoch), or a `duration` (signed nanoseconds). The raw
    /// scalar is stored directly, so the runtime loads it without re-parsing text.
    Date(i32),
    Instant(i128),
    Duration(i128),
}

/// A failure to build a well-formed draft: a §E bound exceeded or an invalid
/// cross-reference. These are producer-side (compiler) faults, not artifact
/// rejections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageBuildError {
    TooManyStrings,
    StringTooLong,
    TooManyConsts,
    TooManyTypes,
    TooManyFields,
    TooManyStructLeaves,
    TooManyEnums,
    TooManyVariants,
    TooManyPayloadFields,
    TooManyCollections,
    TooManyRoots,
    TooManyIndexes,
    TooManyIndexComponents,
    TooManyKeyColumns,
    TooManyDurableMembers,
    DurableTreeTooDeep,
    DurableValueTooDeep,
    TooManySites,
    TooManyFunctions,
    TooManyParams,
    TooManyLocals,
    TooManyExports,
    TooManyTestEntries,
    CodeTooLong,
    LocalCountBelowParams,
    ImageTooLarge,
    /// Two occurrences of one durable Product identity claim different member/value
    /// graphs: two declarations wearing one identity.
    ProductGraphConflict,
    /// Two occurrences of one durable Product identity claim the same member/value graph
    /// with a different entry record.
    ProductEntryRecordConflict,
    InvalidReference(&'static str),
}

impl std::fmt::Display for ImageBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "image build error: {self:?}")
    }
}

impl std::error::Error for ImageBuildError {}

/// The mutable image builder.
///
/// The once-checked generic template pass appends its throwaway image directly to this
/// draft through a [`TemplateProofDraftGuard`], which discards everything the pass appended
/// when it drops.
///
/// A draft is deliberately not `Clone`. Every selector, handle, and site operand it mints
/// carries its identity, so a copied draft would carry a copied identity and a copied stamp
/// cursor: every capability minted afterwards would authenticate against both copies, and
/// the row-stamp check that makes a discarded row's reused ordinal detectable would answer
/// for a row that a different draft appended.
///
/// ```compile_fail,E0599
/// // A draft is not `Clone`: copying one would copy its identity and its stamp cursor.
/// let draft = marrow_image::ImageDraft::new();
/// let _copy = draft.clone();
/// ```
///
/// A site is named by binding a live root occurrence to a live declaration path, never by
/// a raw ordinal or key.
///
/// ```compile_fail,E0308
/// // There is no raw ordinal or key binder: `bind_occurrence_site` takes published
/// // selectors, and neither selector can be spelled as a number.
/// let mut draft = marrow_image::ImageDraft::new();
/// let plan = marrow_image::AdmittedGraphInputPlan::EMPTY;
/// let _ = draft.bind_occurrence_site(&plan, &0usize, &0usize, marrow_image::SemanticTarget::WholePayload);
/// ```
///
/// A handle already carries the one target it was bound for, so requesting a site takes no
/// second target input.
///
/// ```compile_fail,E0061
/// // A handle carries its target; there is no second target input to disagree with it.
/// let mut draft = marrow_image::ImageDraft::new();
/// let plan = marrow_image::AdmittedGraphInputPlan::EMPTY;
/// let handle: marrow_image::OccurrenceSiteHandle = unimplemented!();
/// let _ = draft.request_site(&plan, &handle, marrow_image::SemanticTarget::WholePayload);
/// ```
///
/// Entering the durable graph requires an [`AdmittedGraphInputPlan`]: no plan, no
/// construction.
///
/// ```compile_fail,E0061
/// // There is no planless construction entry point.
/// let mut draft = marrow_image::ImageDraft::new();
/// let _ = draft.declare_product(unimplemented!(), unimplemented!(), Vec::new());
/// ```
///
/// A plan has no literal form: its counts are private, so [`AdmittedGraphInputPlan::admit`]
/// is the only way one comes into being.
///
/// ```compile_fail,E0451
/// // A plan carrying unadmitted counts cannot be spelled.
/// let _ = marrow_image::AdmittedGraphInputPlan { products: 4096, roots: 4096, commands: 1 << 20 };
/// ```
///
/// A template proof's rollback is a guard over one draft, not a mark a caller holds, so
/// there is no value to carry from one draft to another. The guard borrows its draft
/// exclusively for its whole lifetime.
///
/// ```compile_fail,E0505
/// // A guard cannot be separated from the draft it rolls back.
/// let mut first = marrow_image::ImageDraft::new();
/// let guard = first.template_proof();
/// let moved = first;
/// drop(guard);
/// ```
#[derive(Debug)]
pub struct ImageDraft {
    /// This draft's strong identity, and the identity of its one site demand plan. Every
    /// selector, handle, and site operand it mints carries it.
    identity: DraftIdentity,
    /// The source of the stamps appended rows carry. It is deliberately not restored when a
    /// template proof is discarded, so an ordinal reused after a discard carries a fresh
    /// stamp.
    next_stamp: u64,
    strings: Vec<String>,
    consts: Vec<ConstValue>,
    types: Vec<RecordTypeDef>,
    enums: Vec<EnumTypeDef>,
    colls: Vec<CollectionTypeDef>,
    /// The durable Product declarations this draft has admitted, keyed by Product ledger
    /// identity: one canonical member/value graph and one runtime surface each, however
    /// many roots occur over them.
    products: ProductDeclarationTable,
    /// The flat root-occurrence rows, in declaration order. Each references the one
    /// Product declaration it projects; the index of a row is the RootId an entry
    /// identity `Id(^root)` carries.
    occurrences: RootOccurrenceTable,
    /// The first divergent repeat of an already-declared Product, if one was appended.
    /// Two occurrences of one Product identity that claim different graphs are two
    /// declarations wearing one identity: the draft cannot represent that, and
    /// [`Self::check_bounds`] refuses to encode rather than silently canonicalizing one
    /// of them away.
    product_conflict: Option<ProductClaimConflict>,
    application: Option<LedgerIdBytes>,
    /// The one owner of every durable field value shape this draft holds. A
    /// declaration row references a node; no row owns one, so a value shape shared by a
    /// thousand fields — or nested exponentially — is stored once.
    value_shapes: CanonicalValueShapeDag,
    /// The one owner of the operation-site table, its demand map, and its capacity
    /// policy. Every site an image carries is requested through it.
    sites: SiteDemandPlan,
    functions: Vec<FunctionDef>,
    exports: Vec<ExportDef>,
    test_entries: Vec<TestEntryDef>,
}

/// The append-only lengths of every [`ImageDraft`] owner at one instant, plus the
/// application-identity slot and the site plan's policy state.
///
/// It is the private interior of a [`TemplateProofDraftGuard`]: it is never returned,
/// stored, or named outside this module, so there is no mark a caller could hold, copy, or
/// apply to a draft other than the one the guard exclusively borrows.
///
/// A proof pass only appends new entries and fills freshly-reserved records/enums (never a
/// pre-existing index), so truncating each owner back to its recorded length restores the
/// draft to the exact bytes it held before the pass.
///
/// Two draft fields are deliberately excluded, and [`ImageDraft::checkpoint`] destructures
/// the draft exhaustively so a new owner cannot join them by omission: `identity` is fixed
/// at mint and no pass can change it, and `next_stamp` advances monotonically on purpose so
/// an ordinal reused after a discard carries a fresh stamp.
struct DraftCheckpoint {
    strings: usize,
    consts: usize,
    types: usize,
    enums: usize,
    colls: usize,
    products: usize,
    occurrences: usize,
    value_shapes: usize,
    sites: usize,
    functions: usize,
    exports: usize,
    test_entries: usize,
    application: Option<LedgerIdBytes>,
    /// The sticky Product-claim conflict at the checkpoint. It gates encoding while the
    /// declaration table that admits it is truncated with the pass, so a conflict first
    /// recorded *inside* the proof is cleared with the divergent row that caused it; one
    /// recorded *before* the proof is kept, because both disagreeing declarations survive.
    product_conflict: Option<ProductClaimConflict>,
    /// The site plan's policy state at the checkpoint. A crossing that happened *before* the
    /// proof keeps its exact receipt identity; one first recorded *inside* the proof is
    /// cleared with the rest of the pass, so a throwaway proof cannot leave the real draft
    /// refusing sites it has capacity for.
    receipt: Option<SitePolicyReceipt>,
}

/// The exclusive borrow through which the once-checked generic template pass appends its
/// throwaway image to the real draft.
///
/// The pass is proved against the in-progress draft — so a concrete callee's signature stays
/// consistent with every type already minted — and everything it appends is discarded when
/// the guard drops, on a normal return, on an early invariant, or on an unwind.
///
/// The guard **is** the capability. Its checkpoint is private and unnameable, so there is no
/// mark to clone, store, return, or hand to a second draft: the guard borrows one draft
/// exclusively for its whole lifetime, and the draft is unusable through any other path
/// until it drops. The type it replaces was a free-standing `Clone` value carrying no draft
/// identity, so a mark taken on one draft could be applied to another, where it silently
/// truncated unrelated tables.
///
/// What the guard bounds is *when* the draft is reachable and *what survives*, not which
/// operations the pass performs: [`Self::proof_draft`] reborrows the whole draft, because
/// the pass is the ordinary function lowerer and its append surface is the draft's. The
/// reborrow is bounded by the guard, so nothing it appends escapes the rollback, but a
/// narrower operation set would mean a second lowering path, which this row does not build.
///
/// **Temporary.** It is not a general draft transaction — canonical Product, root, or path
/// publication is not a template-proof operation, and there is no commit. It is deleted when
/// the production draft transaction lands.
#[doc(hidden)]
pub struct TemplateProofDraftGuard<'draft> {
    draft: &'draft mut ImageDraft,
    checkpoint: DraftCheckpoint,
}

impl TemplateProofDraftGuard<'_> {
    /// The in-progress draft the proof body appends to.
    ///
    /// The borrow is reborrowed from the guard's own exclusive borrow, so it cannot outlive
    /// the guard and no appended row can escape the rollback.
    pub fn proof_draft(&mut self) -> &mut ImageDraft {
        self.draft
    }
}

impl Drop for TemplateProofDraftGuard<'_> {
    /// Discard everything the proof appended, in reverse dependency order: the code that can
    /// retain site operands first, then the occurrence rows and site rows those operands
    /// name, then the declaration table with the claim conflict its rows can record, then the
    /// remaining owners, and finally the application slot.
    ///
    /// It allocates nothing, indexes nothing, and cannot panic, so it is safe to run during
    /// an unwind that a failed proof started.
    fn drop(&mut self) {
        let at = &self.checkpoint;
        self.draft.test_entries.truncate(at.test_entries);
        self.draft.exports.truncate(at.exports);
        self.draft.functions.truncate(at.functions);
        self.draft.occurrences.truncate(at.occurrences);
        self.draft.sites.rewind(at.sites, at.receipt);
        self.draft.products.truncate(at.products);
        self.draft.value_shapes.truncate(at.value_shapes);
        self.draft.product_conflict = at.product_conflict;
        self.draft.colls.truncate(at.colls);
        self.draft.enums.truncate(at.enums);
        self.draft.types.truncate(at.types);
        self.draft.consts.truncate(at.consts);
        self.draft.strings.truncate(at.strings);
        self.draft.application = at.application;
    }
}

impl Default for ImageDraft {
    /// A fresh draft with a fresh identity. There is deliberately no derived `Default`:
    /// every draft mints its own strong identity, and a derived one would hand every
    /// default-constructed draft the same one.
    fn default() -> Self {
        Self::new()
    }
}

impl ImageDraft {
    pub fn new() -> Self {
        Self {
            identity: DraftIdentity::mint(),
            next_stamp: 0,
            strings: Vec::new(),
            consts: Vec::new(),
            types: Vec::new(),
            enums: Vec::new(),
            colls: Vec::new(),
            products: ProductDeclarationTable::default(),
            occurrences: RootOccurrenceTable::default(),
            product_conflict: None,
            application: None,
            value_shapes: CanonicalValueShapeDag::new(),
            sites: SiteDemandPlan::default(),
            functions: Vec::new(),
            exports: Vec::new(),
            test_entries: Vec::new(),
        }
    }

    /// The stamp the next appended row carries.
    fn next_stamp(&mut self) -> RowStamp {
        self.next_stamp += 1;
        RowStamp(self.next_stamp)
    }

    /// The live tables a site binding is validated and projected against.
    fn graph(&self) -> OccurrenceGraph<'_> {
        OccurrenceGraph {
            draft: self.identity,
            occurrences: &self.occurrences,
            products: &self.products,
        }
    }

    /// Intern a string, returning its logical id. Repeated interning of the same
    /// text returns the same id.
    pub fn intern_string(&mut self, text: &str) -> StrId {
        if let Some(index) = self.strings.iter().position(|existing| existing == text) {
            return StrId(index as u16);
        }
        let id = StrId(self.strings.len() as u16);
        self.strings.push(text.to_string());
        id
    }

    pub fn intern_int(&mut self, value: i64) -> ConstId {
        self.intern_const(ConstValue::Int(value))
    }

    pub fn intern_bool(&mut self, value: bool) -> ConstId {
        self.intern_const(ConstValue::Bool(value))
    }

    /// Intern a text constant, interning its backing string as needed.
    pub fn intern_text(&mut self, text: &str) -> ConstId {
        let str_id = self.intern_string(text);
        self.intern_const(ConstValue::Text(str_id))
    }

    /// Intern a `date` constant (days since the Unix epoch).
    pub fn intern_date(&mut self, days: i32) -> ConstId {
        self.intern_const(ConstValue::Date(days))
    }

    /// Intern an `instant` constant (signed nanoseconds since the epoch).
    pub fn intern_instant(&mut self, nanos: i128) -> ConstId {
        self.intern_const(ConstValue::Instant(nanos))
    }

    /// Intern a `duration` constant (signed nanoseconds).
    pub fn intern_duration(&mut self, nanos: i128) -> ConstId {
        self.intern_const(ConstValue::Duration(nanos))
    }

    fn intern_const(&mut self, value: ConstValue) -> ConstId {
        if let Some(index) = self
            .consts
            .iter()
            .position(|existing| const_eq(*existing, value))
        {
            return ConstId(index as u16);
        }
        let id = ConstId(self.consts.len() as u16);
        self.consts.push(value);
        id
    }

    pub fn add_record_type(&mut self, def: RecordTypeDef) -> TypeId {
        let id = TypeId(self.types.len() as u16);
        self.types.push(def);
        id
    }

    pub fn add_enum_type(&mut self, def: EnumTypeDef) -> EnumId {
        let id = EnumId(self.enums.len() as u16);
        self.enums.push(def);
        id
    }

    /// The number of collection types already appended to this draft.
    pub fn collection_type_count(&self) -> usize {
        self.colls.len()
    }

    /// Add a collection type (a concrete `List`/`Map` instantiation), returning its
    /// index. Unlike records and enums a collection type has no forward-reference
    /// need — its element/key/value types are already-resolved [`ImageType`]s — so
    /// there is no two-pass reserve/fill; the caller interns the inner types first.
    ///
    /// This appends unconditionally rather than deduplicating by image content: the
    /// compiler's type registry is the single owner of collection instantiation
    /// identity and dedups by the *source* element/key/value types (so `List[Age]`
    /// and `List[int]` stay distinct even though a nominal element erases to the same
    /// image `int`), minting one row here per distinct source instantiation.
    pub fn add_collection_type(&mut self, def: CollectionTypeDef) -> CollTypeId {
        let id = CollTypeId(self.colls.len() as u16);
        self.colls.push(def);
        id
    }

    /// Replace the fields of an already-reserved record type. Value types are built
    /// in two passes — every record and enum reserves its type index first so a
    /// field or payload may reference any other value type (including a mutually
    /// referential `struct`), then each definition's fields are filled in. Only the
    /// reserving pass may set fields; the index is fixed at reservation.
    pub fn set_record_fields(&mut self, ty: TypeId, fields: Vec<FieldDef>) {
        self.types[ty.0 as usize].fields = fields;
    }

    /// Replace the variants of an already-reserved enum type (see
    /// [`Self::set_record_fields`] for the two-pass rationale).
    pub fn set_enum_variants(&mut self, id: EnumId, variants: Vec<VariantDef>) {
        self.enums[id.0 as usize].variants = variants;
    }

    /// Admit one durable **Product declaration**: its canonical member/value graph and
    /// the entry record its roots read and write, returning the declaration's direct
    /// members exactly as [`Self::product_members`] publishes them.
    ///
    /// This is the one construction path for the durable graph. It is **flat**: a
    /// Product's members arrive as a command vector whose rows name their parent by an
    /// earlier command, so a caller cannot hand the draft a recursive tree, and every
    /// command is validated against the canonical rules before any row is appended.
    ///
    /// A Product is a declaration and a root is an occurrence of it, so the graph is held
    /// once however many roots project it. The first declaration of a Product identity
    /// binds the row; a later one is a reference that must match it exactly.
    ///
    /// Two declarations of one Product identity that claim different graphs or different
    /// entry records are two declarations wearing one identity. That is recorded rather
    /// than refused here — the later one still resolves to the bound row, and
    /// [`Self::encode`] refuses the image — so the failure is reported once, in wire
    /// order, by the owner that refuses artifacts.
    ///
    /// Refused — as one opaque [`SitePlanStateError`], the single refusal type the durable
    /// construction entry points carry — when the command vector is not a well-formed flat
    /// declaration: a member naming a parent that is not an earlier command, a member
    /// count over the declaration bound, or any other canonical-rule violation. No row is
    /// appended in that case. The cause is not projected: it is a producer-side fault
    /// about a vector the caller built, and no caller branches on which rule it broke.
    ///
    /// Entering construction requires `plan`, and the command vector is admitted under it:
    /// a vector wider than the plan's admitted command count, or a declaration past the
    /// plan's admitted Product count, is refused before any row is appended. The plan
    /// bounds the intake; the declaration graph's own command validation remains the one
    /// validator of the vector's structure, and the encoder remains the one owner of the
    /// member bound — a vector the plan admits one command past that bound still reaches
    /// [`ImageBuildError::TooManyDurableMembers`] rather than being masked here.
    pub fn declare_product(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        entry_record: TypeId,
        members: Vec<DeclarationMemberDef>,
    ) -> Result<Vec<DeclarationMember>, SitePlanStateError> {
        let stamp = self.next_stamp();
        let (row, conflict) = self
            .products
            .admit_under(
                plan,
                DurableProductIdentity::minted(product),
                ProductEntryRecordClaim::new(entry_record),
                members,
                stamp,
            )
            .map_err(|_| SitePlanStateError::new(SitePlanState::InvalidDemand))?;
        if let Some(conflict) = conflict {
            self.product_conflict.get_or_insert(conflict);
        }
        Ok(self.graph().members_of_row(row))
    }

    /// Append one root occurrence over the Product declaration `product` names, returning
    /// what the completed row publishes.
    ///
    /// The occurrence row retains only the root's own placement, spelling, key tuple, and
    /// managed indexes and a reference to the one declaration, so nothing is retained per
    /// (root x member). A root over a Product this draft does not hold is refused: an
    /// occurrence with no declaration is not a root.
    ///
    /// The one opaque [`SitePlanStateError`] the construction entry points carry covers
    /// three causes here, none of them projected: the named Product is not declared here, the
    /// occurrence table is already at its root bound (the same bound the encoder reports
    /// as [`ImageBuildError::TooManyRoots`]), or the completed row's published selectors
    /// exceed what a canonical path can address. Each is a fault in the graph the caller
    /// just built, and no caller branches on which. An occurrence past the plan's admitted
    /// root count is refused the same way, before the row is pushed.
    pub fn add_root_occurrence(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        def: RootOccurrenceDef,
    ) -> Result<AdmittedRoot, SitePlanStateError> {
        if self.occurrences.len() >= plan.roots() {
            return Err(SitePlanStateError::new(SitePlanState::InvalidDemand));
        }
        let RootOccurrenceDef {
            name,
            keys,
            placement,
            indexes,
        } = def;
        let declaration = self
            .products
            .row_of(DurableProductIdentity::minted(product))
            .ok_or_else(|| SitePlanStateError::new(SitePlanState::InvalidDemand))?;
        let stamp = self.next_stamp();
        let occurrence = self
            .occurrences
            .push(
                self.identity,
                RootOccurrenceRow {
                    declaration,
                    name,
                    keys,
                    placement: RootPlacementIdentity::minted(placement),
                    indexes,
                },
                stamp,
            )
            .ok_or_else(|| SitePlanStateError::new(SitePlanState::InvalidDemand))?;
        let root_id = occurrence.wire_root_id();
        let (placement, indexes) = self
            .graph()
            .publish(&occurrence)
            .ok_or_else(|| SitePlanStateError::new(SitePlanState::StaleBinding))?;
        Ok(AdmittedRoot {
            occurrence,
            root_id,
            placement,
            indexes,
        })
    }

    /// The direct members of the Product declaration `product` names, in declaration
    /// order — each member's canonical path selector and its declared shape — or `None`
    /// if this draft holds no such declaration.
    ///
    /// A member's own members are read the same way, through [`Self::members_of`], so a
    /// walk of a declaration is navigational and materializes one level at a time. This
    /// is how a producer obtains a member's canonical path: it is published by the one
    /// owner of the declaration rows, never recomputed by comparing paths. It is also how
    /// a second root over one Product reads the declaration the draft already holds
    /// instead of resolving that resource's anchors again.
    ///
    /// `plan` is the entry capability, not a budget: reading a declaration appends nothing,
    /// and a draft built under [`AdmittedGraphInputPlan::EMPTY`] holds no declaration to
    /// read, so this answers `None` under it without a check of its own.
    pub fn product_members(
        &self,
        _plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
    ) -> Option<Vec<DeclarationMember>> {
        self.graph()
            .product_members(DurableProductIdentity::minted(product))
    }

    /// The direct members of the declaration node `path` names, in declaration order. A
    /// field and a root-scoped path declare none.
    ///
    /// A selector published by another draft, or one whose declaration row was discarded,
    /// is refused rather than answered with an empty member list: "this is not mine" and
    /// "this node declares nothing" are different facts, and a caller that classifies a
    /// declaration by its members would read the first as the second.
    ///
    /// `plan` is the entry capability, not a budget: reading appends nothing, and no
    /// selector this draft published exists under [`AdmittedGraphInputPlan::EMPTY`].
    pub fn members_of(
        &self,
        _plan: &AdmittedGraphInputPlan,
        path: &CanonicalDeclarationPathSelector,
    ) -> Result<Vec<DeclarationMember>, SitePlanStateError> {
        self.graph()
            .members_of(path)
            .ok_or_else(|| SitePlanStateError::new(SitePlanState::StaleBinding))
    }

    /// Bind one root occurrence, one canonical declaration path, and one operation target
    /// into a validated site demand.
    ///
    /// This is the sole binder. It proves that both selectors were published by this
    /// draft and still name live rows, that the path is a canonical path of exactly this
    /// occurrence's Product or exactly this occurrence's own root-scoped case, and that
    /// the one supplied target is the target that node admits. No later call accepts a
    /// second target, and the returned handle borrows nothing: the immutable borrow ends
    /// here, before [`Self::request_site`] takes the draft mutably.
    ///
    /// `plan` is the entry capability, not a budget: binding appends nothing, and both
    /// selectors it proves live are ones only an admitted construction could have
    /// published, so a draft built under [`AdmittedGraphInputPlan::EMPTY`] can present
    /// none.
    pub fn bind_occurrence_site(
        &self,
        _plan: &AdmittedGraphInputPlan,
        root: &RootOccurrenceSelector,
        path: &CanonicalDeclarationPathSelector,
        target: SemanticTarget,
    ) -> Result<OccurrenceSiteHandle, SitePlanStateError> {
        let demand = self.graph().validate(root, path, target)?;
        Ok(OccurrenceSiteHandle::new(self.identity, demand))
    }

    /// Record the application's ledger id. Required exactly when the draft has a
    /// durable root — the durable graph's identity is anchored to it; a storeless
    /// image carries none.
    pub fn set_application_identity(&mut self, id: LedgerIdBytes) {
        self.application = Some(id);
    }

    /// Mint-or-return the operation site answering the bound demand `handle` names,
    /// through the draft's one bounded [`SiteDemandPlan`].
    ///
    /// The first request for a demand appends a row; a later request for the same one
    /// returns the id already minted, so the site table carries a site per *demanded*
    /// place rather than one per declared graph node. Eagerly preseeded bounded sites
    /// (whole-payload, group-entry, index) and lazily demanded field leaves share this
    /// one mint path: they are disjoint by construction — a preseeded demand's path names
    /// a placement, group, or index node and its target is never `FieldLeaf` — so
    /// unifying them mints no different row for any production graph, while leaving no
    /// second path that can append a row the demand map cannot see.
    ///
    /// The plan retains **only** the demand key: three owned typed ordinals. The path the
    /// site encodes to is projected from that key at encode.
    ///
    /// The returned [`LegacyDraftSiteOperand`] is the only way an instruction can name a
    /// site: it is opaque, has no constructor of its own, and carries either the id the
    /// plan minted or the plan's refusal. A refusal is not a failure — the crossing is
    /// nonblocking, and the encoder refuses the image through the Sites bound — but there
    /// is no id that would not alias a fitting site, so none is carried.
    ///
    /// `plan` is the entry capability. The site table is its own bounded owner — the plan
    /// admits graph input, not site capacity — and the handle a request spends could only
    /// have been bound against rows an admitted construction published.
    pub fn request_site(
        &mut self,
        _plan: &AdmittedGraphInputPlan,
        handle: &OccurrenceSiteHandle,
    ) -> Result<LegacyDraftSiteOperand, SitePlanStateError> {
        if handle.draft() != self.identity {
            return Err(SitePlanStateError::new(SitePlanState::WrongPlan));
        }
        // The rows the handle was bound against may have been discarded since; rebinding
        // the same triple against the live tables is what proves they were not.
        let demand = handle.demand();
        let stamp = self.next_stamp();
        let live = self.graph().revalidate(&demand)?;
        Ok(self.sites.request(self.identity, live, stamp))
    }

    /// Append a function body, validating every operation site its code names first.
    ///
    /// A site operand is evidence that *this* draft answered for a place. Appending code
    /// is where that evidence is spent, so it is checked here rather than trusted: an
    /// operand minted by another draft, or one whose site row or policy receipt was
    /// discarded, is refused and **no** row is appended. The success carrier is unchanged
    /// — a function is still named by its [`FuncId`] — so the check widens neither
    /// function identity nor the site-binding state error's authority.
    pub fn add_function(&mut self, def: FunctionDef) -> Result<FuncId, SitePlanStateError> {
        for instr in &def.code {
            if let Some(site) = instr.site_operand() {
                self.sites
                    .validate(self.identity, site)
                    .map_err(SitePlanStateError::new)?;
            }
        }
        let id = FuncId(self.functions.len() as u16);
        self.functions.push(def);
        Ok(id)
    }

    /// Bind the export identity `id` to function `func`. The compiler mints `id`
    /// with [`ExportId::of_local`] from the export's declaration path; at v0 each
    /// public function is one export, so `func` is unique across the table.
    pub fn add_export(&mut self, id: ExportId, func: FuncId) {
        self.exports.push(ExportDef { id, func });
    }

    /// Bind the report name `name` to the storeless test function `func`. Test names
    /// are unique across the project (the compiler rejects a duplicate), so the
    /// encoder sorts entries by their final name-string index.
    pub fn add_test_entry(&mut self, name: StrId, func: FuncId) {
        self.test_entries.push(TestEntryDef { name, func });
    }

    /// The number of record types (image `TypeId` ceiling) currently reserved.
    pub fn record_type_count(&self) -> usize {
        self.types.len()
    }

    /// The number of enum types (image `EnumId` ceiling) currently reserved.
    pub fn enum_type_count(&self) -> usize {
        self.enums.len()
    }

    /// Borrow this draft for one isolated generic-template proof pass.
    ///
    /// The returned guard holds the draft exclusively and discards everything the pass
    /// appends when it drops. The checkpoint it records is its own private interior: there is
    /// no mark value, so a pass cannot roll one draft back onto another.
    #[doc(hidden)]
    pub fn template_proof(&mut self) -> TemplateProofDraftGuard<'_> {
        let checkpoint = self.checkpoint();
        TemplateProofDraftGuard {
            draft: self,
            checkpoint,
        }
    }

    /// The mark a [`TemplateProofDraftGuard`] rolls back to.
    ///
    /// The draft is destructured exhaustively rather than read field by field: a new owner
    /// stops this compiling until it is either recorded here or bound to `_` beside the two
    /// owners whose exclusion [`DraftCheckpoint`] states, so no field can be left out of the
    /// rollback by omission.
    fn checkpoint(&self) -> DraftCheckpoint {
        let Self {
            identity: _,
            next_stamp: _,
            strings,
            consts,
            types,
            enums,
            colls,
            products,
            occurrences,
            product_conflict,
            application,
            value_shapes,
            sites,
            functions,
            exports,
            test_entries,
        } = self;
        DraftCheckpoint {
            strings: strings.len(),
            consts: consts.len(),
            types: types.len(),
            enums: enums.len(),
            colls: colls.len(),
            products: products.declarations().len(),
            occurrences: occurrences.len(),
            value_shapes: value_shapes.len(),
            sites: sites.rows().len(),
            functions: functions.len(),
            exports: exports.len(),
            test_entries: test_entries.len(),
            application: *application,
            product_conflict: *product_conflict,
            receipt: sites.receipt(),
        }
    }

    /// The draft's one durable value-shape arena, for the compiler to mint a field's
    /// value shape into. A declaration row can only carry a reference minted here, so
    /// there is no second place a value shape can come from.
    pub fn value_shapes_mut(&mut self) -> &mut CanonicalValueShapeDag {
        &mut self.value_shapes
    }

    /// The draft's one durable value-shape arena, for reading a minted shape's depth,
    /// kind, and references.
    pub fn value_shapes(&self) -> &CanonicalValueShapeDag {
        &self.value_shapes
    }

    /// This draft's durable contract graph, borrowed in place.
    ///
    /// The graph is not a fifth owner: it is the view spine over the four this draft
    /// already holds — the application identity, the canonical Product declaration
    /// table, the flat root-occurrence table, and the one value-shape DAG. Nothing is
    /// copied and nothing is allocated, so a Product's member graph is stored once
    /// however many roots occur over it and the contract identity is computed over
    /// exactly the rows the DURABLE section is written from.
    pub fn contract_view(&self) -> DurableContractView<'_> {
        DurableContractView::over(
            self.application,
            &self.products,
            &self.occurrences,
            &self.value_shapes,
        )
    }

    // --- accessors used by the encoder ---
    pub(crate) fn strings(&self) -> &[String] {
        &self.strings
    }
    pub(crate) fn consts(&self) -> &[ConstValue] {
        &self.consts
    }
    pub(crate) fn types(&self) -> &[RecordTypeDef] {
        &self.types
    }
    pub(crate) fn enums(&self) -> &[EnumTypeDef] {
        &self.enums
    }
    pub(crate) fn collections(&self) -> &[CollectionTypeDef] {
        &self.colls
    }
    /// The flat root-occurrence rows, in declaration order.
    pub(crate) fn root_occurrences(&self) -> &[RootOccurrence] {
        self.occurrences.rows()
    }

    /// The Product declaration one occurrence row projects.
    pub(crate) fn declaration_of(&self, occurrence: &RootOccurrence) -> &ProductDeclaration {
        self.products.declaration(occurrence.declaration())
    }

    /// The admitted Product declarations, each retained once however many roots occur
    /// over it.
    pub(crate) fn product_declarations(&self) -> &[ProductDeclaration] {
        self.products.declarations()
    }

    /// The first divergent repeat of an already-declared Product, if one was appended.
    pub(crate) fn product_conflict(&self) -> Option<ProductClaimConflict> {
        self.product_conflict
    }
    pub(crate) fn application_identity(&self) -> Option<LedgerIdBytes> {
        self.application
    }
    /// The site table the encoder writes: each retained demand projected into the
    /// semantic path of the node it addresses, handed to `emit` in emission order.
    ///
    /// The projection is the site table's only path source. A row retains ordinals into
    /// the occurrence and declaration tables, so the path a site encodes to is derived
    /// from the same rows the DURABLE section's member graph is written from and cannot
    /// disagree with them.
    ///
    /// The rows are streamed rather than returned as a table: the encoder writes each
    /// site straight into the section it is building, and its coherence preflight walks
    /// the same projection while retaining nothing, so neither holds a second copy of
    /// the site table.
    pub(crate) fn project_sites(
        &self,
        mut emit: impl FnMut(SiteDef),
    ) -> Result<(), ImageBuildError> {
        if self.sites.rows().is_empty() {
            return Ok(());
        }
        let application = self
            .application_identity()
            .ok_or(ImageBuildError::InvalidReference("application identity"))?;
        let graph = self.graph();
        for row in self.sites.rows() {
            let path = graph
                .project_path(application, row.key())
                .ok_or(ImageBuildError::InvalidReference("operation site"))?;
            emit(SiteDef {
                path,
                target: row.key().target(),
            });
        }
        Ok(())
    }

    /// The number of site rows [`ImageDraft::project_sites`] will emit. The site table is
    /// length-prefixed and streamed, so the encoder writes this count before the rows it
    /// has not projected yet; every retained row projects exactly one site.
    pub(crate) fn site_row_count(&self) -> usize {
        self.sites.rows().len()
    }

    /// The plan's logical site demand, saturating at `MAX_SITES + 1`. The encoder's Sites
    /// bound reads this rather than the retained row count: the plan refuses to mint past
    /// its capacity, so the row count can never exceed the bound and reading it would
    /// silently disable the check.
    pub(crate) fn site_demand(&self) -> usize {
        self.sites.demanded()
    }
    pub(crate) fn functions(&self) -> &[FunctionDef] {
        &self.functions
    }

    /// The `(ExportId, function-index)` pairs for the export table.
    pub(crate) fn export_entries(&self) -> Vec<(ExportId, u16)> {
        self.exports
            .iter()
            .map(|export| (export.id, export.func.0))
            .collect()
    }

    /// The `(raw name StrId, function-index)` pairs for the TEST-ENTRY table. The
    /// encoder remaps the name index through the string sort map and sorts.
    pub(crate) fn test_entry_rows(&self) -> Vec<(u16, u16)> {
        self.test_entries
            .iter()
            .map(|entry| (entry.name.raw(), entry.func.0))
            .collect()
    }
}

fn const_eq(a: ConstValue, b: ConstValue) -> bool {
    match (a, b) {
        (ConstValue::Int(x), ConstValue::Int(y)) => x == y,
        (ConstValue::Bool(x), ConstValue::Bool(y)) => x == y,
        (ConstValue::Text(x), ConstValue::Text(y)) => x.0 == y.0,
        (ConstValue::Date(x), ConstValue::Date(y)) => x == y,
        (ConstValue::Instant(x), ConstValue::Instant(y)) => x == y,
        (ConstValue::Duration(x), ConstValue::Duration(y)) => x == y,
        _ => false,
    }
}

impl StrId {
    /// The string-pool id at `index`.
    ///
    /// A logical string id is a pool position, not a capability: the independent verifier
    /// reads one from received bytes and must be able to state it, and every owner that
    /// resolves one checks it against the pool it indexes.
    pub fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// The raw logical index.
    pub fn index(self) -> u16 {
        self.0
    }

    pub(crate) fn raw(self) -> u16 {
        self.0
    }
}

impl ConstValue {
    /// A sort key `(tag, payload-bytes)` where the Text payload is the *final*
    /// string index resolved through `str_map`.
    pub(crate) fn sort_key(self, str_map: &[u16]) -> (u8, Vec<u8>) {
        match self {
            ConstValue::Int(v) => (0x01, v.to_be_bytes().to_vec()),
            ConstValue::Bool(v) => (0x02, vec![u8::from(v)]),
            ConstValue::Text(s) => (0x03, str_map[s.0 as usize].to_be_bytes().to_vec()),
            ConstValue::Date(v) => (0x04, v.to_be_bytes().to_vec()),
            ConstValue::Instant(v) => (0x05, v.to_be_bytes().to_vec()),
            ConstValue::Duration(v) => (0x06, v.to_be_bytes().to_vec()),
        }
    }
}

#[cfg(test)]
mod collection_count_tests {
    use super::{CollectionTypeDef, ImageDraft};
    use crate::{ImageType, Scalar};

    #[test]
    fn collection_type_count_tracks_the_next_published_id() {
        let mut draft = ImageDraft::new();
        assert_eq!(draft.collection_type_count(), 0);

        let list = draft.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        });
        assert_eq!(list.index(), 0);
        assert_eq!(draft.collection_type_count(), 1);

        let map = draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Text),
            value: ImageType::scalar(Scalar::Bool),
        });
        assert_eq!(map.index(), 1);
        assert_eq!(draft.collection_type_count(), 2);
    }
}

#[cfg(test)]
mod site_binding_tests {
    use super::{AdmittedGraphInputPlan, ImageDraft, RootOccurrenceDef, TypeId};
    use crate::durable_id::LedgerIdBytes;
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape};
    use crate::semantic::SemanticTarget;
    use crate::site_plan::{SitePlanState, SitePlanStateError};
    use crate::ty::Scalar;

    /// The construction budget these fixtures are admitted under: one Product, two root
    /// occurrences (the two-draft cases build one each), and the image's own command
    /// ceiling.
    fn plan() -> AdmittedGraphInputPlan {
        AdmittedGraphInputPlan::admit(1, 2, crate::bounds::MAX_ADMITTED_DECLARATION_COMMANDS)
            .expect("a one-Product fixture census is within what an image can hold")
    }

    fn product() -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([0x11; 16])
    }

    fn placement() -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([0x22; 16])
    }

    fn field() -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([0x33; 16])
    }

    /// A draft holding one Product of one required int field and one keyless root over it.
    fn one_root() -> (ImageDraft, super::AdmittedRoot) {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(LedgerIdBytes::from_bytes([0x01; 16]));
        let name = draft.intern_string("r");
        let value = draft.value_shapes_mut().scalar(Scalar::Int);
        draft
            .declare_product(
                &plan(),
                product(),
                TypeId(0),
                vec![DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: field(),
                        required: true,
                        value,
                    },
                }],
            )
            .expect("a well-formed declaration");
        let admitted = draft
            .add_root_occurrence(
                &plan(),
                product(),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: placement(),
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");
        (draft, admitted)
    }

    /// The three private refusal cases, each reached through the public binder.
    ///
    /// The public type is one opaque invariant, so the discriminant is only ever observed
    /// here: it is what makes "wrong draft", "the row is gone", and "that is not a place"
    /// distinguishable to the owner without publishing three of them to every consumer.
    #[test]
    fn the_binder_distinguishes_its_three_refusals() {
        let (first, first_root) = one_root();
        let (second, _) = one_root();

        let mine = first.product_members(&plan(), product()).expect("declared");
        let theirs = second
            .product_members(&plan(), product())
            .expect("declared");

        // A path selector published by another draft is a wrong-plan refusal, however
        // identical the two graphs look.
        assert_eq!(
            first
                .bind_occurrence_site(
                    &plan(),
                    first_root.occurrence(),
                    theirs[0].path(),
                    SemanticTarget::FieldLeaf,
                )
                .expect_err("a foreign selector cannot bind"),
            SitePlanStateError::new(SitePlanState::WrongPlan),
        );

        // A target the node does not admit is an invalid demand: a field admits only a
        // field-leaf read or write, and a root placement only a whole payload.
        assert_eq!(
            first
                .bind_occurrence_site(
                    &plan(),
                    first_root.occurrence(),
                    mine[0].path(),
                    SemanticTarget::WholePayload,
                )
                .expect_err("a field admits no whole-payload site"),
            SitePlanStateError::new(SitePlanState::InvalidDemand),
        );
        assert_eq!(
            first
                .bind_occurrence_site(
                    &plan(),
                    first_root.occurrence(),
                    first_root.placement_path(),
                    SemanticTarget::FieldLeaf,
                )
                .expect_err("a placement admits no field-leaf site"),
            SitePlanStateError::new(SitePlanState::InvalidDemand),
        );

        // The one target each admits does bind, so the refusals above are about the
        // target and not about the pair being unbindable.
        assert!(
            first
                .bind_occurrence_site(
                    &plan(),
                    first_root.occurrence(),
                    mine[0].path(),
                    SemanticTarget::FieldLeaf,
                )
                .is_ok()
        );
    }

    /// Discarding the rows a handle was bound against makes it stale, and the ordinal
    /// being reused afterwards does not revive it.
    #[test]
    fn a_handle_over_a_discarded_row_is_stale_even_when_its_ordinal_is_reused() {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(LedgerIdBytes::from_bytes([0x01; 16]));
        // Build the rows inside a proof guard, so dropping it discards exactly them.
        let handle = {
            let mut guard = draft.template_proof();
            let proof = guard.proof_draft();
            let name = proof.intern_string("r");
            let value = proof.value_shapes_mut().scalar(Scalar::Int);
            proof
                .declare_product(
                    &plan(),
                    product(),
                    TypeId(0),
                    vec![DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Field {
                            id: field(),
                            required: true,
                            value,
                        },
                    }],
                )
                .expect("a well-formed declaration");
            let admitted = proof
                .add_root_occurrence(
                    &plan(),
                    product(),
                    RootOccurrenceDef {
                        name,
                        keys: Vec::new(),
                        placement: placement(),
                        indexes: Vec::new(),
                    },
                )
                .expect("the Product is declared");
            proof
                .bind_occurrence_site(
                    &plan(),
                    admitted.occurrence(),
                    admitted.placement_path(),
                    SemanticTarget::WholePayload,
                )
                .expect("the root admits a whole-payload site")
        };

        assert_eq!(
            draft
                .request_site(&plan(), &handle)
                .expect_err("the occurrence row was discarded"),
            SitePlanStateError::new(SitePlanState::StaleBinding),
        );

        // The same ordinals are re-minted deterministically; the handle must still not
        // authenticate the replacement.
        let name = draft.intern_string("r");
        let value = draft.value_shapes_mut().scalar(Scalar::Int);
        draft
            .declare_product(
                &plan(),
                product(),
                TypeId(0),
                vec![DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: field(),
                        required: true,
                        value,
                    },
                }],
            )
            .expect("a well-formed declaration");
        draft
            .add_root_occurrence(
                &plan(),
                product(),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: placement(),
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");

        assert_eq!(
            draft
                .request_site(&plan(), &handle)
                .expect_err("a re-minted row is not the row the handle was bound against"),
            SitePlanStateError::new(SitePlanState::StaleBinding),
        );
    }

    /// A command vector that does not state a forest is refused before any row is
    /// appended, so a malformed declaration cannot reach the encoder at all.
    #[test]
    fn a_malformed_command_vector_appends_no_row() {
        let mut draft = ImageDraft::new();

        let refusal = draft
            .declare_product(
                &plan(),
                product(),
                TypeId(0),
                vec![DeclarationMemberDef {
                    parent: Some(0),
                    shape: DeclarationMemberShape::Group { id: field() },
                }],
            )
            .expect_err("a command cannot be its own parent");

        assert_eq!(
            refusal,
            SitePlanStateError::new(SitePlanState::InvalidDemand)
        );
        assert!(draft.product_members(&plan(), product()).is_none());
        assert!(draft.root_occurrences().is_empty());
    }
}
