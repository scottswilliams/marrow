//! The typed validating `ImageDraft`.
//!
//! The compiler mutates a draft only through the journaled [`DraftTxn`] that
//! [`ImageDraft::begin_transaction`] arms, and calls [`ImageDraft::encode`] for canonical
//! container bytes with a computed digest. Building works in logical intern ids; the
//! encoder sorts the string and constant pools into canonical order and rewrites every
//! reference, so the compiler never reasons about final pool positions.
//!
//! Sites are minted only through the bounded [`SiteDemandPlan`], by binding a live root
//! occurrence to a live canonical declaration path, so a producer cannot address a node
//! the graph does not contain. The draft's checks are a producer-side guard, not the
//! trust boundary: the independent verifier rechecks every bound against received bytes.
//!
//! Every owned pre-seal id carries a wide `u32` ordinal, so an over-policy table still
//! mints the N+1 id and is refused at the encode fence, whose measure core performs the
//! only narrowing to the wire's `u16` spelling.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bounds;
use crate::durable_id::{
    DurableContractView, DurableIndexShape, DurableProductIdentity, LedgerIdBytes,
};
use crate::encode::SPAN_ROW_BYTES;
use crate::export_id::ExportId;
use crate::instr::Instr;
use crate::product::{
    CanonicalDeclarationPathSelector, DeclarationMember, DeclarationMemberDef,
    DurableContractGraph, DurableGraphCheckpoint, OccurrenceGraph, ProductClaimConflict,
    ProductDeclaration, RootOccurrence, RootOccurrenceSelector,
};
use crate::semantic::SemanticTarget;
use crate::site_plan::{
    OccurrenceSiteHandle, PlannedSiteRef, SiteDemandPlan, SitePlanState, SitePlanStateError,
    SitePolicyReceipt,
};
use crate::ty::{ImageType, Scalar};
use crate::value_dag::{CanonicalValueShapeDag, ImageByteSink, ValueShapeLeaf, ValueShapeNodeId};

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
/// The counts are private and [`Self::admit`] is the only constructor, so a plan carrying
/// unadmitted counts has no literal form outside this module.
///
/// It is a budget, not permission to reach it: every table the entry points append to
/// still rechecks its own bound, and the plan bounds *intake* while classifying nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedGraphInputPlan {
    products: usize,
    roots: usize,
    commands: usize,
}

impl AdmittedGraphInputPlan {
    /// The plan a storeless compile carries: no durable construction is admitted at all.
    /// A storeless image declares no Product, occurs no root, and states no member, so
    /// its budget is zero rather than absent.
    pub const EMPTY: Self = Self {
        products: 0,
        roots: 0,
        commands: 0,
    };

    /// The construction budget for `products` Product declarations, `roots` root
    /// occurrences, and `commands` member commands in any one declaration.
    ///
    /// Each term saturates one past the bound whose refusal owner keeps its refusal (see
    /// [`bounds::MAX_ADMITTED_ROOT_OCCURRENCES`]), so a census that overruns still reaches
    /// [`ImageBuildError::TooManyRoots`] or [`ImageBuildError::TooManyDurableMembers`]
    /// over a *complete* graph instead of being truncated at the entry point.
    pub fn admit(products: usize, roots: usize, commands: usize) -> Self {
        Self {
            products: products.min(bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS),
            roots: roots.min(bounds::MAX_ADMITTED_ROOT_OCCURRENCES),
            commands: commands.min(bounds::MAX_ADMITTED_DECLARATION_COMMANDS),
        }
    }

    pub(crate) fn products(&self) -> usize {
        self.products
    }

    pub(crate) fn roots(&self) -> usize {
        self.roots
    }

    /// Member commands admitted in any one declaration.
    pub(crate) fn commands(&self) -> usize {
        self.commands
    }
}

/// Mint the next logical ordinal for an owned pre-seal table of `len` rows.
///
/// The domain checked here is the `u32` the id newtypes hold, not a public policy
/// maximum: an over-policy table still mints the N+1 id and the encode fence's policy
/// walk refuses the image. A caller at the `u32` boundary receives the carrier-domain
/// refusal before any owner mutates.
pub(crate) fn wide_ordinal(len: usize) -> Result<u32, DraftStateError> {
    u32::try_from(len).map_err(|_| DraftStateError::CarrierDomain)
}

/// The function-slot ordinal, checked at its mint.
///
/// Function width stays `u16` here, so a draft carrying more functions than the carrier
/// spells would wrap and alias slot zero. The mint returns the builder-domain refusal
/// rather than leaving the bound to the encoder to notice afterwards.
fn function_ordinal(len: usize) -> Result<u16, DraftStateError> {
    u16::try_from(len).map_err(|_| DraftStateError::CarrierDomain)
}

/// A logical string-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(u32);

/// A logical constant-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstId(pub(crate) u32);

impl ConstId {
    /// The constant-pool id at `index`, widened from a `u16` ordinal. Production ids
    /// come from the draft's own checked interning mints; this spells a known-answer
    /// operand against a pool the caller already built.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical ordinal, as carried in a `ConstLoad` operand until the encoder
    /// rewrites it to the final sorted pool position. Never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A record-type index (also the final container index; types keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub(crate) u32);

impl TypeId {
    /// The record-type index at `index` — the widening of a received `u16` wire read.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical record-type ordinal, as carried in a `RecordNew` operand and
    /// in an `ImageType::Record`. Never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The `u16` ordinal this reference is spelled with in an image operand and in the
    /// kernel's value domain. Total over every id a verified image holds (each widened from
    /// a `u16` read) and every id of a policy-clean draft (`bounds` asserts each table
    /// maximum within `u16`).
    ///
    /// # Panics
    ///
    /// On a draft id minted past `u16::MAX`: a draft mints wide ordinals before its encode
    /// fence refuses an over-policy table, so call this only on a verified or policy-clean
    /// id.
    pub fn wire_index(self) -> u16 {
        crate::measure::wire_ordinal(self.0)
    }
}

/// An enum-type index (also the final container index; enums keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnumId(pub(crate) u32);

impl EnumId {
    /// The enum-type index at `index` — the widening of a received `u16` wire read.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical enum-type ordinal, as carried in `EnumConstruct` operands and
    /// in an `ImageType::Enum`. Never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The `u16` ordinal this reference is spelled with; total, and panicking, on the same
    /// terms as [`TypeId::wire_index`].
    pub fn wire_index(self) -> u16 {
        crate::measure::wire_ordinal(self.0)
    }
}

/// A collection-type index (also the final container index; collection types keep
/// insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CollTypeId(pub(crate) u32);

impl CollTypeId {
    /// The collection-type index at `index` — the widening of a received `u16` wire
    /// read.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical collection-type ordinal, as carried in `ListNew`/`MapNew`
    /// operands and in an `ImageType::Collection`. Never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The `u16` ordinal this reference is spelled with; total, and panicking, on the same
    /// terms as [`TypeId::wire_index`].
    pub fn wire_index(self) -> u16 {
        crate::measure::wire_ordinal(self.0)
    }
}

/// A durable root reference: the wide logical ordinal of one row in the flat
/// root-occurrence table — the reference an `ImageType::Identity` and a
/// `MakeIdentity` instruction embed, and the fact [`AdmittedRoot::root_id`] publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RootId(pub(crate) u32);

impl RootId {
    /// The root reference at `index` — the widening of a received `u16` wire read.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical occurrence ordinal. Never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The `u16` ordinal this reference is spelled with; total, and panicking, on the same
    /// terms as [`TypeId::wire_index`].
    pub fn wire_index(self) -> u16 {
        crate::measure::wire_ordinal(self.0)
    }
}

/// A reserved function index (also the final container index).
///
/// Callers must retain IDs from this draft's current state. The index carries no
/// provenance and does not distinguish another draft or a rolled-back reservation.
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
/// A draft never takes one: a draft instruction names its site by the opaque
/// [`PlannedSiteRef`] the plan mints, so no caller-written number reaches the site table.
/// A verified instruction carries the id its operand decoded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SiteId(u16);

impl SiteId {
    /// The site row at `index` — a received `u16` operand, or a row the site demand plan
    /// minted after checking vacant capacity.
    pub const fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// The raw site index, as carried in a `Dur*` operand.
    pub const fn index(self) -> u16 {
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
/// payload leaf is a bare (non-optional) [`ImageType`]: a scalar, a record, or
/// another enum, whether the member was declared or monomorphized from a generic
/// template. Payload order is the declaration order — the canonical product-leaf
/// order the checker owns.
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
    ///
    /// Held as a shared owner, the way a Product's member rows are: a caller that keeps
    /// its own handle on the list it admitted — the independent verifier does — refers to
    /// this one allocation rather than to a second copy of every projection.
    pub indexes: Rc<[DurableIndexShape]>,
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
    root_id: RootId,
    placement: CanonicalDeclarationPathSelector,
    indexes: Vec<CanonicalDeclarationPathSelector>,
}

impl AdmittedRoot {
    /// The selector naming this occurrence row.
    pub fn occurrence(&self) -> &RootOccurrenceSelector {
        &self.occurrence
    }

    /// The typed durable root reference of this occurrence — the wide logical ordinal
    /// whose `u16` narrowing is the discriminant an entry identity `Id(^root)` carries
    /// on the wire. A fact the compiler embeds into identity instructions, not a way to
    /// name the occurrence row: that is what the selector is for.
    pub fn root_id(&self) -> RootId {
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
pub(crate) struct ExportDef {
    id: ExportId,
    func: FuncId,
}

impl ExportDef {
    pub(crate) fn id(&self) -> &ExportId {
        &self.id
    }

    /// The bound function's table index, as the wire spells it.
    pub(crate) fn func(&self) -> u16 {
        self.func.0
    }
}

/// A test entry: a report-name string bound to a storeless zero-argument function
/// `marrow test` runs. Unlike an export it carries no wire identity — the name is a
/// human report label only, never an interface, demand, or durable identity.
#[derive(Debug, Clone)]
pub(crate) struct TestEntryDef {
    name: StrId,
    func: FuncId,
}

impl TestEntryDef {
    pub(crate) fn name(&self) -> StrId {
        self.name
    }

    /// The bound function's table index, as the wire spells it.
    pub(crate) fn func(&self) -> u16 {
        self.func.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// What an incoherent draft reference names. The encode fence reports the first
/// one it reaches, so the kind is the whole payload: there is no second reference of
/// the same kind to disambiguate within one verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// The application ledger identity a non-empty durable graph must be anchored by.
    ApplicationIdentity,
    /// A durable branch's name string.
    BranchName,
    /// A `Call` operand's function-table position.
    CallTarget,
    /// A COLLTYPES-table position.
    CollectionType,
    /// A constant-pool position.
    Constant,
    /// An enum definition's name string.
    EnumName,
    /// An ENUMS-table position, or a variant of the enum it names.
    EnumType,
    /// An EXPORTS-table relation: one export per function, one row per id.
    ExportTable,
    /// An export row's function-table position.
    ExportTarget,
    /// A record field's name string.
    FieldName,
    /// A function's name string.
    FunctionName,
    /// A function's source-path string.
    FunctionSource,
    /// A transfer operand's position in its own instruction list.
    JumpTarget,
    /// A durable operation site's live provenance.
    OperationSite,
    /// A record type's name string.
    RecordName,
    /// A root occurrence's name string.
    RootName,
    /// A ROOTS-table position, or a root's key arity.
    RootTable,
    /// A span row's instruction position in its own function.
    SpanInstruction,
    /// A test entry's name string.
    TestName,
    /// A TEST-ENTRY relation: uniqueness, disjointness, signature, or reachability.
    TestTable,
    /// A test entry's function-table position.
    TestTarget,
    /// A text constant's string-pool position.
    TextConstant,
    /// A TYPES-table position.
    TypeTable,
    /// A reserved enum row still unfilled at the encode fence.
    VacantEnumType,
    /// A reserved function row still unfilled at the encode fence.
    VacantFunction,
    /// A reserved record row still unfilled at the encode fence.
    VacantRecordType,
    /// A durable value-shape arena node.
    ValueShape,
    /// An enum variant's name string.
    VariantName,
}

/// A failure to build a well-formed draft: a bound exceeded or an invalid
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
    /// A divergent application ledger identity was set after one was recorded: two
    /// applications wearing one draft. The first identity is retained and the
    /// divergence is latched as a sticky coherence fact the fence reports.
    ApplicationIdentityConflict,
    InvalidReference(ReferenceKind),
}

impl std::fmt::Display for ImageBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "image build error: {self:?}")
    }
}

impl std::error::Error for ImageBuildError {}

/// The mutable image builder.
///
/// Every mutation flows through the journaled [`DraftTxn`] that
/// [`Self::begin_transaction`] arms over an exclusive borrow. A draft is deliberately not
/// `Clone`: every selector, handle, and site operand it mints carries its identity, so a
/// copy would authenticate capabilities against both drafts.
///
/// ```compile_fail,E0599
/// let draft = marrow_image::ImageDraft::new();
/// let _copy = draft.clone();
/// ```
#[derive(Debug)]
pub struct ImageDraft {
    /// The one durable-graph owner: this draft's strong identity and stamp source, its
    /// application identity, its canonical Product declaration table, its flat
    /// root-occurrence table, and the one value-shape arena their fields reference. It is
    /// the same owner the independent verifier builds from received bytes, so both sides
    /// are admitted, stamped, and bounded by one implementation.
    durable: DurableContractGraph,
    strings: Vec<String>,
    /// Lookup-only interning projection; the vector remains the canonical order.
    string_index: HashMap<String, StrId>,
    consts: Vec<ConstValue>,
    /// Lookup-only interning projection; the vector remains the canonical order.
    const_index: HashMap<ConstValue, ConstId>,
    types: Vec<RecordTypeDef>,
    /// One-time-fill state per record row, in lockstep with `types`.
    types_fill: Vec<FillState>,
    enums: Vec<EnumTypeDef>,
    /// One-time-fill state per enum row, in lockstep with `enums`.
    enums_fill: Vec<FillState>,
    colls: Vec<CollectionTypeDef>,
    /// The first divergent repeat of an already-declared Product, if one was appended.
    /// The draft cannot represent two declarations wearing one identity, so the
    /// coherence pass refuses to encode rather than canonicalizing one of them away.
    product_conflict: Option<ProductClaimConflict>,
    /// The sticky application-identity divergence latch: set once, or set to the same.
    application_conflict: Option<ApplicationIdentityConflict>,
    /// The one owner of the operation-site table, its demand map, and its capacity
    /// policy. Every site an image carries is requested through it.
    sites: SiteDemandPlan,
    functions: Vec<Option<FunctionDef>>,
    /// The bytes the retained function bodies alone commit the image to, saturated at
    /// one past [`bounds::MAX_IMAGE_BYTES`] (see [`Self::function_payload_exceeds_image_limit`]).
    function_payload_charge: usize,
    exports: Vec<ExportDef>,
    test_entries: Vec<TestEntryDef>,
}

/// The charge at which the function payload alone proves the image cannot fit.
const DECISIVE_FUNCTION_PAYLOAD: usize = bounds::MAX_IMAGE_BYTES + 1;

/// The bytes one appended body commits the image to: every opcode encodes to at least
/// one byte and every span row to exactly [`SPAN_ROW_BYTES`], while operands, headers,
/// and sections only add. The charge can therefore miss an oversized image but never
/// exceed one that fits.
fn function_payload_floor(def: &FunctionDef) -> usize {
    def.code
        .len()
        .saturating_add(SPAN_ROW_BYTES.saturating_mul(def.spans.len()))
}

/// The one-time-fill state of a reserved record or enum row: a row is minted
/// unfilled and admits exactly one later fill. Distinct from the definition's own
/// field count — a row minted complete simply never spends its fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillState {
    Unfilled,
    Filled,
}

/// The sticky application-identity divergence latch: the retained first identity and the
/// first divergent replacement, reported once at the encode fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplicationIdentityConflict {
    first: LedgerIdBytes,
    divergent: LedgerIdBytes,
}

/// A hostile-state refusal of the transaction surface: the closed set the mutation entry
/// points return before any owner changes. Never a policy maximum — crossing a public
/// image policy is not a returned error anywhere on this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftStateError {
    /// The id or reference was minted by another draft, or names a row this draft does
    /// not hold.
    ForeignDraft,
    /// The id names a row of this draft whose state refuses the operation: a fill of a
    /// row already filled, or a site operand whose row or receipt the plan no longer
    /// holds.
    RowState,
    /// The argument exceeds the proved carrier/layout domain of the builder surface. The
    /// production compiler maps this to a compiler invariant — never a policy or source
    /// refusal.
    CarrierDomain,
}

impl std::fmt::Display for DraftStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DraftStateError::ForeignDraft => "the id was minted by another draft",
            DraftStateError::RowState => "the row's state refuses the operation",
            DraftStateError::CarrierDomain => {
                "the argument exceeds the proved carrier domain of the builder surface"
            }
        })
    }
}

impl std::error::Error for DraftStateError {}

/// A site operation the draft did not answer for is a row-state refusal at the builder
/// surface. The site error's own private cases stay private: this crossing carries the
/// classification, not the cause.
impl From<crate::site_plan::SitePlanStateError> for DraftStateError {
    fn from(_: crate::site_plan::SitePlanStateError) -> Self {
        DraftStateError::RowState
    }
}

/// One prepared string mint: the id the row will carry, and — when the row is new — the
/// spelling to append.
struct PreparedString {
    id: StrId,
    fresh: Option<FreshString>,
}

struct FreshString {
    text: String,
}

/// One prepared constant mint (see [`PreparedString`]).
struct PreparedConst {
    id: ConstId,
    fresh: Option<FreshConst>,
}

struct FreshConst {
    value: ConstValue,
}

/// The structural image a transaction's journal restores to: every owner's append-only
/// length, the durable graph's checkpoint, and the conflict/receipt slots. Fill state is
/// not scanned: a fill of a pre-transaction row is journaled individually.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DraftSnapshot {
    strings: usize,
    consts: usize,
    types: usize,
    enums: usize,
    colls: usize,
    durable: DurableGraphCheckpoint,
    sites: usize,
    functions: usize,
    function_payload_charge: usize,
    exports: usize,
    test_entries: usize,
    product_conflict: Option<ProductClaimConflict>,
    application_conflict: Option<ApplicationIdentityConflict>,
    receipt: Option<SitePolicyReceipt>,
}

/// One journaled fill of a pre-transaction row. Functions restore `None` by index;
/// record and enum rows retain their displaced definition. A suffix fill needs no
/// entry because its row truncates. No inverse allocates or clones a body.
#[derive(Debug)]
enum FillInverse {
    Function {
        row: usize,
    },
    Record {
        row: usize,
        fields: Vec<FieldDef>,
    },
    Enum {
        row: usize,
        variants: Vec<VariantDef>,
    },
}

/// The pre-reserved inverse journal the armed guard holds: the structural image at
/// admission and the one-time-fill inverses. Every element's
/// storage is reserved in the preflight of the mutation that needs it, so the armed
/// `Drop` inverse is allocation-free, assertion-free, indexing-free, and non-panicking.
#[derive(Debug)]
struct DraftJournal {
    at: DraftSnapshot,
    fills: Vec<FillInverse>,
}

/// The sole mutation surface over one [`ImageDraft`]: an armed guard
/// [`ImageDraft::begin_transaction`] hands out over an exclusive borrow. It mutates the
/// draft immediately and in place — never batching, deferring, or reordering a call,
/// which is what keeps mint order equal to wire order — while journaling the inverses an
/// armed rollback needs. [`DraftTxn::commit`] disarms and retains every mutation; the
/// armed `Drop` performs the total inverse.
///
/// Reads pass through [`std::ops::Deref`] to the draft's read surface; the guard
/// exposes no `&mut ImageDraft`, so no mutation can bypass the journal.
#[derive(Debug)]
pub struct DraftTxn<'d> {
    draft: &'d mut ImageDraft,
    journal: DraftJournal,
    armed: bool,
}

impl std::ops::Deref for DraftTxn<'_> {
    type Target = ImageDraft;

    fn deref(&self) -> &ImageDraft {
        self.draft
    }
}

impl<'d> DraftTxn<'d> {
    /// Disarm the guard, retaining every mutation and accepted policy observation.
    pub fn commit(mut self) {
        self.armed = false;
    }

    /// Run the total inverse now: the explicit spelling of what dropping the armed guard
    /// does.
    pub fn rollback(mut self) {
        self.rollback_armed();
        self.armed = false;
    }

    /// Intern a string, returning its logical id. Repeated interning of the same text
    /// returns the same id and mutates nothing, even at a full table, so dedup runs
    /// before any policy observation.
    pub fn intern_string(&mut self, text: &str) -> Result<StrId, DraftStateError> {
        let prepared = self.draft.prepare_string(text)?;
        Ok(self.draft.commit_string(prepared))
    }

    pub fn intern_int(&mut self, value: i64) -> Result<ConstId, DraftStateError> {
        self.draft.intern_const(ConstValue::Int(value))
    }

    pub fn intern_bool(&mut self, value: bool) -> Result<ConstId, DraftStateError> {
        self.draft.intern_const(ConstValue::Bool(value))
    }

    /// Intern a text constant, interning its backing string as needed. The string row,
    /// the constant row, and both index entries land as one unit: a refusal can never
    /// leave half the compound behind.
    pub fn intern_text(&mut self, text: &str) -> Result<ConstId, DraftStateError> {
        // Both halves derive from the same preimage, so both are prepared before either
        // lands: a fresh string implies a fresh `Text` constant (its `StrId` does not
        // exist yet), and the constant's ordinal does not depend on the string commit.
        let string = self.draft.prepare_string(text)?;
        let konst = self.draft.prepare_const(ConstValue::Text(string.id))?;
        self.draft.commit_string(string);
        Ok(self.draft.commit_const(konst))
    }

    /// Intern a `date` constant (days since the Unix epoch).
    pub fn intern_date(&mut self, days: i32) -> Result<ConstId, DraftStateError> {
        self.draft.intern_const(ConstValue::Date(days))
    }

    /// Intern an `instant` constant (signed nanoseconds since the epoch).
    pub fn intern_instant(&mut self, nanos: i128) -> Result<ConstId, DraftStateError> {
        self.draft.intern_const(ConstValue::Instant(nanos))
    }

    /// Intern a `duration` constant (signed nanoseconds).
    pub fn intern_duration(&mut self, nanos: i128) -> Result<ConstId, DraftStateError> {
        self.draft.intern_const(ConstValue::Duration(nanos))
    }

    /// Add a record type with its complete definition: the row never spends a fill, so
    /// a later fill is the double-fill refusal, never a replacement. A two-pass forward
    /// reference reserves with [`Self::reserve_record_type`] instead.
    pub fn add_record_type(&mut self, def: RecordTypeDef) -> Result<TypeId, DraftStateError> {
        self.draft.append_record_type(def, FillState::Filled)
    }

    /// Add an enum type with its complete definition (see [`Self::add_record_type`]).
    pub fn add_enum_type(&mut self, def: EnumTypeDef) -> Result<EnumId, DraftStateError> {
        self.draft.append_enum_type(def, FillState::Filled)
    }

    /// Reserve a `Vacant` record row for a two-pass forward reference: distinct from a
    /// filled-empty definition, it admits exactly one later fill and is the fence's
    /// coherence invariant if never filled.
    pub fn reserve_record_type(&mut self, name: StrId) -> Result<TypeId, DraftStateError> {
        self.draft.append_record_type(
            RecordTypeDef {
                name,
                fields: Vec::new(),
            },
            FillState::Unfilled,
        )
    }

    /// Reserve a `Vacant` enum row (see [`Self::reserve_record_type`]).
    pub fn reserve_enum_type(&mut self, name: StrId) -> Result<EnumId, DraftStateError> {
        self.draft.append_enum_type(
            EnumTypeDef {
                name,
                variants: Vec::new(),
            },
            FillState::Unfilled,
        )
    }

    /// Add a collection type (a concrete `List`/`Map` instantiation) whose inner types
    /// are already resolved. Appends unconditionally: the compiler's type registry owns
    /// instantiation identity and dedups by the *source* types, so `List[Age]` and
    /// `List[int]` stay distinct although a nominal element erases to the same image
    /// `int`.
    pub fn add_collection_type(
        &mut self,
        def: CollectionTypeDef,
    ) -> Result<CollTypeId, DraftStateError> {
        let id = CollTypeId(wide_ordinal(self.draft.colls.len())?);
        self.draft.colls.push(def);
        Ok(id)
    }

    /// Fill a live record reservation from this draft, exactly once. The caller owns the
    /// same-draft precondition: ordinal IDs carry no foreign/stale provenance. An absent
    /// row is [`DraftStateError::ForeignDraft`] and a filled one
    /// [`DraftStateError::RowState`]; a fill of a pre-transaction row journals its
    /// displaced definition first.
    pub fn set_record_fields(
        &mut self,
        ty: TypeId,
        fields: Vec<FieldDef>,
    ) -> Result<(), DraftStateError> {
        let row = ty.index() as usize;
        let Some(state) = self.draft.types_fill.get(row).copied() else {
            return Err(DraftStateError::ForeignDraft);
        };
        if state == FillState::Filled {
            return Err(DraftStateError::RowState);
        }
        if row < self.journal.at.types {
            self.journal.fills.reserve(1);
            let Some(slot) = self.draft.types.get_mut(row) else {
                return Err(DraftStateError::ForeignDraft);
            };
            let prior = std::mem::replace(&mut slot.fields, fields);
            self.journal
                .fills
                .push(FillInverse::Record { row, fields: prior });
        } else {
            let Some(slot) = self.draft.types.get_mut(row) else {
                return Err(DraftStateError::ForeignDraft);
            };
            slot.fields = fields;
        }
        if let Some(state) = self.draft.types_fill.get_mut(row) {
            *state = FillState::Filled;
        }
        Ok(())
    }

    /// Fill the variants of an already-reserved enum type, exactly once (see
    /// [`Self::set_record_fields`]).
    pub fn set_enum_variants(
        &mut self,
        id: EnumId,
        variants: Vec<VariantDef>,
    ) -> Result<(), DraftStateError> {
        let row = id.index() as usize;
        let Some(state) = self.draft.enums_fill.get(row).copied() else {
            return Err(DraftStateError::ForeignDraft);
        };
        if state == FillState::Filled {
            return Err(DraftStateError::RowState);
        }
        if row < self.journal.at.enums {
            self.journal.fills.reserve(1);
            let Some(slot) = self.draft.enums.get_mut(row) else {
                return Err(DraftStateError::ForeignDraft);
            };
            let prior = std::mem::replace(&mut slot.variants, variants);
            self.journal.fills.push(FillInverse::Enum {
                row,
                variants: prior,
            });
        } else {
            let Some(slot) = self.draft.enums.get_mut(row) else {
                return Err(DraftStateError::ForeignDraft);
            };
            slot.variants = variants;
        }
        if let Some(state) = self.draft.enums_fill.get_mut(row) {
            *state = FillState::Filled;
        }
        Ok(())
    }

    /// Admit one durable Product declaration: its canonical member/value graph and the
    /// entry record its roots read and write, returning the declaration's direct members
    /// exactly as [`ImageDraft::product_members`] publishes them.
    ///
    /// The one construction path for the durable graph, and it is flat: members arrive as
    /// a command vector whose rows name their parent by an earlier command, so a caller
    /// cannot hand the draft a recursive tree. A Product is held once however many roots
    /// occur over it; a later declaration of the same identity claiming a different graph
    /// or entry record is recorded, not refused here, so [`ImageDraft::encode`] reports it
    /// once in wire order. A malformed vector, or one past what `plan` admits, is one
    /// opaque [`SitePlanStateError`] with no row appended; the encoder remains the owner
    /// of the member bound.
    pub fn declare_product(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        entry_record: TypeId,
        members: Vec<DeclarationMemberDef>,
    ) -> Result<Vec<DeclarationMember>, SitePlanStateError> {
        let (row, conflict) = self
            .draft
            .durable
            .admit_product(plan, product, entry_record, members)
            .map_err(|_| SitePlanStateError::new(SitePlanState::InvalidDemand))?;
        if let Some(conflict) = conflict {
            self.draft.product_conflict.get_or_insert(conflict);
        }
        Ok(self.draft.graph().members_of_row(row))
    }

    /// Append one root occurrence over the Product declaration `product` names, returning
    /// what the completed row publishes.
    ///
    /// The row retains only the root's own placement, spelling, key tuple, and managed
    /// indexes plus a reference to the one declaration, so nothing is retained per
    /// (root x member). An undeclared Product, selectors past what a canonical path can
    /// address, or an occurrence past the plan's admitted root count are one opaque
    /// [`SitePlanStateError`], each refused before the row is pushed. Crossing the public
    /// Roots policy is not refused here: the N+1 occurrence commits and the fence reports
    /// [`ImageBuildError::TooManyRoots`].
    pub fn add_root_occurrence(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        def: RootOccurrenceDef,
    ) -> Result<AdmittedRoot, SitePlanStateError> {
        if def.indexes.len() > usize::from(u16::MAX) + 1 {
            return Err(SitePlanStateError::new(SitePlanState::InvalidDemand));
        }
        let occurrence = self
            .draft
            .durable
            .admit_root_occurrence(plan, product, def)
            .map_err(|_| SitePlanStateError::new(SitePlanState::InvalidDemand))?;
        let root_id = occurrence.wire_root_id();
        // Preflighted above, so the just-pushed row always publishes; the refusal arm is
        // defense in depth, never a panic and never an orphan row.
        let (placement, indexes) = self
            .draft
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

    /// Record the application's ledger id, set-once-or-same: an equal reset is a no-op
    /// and a divergent replacement latches the sticky conflict the fence reports,
    /// retaining the first identity. Required exactly when the draft has a durable root.
    pub fn set_application_identity(&mut self, id: LedgerIdBytes) {
        match self.draft.durable.application() {
            None => self.draft.durable.set_application_identity(id),
            Some(first) if first == id => {}
            Some(first) => {
                self.draft
                    .application_conflict
                    .get_or_insert(ApplicationIdentityConflict {
                        first,
                        divergent: id,
                    });
            }
        }
    }

    /// Mint-or-return the operation site answering the bound demand `handle` names,
    /// through the draft's one bounded [`SiteDemandPlan`].
    ///
    /// The first request for a demand appends a row; a later request for the same one
    /// returns the id already minted, so the site table carries a site per *demanded*
    /// place. The plan retains only the demand key — three owned typed ordinals — and the
    /// path a site encodes to is projected from it at encode. The returned
    /// [`PlannedSiteRef`] is the only way an instruction can name a site: opaque, minted
    /// here alone, carrying either the id or the plan's refusal, which the encoder later
    /// refuses through the Sites bound. A request takes no construction budget.
    pub fn request_site(
        &mut self,
        handle: &OccurrenceSiteHandle,
    ) -> Result<PlannedSiteRef, SitePlanStateError> {
        let draft = &mut *self.draft;
        if handle.draft() != draft.durable.identity() {
            return Err(SitePlanStateError::new(SitePlanState::WrongPlan));
        }
        // The rows the handle was bound against may have been discarded since; rebinding
        // the same triple against the live tables is what proves they were not.
        let demand = handle.demand();
        let stamp = draft.durable.next_stamp();
        let live = draft.graph().revalidate(&demand)?;
        Ok(draft.sites.request(draft.durable.identity(), live, stamp))
    }

    /// Append a function body, validating every operation site its code names first: an
    /// operand minted by another draft, or one whose site row or policy receipt was
    /// discarded, is refused and no row is appended.
    pub fn add_function(&mut self, def: FunctionDef) -> Result<FuncId, DraftStateError> {
        self.draft.validate_function(&def)?;
        self.draft.allocate_function(Some(def))
    }

    /// Reserve the next function identity without a body. Empty slots cannot encode.
    pub fn reserve_function(&mut self) -> Result<FuncId, DraftStateError> {
        self.draft.allocate_function(None)
    }

    /// Fill this draft's reserved function exactly once. The caller must supply an
    /// identity from this draft; `FuncId` carries no foreign/stale-draft provenance. An
    /// absent row is [`DraftStateError::ForeignDraft`] and a filled one
    /// [`DraftStateError::RowState`].
    pub fn fill_function(&mut self, id: FuncId, def: FunctionDef) -> Result<(), DraftStateError> {
        let row = usize::from(id.index());
        match self.draft.functions.get(row) {
            None => return Err(DraftStateError::ForeignDraft),
            Some(Some(_)) => return Err(DraftStateError::RowState),
            Some(None) => {}
        }
        self.draft.validate_function(&def)?;
        if row < self.journal.at.functions {
            self.journal.fills.reserve(1);
            self.journal.fills.push(FillInverse::Function { row });
        }
        self.draft.charge_function(&def);
        self.draft.functions[row] = Some(def);
        Ok(())
    }

    /// Bind the export identity `id` to function `func`. The compiler mints `id` with
    /// [`ExportId::of_local`] from the export's declaration path; each public function is
    /// one export, so `func` is unique across the table.
    pub fn add_export(&mut self, id: ExportId, func: FuncId) {
        self.draft.exports.push(ExportDef { id, func });
    }

    /// Bind the report name `name` to the storeless test function `func`. Test names are
    /// unique across the project (the compiler rejects a duplicate), so the encoder sorts
    /// entries by their final name-string index.
    pub fn add_test_entry(&mut self, name: StrId, func: FuncId) {
        self.draft.test_entries.push(TestEntryDef { name, func });
    }

    /// Mint one scalar durable value shape into the draft's one arena.
    pub fn value_scalar(&mut self, scalar: Scalar) -> Result<ValueShapeNodeId, DraftStateError> {
        self.draft.durable.value_shapes_mut().scalar(scalar)
    }

    /// Mint one dense composite durable value shape, its leaves named in declaration
    /// order, into the draft's one arena.
    ///
    /// An arity past [`crate::bounds::MAX_STRUCT_LEAVES`] is the carrier-domain refusal,
    /// a logical-domain decision rather than a policy kind. Leaf provenance and leaf-name
    /// presence stay the arena's own decisions. No refusal mutates the arena.
    pub fn value_struct(
        &mut self,
        leaves: Vec<ValueShapeLeaf>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        if leaves.len() > bounds::MAX_STRUCT_LEAVES {
            return Err(DraftStateError::CarrierDomain);
        }
        self.draft.durable.value_shapes_mut().struct_shape(leaves)
    }

    /// Mint one enum durable value shape into the draft's one arena (checked at the
    /// surface exactly like [`Self::value_struct`], over the variant and payload
    /// bounds).
    pub fn value_enum(
        &mut self,
        identity: LedgerIdBytes,
        members: Vec<(LedgerIdBytes, Vec<ValueShapeLeaf>)>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        if members.len() > bounds::MAX_VARIANTS {
            return Err(DraftStateError::CarrierDomain);
        }
        for (_, payload) in &members {
            if payload.len() > bounds::MAX_PAYLOAD_FIELDS {
                return Err(DraftStateError::CarrierDomain);
            }
        }
        self.draft
            .durable
            .value_shapes_mut()
            .enum_shape(identity, members)
    }

    /// The total admitted inverse, in reverse dependency order: dependents before the
    /// owner suffixes they reference, and prefix fills before the owners their
    /// definitions reference. Called only by the armed `Drop`, so it is allocation-free,
    /// assertion-free, indexing-free, and non-panicking during an existing unwind.
    fn rollback_armed(&mut self) {
        let at = &self.journal.at;
        let draft = &mut *self.draft;
        draft.test_entries.truncate(at.test_entries);
        draft.exports.truncate(at.exports);
        draft.functions.truncate(at.functions);
        draft.function_payload_charge = at.function_payload_charge;
        while let Some(fill) = self.journal.fills.pop() {
            match fill {
                FillInverse::Function { row } => {
                    if let Some(slot) = draft.functions.get_mut(row) {
                        *slot = None;
                    }
                }
                FillInverse::Record { row, fields } => {
                    if let Some(slot) = draft.types.get_mut(row) {
                        slot.fields = fields;
                    }
                    if let Some(state) = draft.types_fill.get_mut(row) {
                        *state = FillState::Unfilled;
                    }
                }
                FillInverse::Enum { row, variants } => {
                    if let Some(slot) = draft.enums.get_mut(row) {
                        slot.variants = variants;
                    }
                    if let Some(state) = draft.enums_fill.get_mut(row) {
                        *state = FillState::Unfilled;
                    }
                }
            }
        }
        draft.sites.pop_suffix_to(at.sites, at.receipt);
        draft.durable.rewind_total(&at.durable);
        draft.product_conflict = at.product_conflict;
        draft.application_conflict = at.application_conflict;
        // Each interned owner's index key is removed while the popped row is still live.
        draft.colls.truncate(at.colls);
        draft.enums.truncate(at.enums);
        draft.enums_fill.truncate(at.enums);
        draft.types.truncate(at.types);
        draft.types_fill.truncate(at.types);
        while draft.consts.len() > at.consts {
            if let Some(value) = draft.consts.last() {
                draft.const_index.remove(value);
            }
            draft.consts.pop();
        }
        while draft.strings.len() > at.strings {
            if let Some(text) = draft.strings.last() {
                draft.string_index.remove(text);
            }
            draft.strings.pop();
        }
    }
}

impl Drop for DraftTxn<'_> {
    /// The armed inverse. A committed guard was disarmed and restores nothing.
    fn drop(&mut self) {
        if self.armed {
            self.rollback_armed();
        }
    }
}

impl Default for ImageDraft {
    /// A fresh draft with a fresh identity. Deliberately not derived: a derived `Default`
    /// would hand every default-constructed draft the same identity.
    fn default() -> Self {
        Self::new()
    }
}

impl ImageDraft {
    pub fn new() -> Self {
        Self {
            durable: DurableContractGraph::new(),
            strings: Vec::new(),
            string_index: HashMap::new(),
            consts: Vec::new(),
            const_index: HashMap::new(),
            types: Vec::new(),
            types_fill: Vec::new(),
            enums: Vec::new(),
            enums_fill: Vec::new(),
            colls: Vec::new(),
            product_conflict: None,
            application_conflict: None,
            sites: SiteDemandPlan::default(),
            functions: Vec::new(),
            function_payload_charge: 0,
            exports: Vec::new(),
            test_entries: Vec::new(),
        }
    }

    /// The live tables a site binding is validated and projected against.
    fn graph(&self) -> OccurrenceGraph<'_> {
        self.durable.occurrence_graph()
    }

    /// Derive what interning `text` would append, without touching an owner.
    ///
    /// The read-only half of a string mint: every way the mint can fail lives here, so the
    /// matching commit cannot stop partway and a compound operation can prepare both of
    /// its rows before either lands.
    fn prepare_string(&self, text: &str) -> Result<PreparedString, DraftStateError> {
        if let Some(&id) = self.string_index.get(text) {
            return Ok(PreparedString { id, fresh: None });
        }
        let id = StrId(wide_ordinal(self.strings.len())?);
        Ok(PreparedString {
            id,
            fresh: Some(FreshString {
                text: text.to_string(),
            }),
        })
    }

    /// Apply a prepared string mint. Infallible by construction.
    fn commit_string(&mut self, prepared: PreparedString) -> StrId {
        let PreparedString { id, fresh } = prepared;
        let Some(FreshString { text }) = fresh else {
            return id;
        };
        self.string_index.insert(text.clone(), id);
        self.strings.push(text);
        id
    }

    fn intern_const(&mut self, value: ConstValue) -> Result<ConstId, DraftStateError> {
        let prepared = self.prepare_const(value)?;
        Ok(self.commit_const(prepared))
    }

    /// The read-only half of a constant mint (see [`Self::prepare_string`]).
    fn prepare_const(&self, value: ConstValue) -> Result<PreparedConst, DraftStateError> {
        if let Some(&id) = self.const_index.get(&value) {
            return Ok(PreparedConst { id, fresh: None });
        }
        let id = ConstId(wide_ordinal(self.consts.len())?);
        Ok(PreparedConst {
            id,
            fresh: Some(FreshConst { value }),
        })
    }

    /// Apply a prepared constant mint. Infallible by construction.
    fn commit_const(&mut self, prepared: PreparedConst) -> ConstId {
        let PreparedConst { id, fresh } = prepared;
        let Some(FreshConst { value }) = fresh else {
            return id;
        };
        self.consts.push(value);
        self.const_index.insert(value, id);
        id
    }

    fn append_record_type(
        &mut self,
        def: RecordTypeDef,
        fill: FillState,
    ) -> Result<TypeId, DraftStateError> {
        let id = TypeId(wide_ordinal(self.types.len())?);
        self.types.push(def);
        self.types_fill.push(fill);
        Ok(id)
    }

    fn append_enum_type(
        &mut self,
        def: EnumTypeDef,
        fill: FillState,
    ) -> Result<EnumId, DraftStateError> {
        let id = EnumId(wide_ordinal(self.enums.len())?);
        self.enums.push(def);
        self.enums_fill.push(fill);
        Ok(id)
    }

    /// The number of collection types already appended to this draft.
    pub fn collection_type_count(&self) -> usize {
        self.colls.len()
    }

    /// The direct members of the Product declaration `product` names, in declaration
    /// order — each member's canonical path selector and its declared shape — or `None`
    /// if this draft holds no such declaration.
    ///
    /// A member's own members are read the same way, through [`Self::members_of`], so a
    /// walk of a declaration materializes one level at a time. A member's canonical path
    /// is published by the one owner of the declaration rows, never recomputed by
    /// comparing paths.
    ///
    /// Reading takes no construction budget: it appends nothing.
    pub fn product_members(&self, product: LedgerIdBytes) -> Option<Vec<DeclarationMember>> {
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
    pub fn members_of(
        &self,
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
    /// occurrence's Product or its own root-scoped case, and that the supplied target is
    /// the one that node admits. No later call accepts a second target, and the returned
    /// handle borrows nothing: the immutable borrow ends here, before
    /// [`Self::request_site`] takes the draft mutably.
    pub fn bind_occurrence_site(
        &self,
        root: &RootOccurrenceSelector,
        path: &CanonicalDeclarationPathSelector,
        target: SemanticTarget,
    ) -> Result<OccurrenceSiteHandle, SitePlanStateError> {
        let demand = self.graph().validate(root, path, target)?;
        Ok(OccurrenceSiteHandle::new(self.durable.identity(), demand))
    }

    fn validate_function(&self, def: &FunctionDef) -> Result<(), DraftStateError> {
        for instr in &def.code {
            if let Some(site) = instr.site() {
                self.validate_site_ref(site)?;
            }
        }
        Ok(())
    }

    /// The sole function ordinal mint. A defined row has already passed operand
    /// validation, so a failed append leaves no reservation behind.
    fn allocate_function(&mut self, def: Option<FunctionDef>) -> Result<FuncId, DraftStateError> {
        let id = FuncId(function_ordinal(self.functions.len())?);
        if let Some(def) = &def {
            self.charge_function(def);
        }
        self.functions.push(def);
        Ok(id)
    }

    fn charge_function(&mut self, def: &FunctionDef) {
        self.function_payload_charge = self
            .function_payload_charge
            .saturating_add(function_payload_floor(def))
            .min(DECISIVE_FUNCTION_PAYLOAD);
    }

    /// Whether the retained function bodies alone already exceed
    /// [`bounds::MAX_IMAGE_BYTES`], so no completion of this draft can encode. A producer
    /// polls this after each settled body to stop retaining work the image cannot carry;
    /// the encoder's measurement remains the verdict on a draft that passes it.
    pub fn function_payload_exceeds_image_limit(&self) -> bool {
        self.function_payload_charge > bounds::MAX_IMAGE_BYTES
    }

    /// The number of record types (image `TypeId` ceiling) currently reserved.
    pub fn record_type_count(&self) -> usize {
        self.types.len()
    }

    /// The number of enum types (image `EnumId` ceiling) currently reserved.
    pub fn enum_type_count(&self) -> usize {
        self.enums.len()
    }

    /// The draft's one durable value-shape arena, for reading a minted shape's depth,
    /// kind, and references.
    pub fn value_shapes(&self) -> &CanonicalValueShapeDag {
        self.durable.value_shapes()
    }

    /// This draft's durable contract graph, borrowed in place.
    ///
    /// The graph is not a fifth owner: it is the view spine over the four this draft
    /// already holds — the application identity, the Product declaration table, the flat
    /// root-occurrence table, and the value-shape DAG. Nothing is copied, so the contract
    /// identity is computed over exactly the rows the DURABLE section is written from.
    pub fn contract_view(&self) -> DurableContractView<'_> {
        self.durable.contract_view()
    }

    /// Arm the one mutation surface over this draft. The guard's journal restores the
    /// draft to exactly this state on rollback or unwind; [`DraftTxn::commit`] retains
    /// everything.
    ///
    /// The guard borrows its draft exclusively for its whole lifetime, so a second
    /// admission cannot open while one is live and a guard cannot outlive its draft.
    ///
    /// ```compile_fail,E0499
    /// let mut draft = marrow_image::ImageDraft::new();
    /// let first = draft.begin_transaction();
    /// let second = draft.begin_transaction();
    /// drop(first);
    /// drop(second);
    /// ```
    /// ```compile_fail,E0597
    /// let txn = {
    ///     let mut draft = marrow_image::ImageDraft::new();
    ///     draft.begin_transaction()
    /// };
    /// drop(txn);
    /// ```
    pub fn begin_transaction(&mut self) -> DraftTxn<'_> {
        DraftTxn {
            journal: DraftJournal {
                at: self.snapshot(),
                fills: Vec::new(),
            },
            draft: self,
            armed: true,
        }
    }

    /// The structural image the journal restores to. The draft is destructured
    /// exhaustively, so a new owner stops this compiling until it is recorded here or
    /// deliberately excluded.
    fn snapshot(&self) -> DraftSnapshot {
        let Self {
            durable,
            strings,
            string_index: _,
            consts,
            const_index: _,
            types,
            types_fill: _,
            enums,
            enums_fill: _,
            colls,
            product_conflict,
            application_conflict,
            sites,
            functions,
            function_payload_charge,
            exports,
            test_entries,
        } = self;
        DraftSnapshot {
            strings: strings.len(),
            consts: consts.len(),
            types: types.len(),
            enums: enums.len(),
            colls: colls.len(),
            durable: durable.checkpoint(),
            sites: sites.rows().len(),
            functions: functions.len(),
            function_payload_charge: *function_payload_charge,
            exports: exports.len(),
            test_entries: test_entries.len(),
            product_conflict: *product_conflict,
            application_conflict: *application_conflict,
            receipt: sites.receipt(),
        }
    }

    /// The sticky application-identity divergence, if one was latched.
    pub(crate) fn application_conflict(&self) -> Option<ApplicationIdentityConflict> {
        self.application_conflict
    }

    pub(crate) fn strings(&self) -> &[String] {
        &self.strings
    }
    pub(crate) fn consts(&self) -> &[ConstValue] {
        &self.consts
    }
    pub(crate) fn types(&self) -> &[RecordTypeDef] {
        &self.types
    }
    /// Per-record fill state, in lockstep with `types`, for the fence's vacancy check.
    pub(crate) fn types_fill(&self) -> &[FillState] {
        &self.types_fill
    }
    /// Per-enum fill state, in lockstep with `enums`, for the fence's vacancy check.
    pub(crate) fn enums_fill(&self) -> &[FillState] {
        &self.enums_fill
    }
    pub(crate) fn enums(&self) -> &[EnumTypeDef] {
        &self.enums
    }
    pub(crate) fn collections(&self) -> &[CollectionTypeDef] {
        &self.colls
    }
    /// The flat root-occurrence rows, in declaration order.
    pub(crate) fn root_occurrences(&self) -> &[RootOccurrence] {
        self.durable.occurrences().rows()
    }

    /// The Product declaration one occurrence row projects.
    pub(crate) fn declaration_of(&self, occurrence: &RootOccurrence) -> &ProductDeclaration {
        self.durable
            .products()
            .declaration(occurrence.declaration())
    }

    /// The admitted Product declarations, each retained once however many roots occur
    /// over it.
    pub(crate) fn product_declarations(&self) -> &[ProductDeclaration] {
        self.durable.products().declarations()
    }

    /// The first divergent repeat of an already-declared Product, if one was appended.
    pub(crate) fn product_conflict(&self) -> Option<ProductClaimConflict> {
        self.product_conflict
    }
    pub(crate) fn application_identity(&self) -> Option<LedgerIdBytes> {
        self.durable.application()
    }
    /// Write the site-table rows into `sink`: per retained row, the semantic path of the
    /// node the demand addresses — `u8(step_count) ‖ [u8(ledger_kind) ‖ 16 id bytes]*`,
    /// the same frozen ledger `IDREF` kinds a durable node's identity uses — then the
    /// one-byte operation target. The one site-row codec, driven by the measure core's
    /// counting run and by emission alike.
    ///
    /// Each row's steps are streamed twice through
    /// [`crate::product::OccurrenceGraph::project_steps`], once for the count-first prefix
    /// and once for the steps, so no path is materialized and no row retains anything.
    /// That projection is the site table's only path source, so a site's path is derived
    /// from the same rows the DURABLE member graph is written from and cannot disagree
    /// with them.
    ///
    /// A chain is at most `2 + MAX_DURABLE_DEPTH = MAX_SITE_PATH_STEPS` steps, which
    /// `bounds` const-asserts against one byte; the checked conversion keeps totality tied
    /// to that bound rather than to a silent cast.
    pub(crate) fn write_site_rows(
        &self,
        sink: &mut impl ImageByteSink,
    ) -> Result<(), ImageBuildError> {
        if self.sites.rows().is_empty() {
            return Ok(());
        }
        let application = self
            .application_identity()
            .ok_or(ImageBuildError::InvalidReference(
                ReferenceKind::ApplicationIdentity,
            ))?;
        let graph = self.graph();
        for row in self.sites.rows() {
            // A count already past the ceiling is decided; further rows only grow it.
            if sink.is_full() {
                return Ok(());
            }
            let mut steps = 0usize;
            graph
                .project_steps(application, row.key(), |_| steps += 1)
                .ok_or(ImageBuildError::InvalidReference(
                    ReferenceKind::OperationSite,
                ))?;
            let step_count = u8::try_from(steps)
                .expect("a bounded semantic path's step count fits the site-path width");
            sink.push(step_count);
            graph
                .project_steps(application, row.key(), |step| {
                    sink.push(step.kind.ledger_kind());
                    sink.extend_bytes(step.id.bytes());
                })
                .ok_or(ImageBuildError::InvalidReference(
                    ReferenceKind::OperationSite,
                ))?;
            sink.push(match row.key().target() {
                SemanticTarget::WholePayload => 0x00,
                SemanticTarget::FieldLeaf => 0x01,
                SemanticTarget::IndexScan => 0x02,
                SemanticTarget::IndexLookup => 0x03,
                SemanticTarget::GroupEntry => 0x04,
            });
        }
        Ok(())
    }

    /// Prove that every retained site row still projects: the coherence walk's
    /// validation-only twin of [`Self::write_site_rows`], retaining nothing.
    pub(crate) fn validate_site_projection(&self) -> Result<(), ImageBuildError> {
        if self.sites.rows().is_empty() {
            return Ok(());
        }
        let application = self
            .application_identity()
            .ok_or(ImageBuildError::InvalidReference(
                ReferenceKind::ApplicationIdentity,
            ))?;
        let graph = self.graph();
        for row in self.sites.rows() {
            graph.project_steps(application, row.key(), |_| {}).ok_or(
                ImageBuildError::InvalidReference(ReferenceKind::OperationSite),
            )?;
        }
        Ok(())
    }

    /// The number of site rows [`ImageDraft::write_site_rows`] will emit. The site table is
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

    /// The one validator for spending a site ref: the plan's provenance check — the
    /// minting draft and the exact site row or receipt the ref stands on — plus the live
    /// graph's recheck of the occurrence and path row identities inside the ref's bound
    /// demand. The graph half is what makes a rolled-back ref detectable when its rows
    /// re-mint at the same ordinals with fresh stamps while a preexisting receipt stays
    /// live. An over-policy ref with intact provenance is valid here: that crossing is
    /// the Sites policy candidate's to report, never a coherence fault.
    fn validate_site_ref(&self, site: &PlannedSiteRef) -> Result<(), SitePlanStateError> {
        self.sites
            .validate(self.durable.identity(), site)
            .map_err(SitePlanStateError::new)?;
        self.graph().revalidate(&site.demand())?;
        Ok(())
    }

    /// Whether `site` was minted by this draft's plan and every row it stands on is
    /// still live: the coherence walk's spelling of [`Self::validate_site_ref`].
    pub(crate) fn site_ref_is_live(&self, site: &PlannedSiteRef) -> bool {
        self.validate_site_ref(site).is_ok()
    }

    /// The exact wire ordinal of one validated fitting site ref. Reached only through the
    /// measured wire plan's site projection, so no numeric site id exists before capped
    /// measurement finds the draft policy-clean.
    pub(crate) fn site_wire_ordinal(&self, site: &PlannedSiteRef) -> Result<u16, ImageBuildError> {
        if self.graph().revalidate(&site.demand()).is_err() {
            return Err(ImageBuildError::InvalidReference(
                ReferenceKind::OperationSite,
            ));
        }
        self.sites.wire_ordinal(self.durable.identity(), site)
    }
    /// Borrow the instruction sequence at a function's insertion ordinal.
    ///
    /// A `FuncId` carries no draft provenance or verification claim. The caller
    /// must use an identity returned by this draft and handle an absent ordinal.
    pub fn function_code(&self, function: FuncId) -> Option<&[Instr]> {
        self.functions
            .get(usize::from(function.index()))
            .and_then(Option::as_ref)
            .map(|function| function.code.as_slice())
    }

    /// The reserved function domain, including slots whose bodies are unavailable.
    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    pub(crate) fn functions(&self) -> &[Option<FunctionDef>] {
        &self.functions
    }

    /// The export rows, borrowed in insertion order: the retained base row set the
    /// encoder's canonical permutation maps.
    pub(crate) fn export_rows(&self) -> &[ExportDef] {
        &self.exports
    }

    /// The number of export rows, without materializing them.
    pub(crate) fn export_count(&self) -> usize {
        self.exports.len()
    }

    /// The test-entry rows, borrowed in insertion order: the retained base row set the
    /// encoder's canonical permutation maps.
    pub(crate) fn test_entry_rows(&self) -> &[TestEntryDef] {
        &self.test_entries
    }

    /// The number of test-entry rows, without materializing them.
    pub(crate) fn test_entry_count(&self) -> usize {
        self.test_entries.len()
    }

    /// The canonical test-entry permutation: base-row indices ascending by remapped name
    /// index, computed by the table's one comparator.
    ///
    /// The raw map reads are deliberately outside the token seal: they are the
    /// comparator's keys, resolved once per base row in row order, never a section writer
    /// resolving a reference it could branch on.
    pub(crate) fn test_entry_permutation(&self, str_map: &[u16]) -> Vec<usize> {
        let keys: Vec<u16> = self
            .test_entries
            .iter()
            .map(|entry| str_map[entry.name.raw() as usize])
            .collect();
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by_key(|&row| keys[row]);
        order
    }
}

impl StrId {
    /// The string-pool id at `index`.
    ///
    /// A logical string id is a pool position, not a capability: the independent verifier
    /// reads one from received bytes, and every owner that resolves one checks it against
    /// the pool it indexes.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical ordinal. Never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }

    pub(crate) fn raw(self) -> u32 {
        self.0
    }
}

impl ConstValue {
    /// A sort key `(tag, payload-bytes)` where the Text payload is the *final* string
    /// index resolved through `str_map`.
    ///
    /// This raw map read is deliberately outside the token seal: it is the canonical-order
    /// *comparator* itself, fed by the checked base rows, not a section writer resolving a
    /// reference it could branch on.
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
        let mut txn = draft.begin_transaction();

        let list = txn
            .add_collection_type(CollectionTypeDef::List {
                elem: ImageType::scalar(Scalar::Int),
            })
            .expect("a within-domain mint");
        assert_eq!(list.index(), 0);
        assert_eq!(txn.collection_type_count(), 1);

        let map = txn
            .add_collection_type(CollectionTypeDef::Map {
                key: ImageType::scalar(Scalar::Text),
                value: ImageType::scalar(Scalar::Bool),
            })
            .expect("a within-domain mint");
        assert_eq!(map.index(), 1);
        assert_eq!(txn.collection_type_count(), 2);
    }
}

#[cfg(test)]
mod site_binding_tests {
    use super::{
        AdmittedGraphInputPlan, AdmittedRoot, DraftTxn, ImageDraft, RootOccurrenceDef, TypeId,
    };
    use crate::durable_id::{DurableIndexComponent, DurableIndexShape, LedgerIdBytes};
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape};
    use crate::semantic::{SemanticStepKind, SemanticTarget};
    use crate::site_plan::{SitePlanState, SitePlanStateError};
    use crate::ty::Scalar;

    /// One Product, two root occurrences (the two-draft cases build one each), and the
    /// image's own command ceiling.
    fn plan() -> AdmittedGraphInputPlan {
        AdmittedGraphInputPlan::admit(1, 2, crate::bounds::MAX_ADMITTED_DECLARATION_COMMANDS)
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

    /// Declare one Product of one required int field and admit one keyless root over it,
    /// carrying `indexes`.
    fn declare_one_root(txn: &mut DraftTxn<'_>, indexes: Vec<DurableIndexShape>) -> AdmittedRoot {
        txn.set_application_identity(LedgerIdBytes::from_bytes([0x01; 16]));
        let name = txn.intern_string("r").expect("a within-domain mint");
        let value = txn.value_scalar(Scalar::Int).expect("the test arena mints");
        txn.declare_product(
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
        txn.add_root_occurrence(
            &plan(),
            product(),
            RootOccurrenceDef {
                name,
                keys: Vec::new(),
                placement: placement(),
                indexes: indexes.into(),
            },
        )
        .expect("the Product is declared")
    }

    /// A committed draft holding one Product of one required int field and one keyless
    /// root over it.
    fn one_root() -> (ImageDraft, AdmittedRoot) {
        let mut draft = ImageDraft::new();
        let mut txn = draft.begin_transaction();
        let admitted = declare_one_root(&mut txn, Vec::new());
        txn.commit();
        (draft, admitted)
    }

    /// The three private refusal cases, each reached through the public binder. The public
    /// type is one opaque invariant, so the discriminant is only ever observed here.
    #[test]
    fn the_binder_distinguishes_its_three_refusals() {
        let (first, first_root) = one_root();
        let (second, _) = one_root();

        let mine = first.product_members(product()).expect("declared");
        let theirs = second.product_members(product()).expect("declared");

        // A path selector published by another draft is a wrong-plan refusal, however
        // identical the two graphs look.
        assert_eq!(
            first
                .bind_occurrence_site(
                    first_root.occurrence(),
                    theirs[0].path(),
                    SemanticTarget::FieldLeaf
                )
                .expect_err("a foreign selector cannot bind"),
            SitePlanStateError::new(SitePlanState::WrongPlan),
        );

        // A target the node does not admit is an invalid demand: a field admits only a
        // field-leaf read or write, and a root placement only a whole payload.
        assert_eq!(
            first
                .bind_occurrence_site(
                    first_root.occurrence(),
                    mine[0].path(),
                    SemanticTarget::WholePayload
                )
                .expect_err("a field admits no whole-payload site"),
            SitePlanStateError::new(SitePlanState::InvalidDemand),
        );
        assert_eq!(
            first
                .bind_occurrence_site(
                    first_root.occurrence(),
                    first_root.placement_path(),
                    SemanticTarget::FieldLeaf
                )
                .expect_err("a placement admits no field-leaf site"),
            SitePlanStateError::new(SitePlanState::InvalidDemand),
        );

        // The one target each admits does bind, so the refusals above are about the
        // target and not about the pair being unbindable.
        assert!(
            first
                .bind_occurrence_site(
                    first_root.occurrence(),
                    mine[0].path(),
                    SemanticTarget::FieldLeaf
                )
                .is_ok()
        );
    }

    /// Discarding the rows a handle was bound against makes it stale, and the ordinal
    /// being reused afterwards does not revive it.
    #[test]
    fn a_handle_over_a_discarded_row_is_stale_even_when_its_ordinal_is_reused() {
        let mut draft = ImageDraft::new();
        // Build the rows inside an armed transaction, so dropping it discards them.
        let handle = {
            let mut proof = draft.begin_transaction();
            let admitted = declare_one_root(&mut proof, Vec::new());
            proof
                .bind_occurrence_site(
                    admitted.occurrence(),
                    admitted.placement_path(),
                    SemanticTarget::WholePayload,
                )
                .expect("the root admits a whole-payload site")
        };

        let mut retry = draft.begin_transaction();
        assert_eq!(
            retry
                .request_site(&handle)
                .expect_err("the occurrence row was discarded"),
            SitePlanStateError::new(SitePlanState::StaleBinding),
        );

        // The same ordinals are re-minted deterministically; the handle must still not
        // authenticate the replacement.
        declare_one_root(&mut retry, Vec::new());
        assert_eq!(
            retry
                .request_site(&handle)
                .expect_err("a re-minted row is not the row the handle was bound against"),
            SitePlanStateError::new(SitePlanState::StaleBinding),
        );
    }

    /// The one streaming projection grammar spells every demand kind outermost-first —
    /// the application step, the placement step, then the index or member chain — for
    /// the root placement, a managed index, and a declaration member alike.
    #[test]
    fn the_streamed_projection_spells_every_demand_kind() {
        let mut draft = ImageDraft::new();
        let mut txn = draft.begin_transaction();
        let admitted = declare_one_root(
            &mut txn,
            vec![DurableIndexShape {
                id: LedgerIdBytes::from_bytes([0x44; 16]),
                unique: false,
                components: vec![DurableIndexComponent::Field(field())],
            }],
        );
        txn.commit();
        let members = draft.product_members(product()).expect("declared");

        let cases = [
            (
                admitted.placement_path(),
                SemanticTarget::WholePayload,
                vec![SemanticStepKind::Application, SemanticStepKind::Placement],
            ),
            (
                &admitted.index_paths()[0],
                SemanticTarget::IndexScan,
                vec![
                    SemanticStepKind::Application,
                    SemanticStepKind::Placement,
                    SemanticStepKind::Index,
                ],
            ),
            (
                members[0].path(),
                SemanticTarget::FieldLeaf,
                vec![
                    SemanticStepKind::Application,
                    SemanticStepKind::Placement,
                    SemanticStepKind::Field,
                ],
            ),
        ];
        for (selector, target, expected) in cases {
            let handle = draft
                .bind_occurrence_site(admitted.occurrence(), selector, target)
                .expect("the node admits its one target");
            let key = handle.demand().key();
            let application = draft.application_identity().expect("anchored");
            let graph = draft.graph();
            let mut kinds = Vec::new();
            graph
                .project_steps(application, key, |step| kinds.push(step.kind))
                .expect("a bound demand projects");
            assert_eq!(kinds, expected);
        }
    }

    /// A command vector that does not state a forest is refused before any row is
    /// appended, so a malformed declaration cannot reach the encoder at all.
    #[test]
    fn a_malformed_command_vector_appends_no_row() {
        let mut draft = ImageDraft::new();
        let mut txn = draft.begin_transaction();

        let refusal = txn
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
        assert!(txn.product_members(product()).is_none());
        assert!(txn.root_occurrences().is_empty());
    }
}

#[cfg(test)]
mod row_access_tests {
    use super::{FunctionDef, ImageDraft, StrId};
    use crate::export_id::ExportId;
    use crate::instr::Instr;
    use crate::ty::ImageType;

    fn unit_function(name: StrId, source: StrId) -> FunctionDef {
        FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code: vec![Instr::Return],
            spans: Vec::new(),
        }
    }

    #[test]
    fn function_code_borrows_the_appended_allocation_and_tracks_rollback() {
        let mut draft = ImageDraft::new();
        let mut txn = draft.begin_transaction();
        let name = txn.intern_string("body").expect("name fits");
        let source = txn.intern_string("source").expect("source fits");
        let def = unit_function(name, source);
        let allocation = def.code.as_ptr();
        let func = txn.add_function(def).expect("no operation sites");
        let borrowed = txn.function_code(func).expect("append is visible");
        assert_eq!(borrowed.as_ptr(), allocation);
        assert!(matches!(borrowed, [Instr::Return]));
        txn.rollback();
        assert!(draft.function_code(func).is_none());
    }

    /// The borrowed row slices and their counts mirror exactly what was added, in
    /// insertion order — the retained base row set the encoder's permutations map.
    #[test]
    fn the_borrowed_export_and_test_rows_mirror_what_was_added() {
        let mut draft = ImageDraft::new();
        let mut txn = draft.begin_transaction();
        let source = txn.intern_string("s").expect("a within-domain mint");
        let alpha = txn.intern_string("alpha").expect("a within-domain mint");
        let zeta = txn.intern_string("zeta").expect("a within-domain mint");
        let mut funcs = Vec::new();
        for name in [zeta, alpha] {
            funcs.push(
                txn.add_function(unit_function(name, source))
                    .expect("no site operand needs validating"),
            );
        }
        let first = ExportId::of_local("m", "zeta");
        let second = ExportId::of_local("m", "alpha");
        txn.add_export(first, funcs[0]);
        txn.add_export(second, funcs[1]);
        txn.add_test_entry(zeta, funcs[0]);
        txn.commit();

        assert_eq!(draft.export_count(), 2);
        assert_eq!(draft.test_entry_count(), 1);

        let exports = draft.export_rows();
        assert_eq!(exports.len(), 2);
        assert_eq!(exports[0].id(), &first);
        assert_eq!(exports[0].func(), funcs[0].index());
        assert_eq!(exports[1].id(), &second);
        assert_eq!(exports[1].func(), funcs[1].index());

        let tests = draft.test_entry_rows();
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name(), zeta);
        assert_eq!(tests[0].func(), funcs[0].index());
    }

    /// The canonical test-entry permutation orders base-row indices by remapped name,
    /// stably, without touching the rows themselves.
    #[test]
    fn the_test_entry_permutation_orders_rows_by_remapped_name() {
        let mut draft = ImageDraft::new();
        let mut txn = draft.begin_transaction();
        let source = txn.intern_string("s").expect("a within-domain mint");
        let zeta = txn.intern_string("zeta").expect("a within-domain mint");
        let alpha = txn.intern_string("alpha").expect("a within-domain mint");
        for name in [zeta, alpha] {
            let func = txn
                .add_function(unit_function(name, source))
                .expect("no site operand needs validating");
            txn.add_test_entry(name, func);
        }
        txn.commit();
        // The pool interned [s, zeta, alpha]; byte-sorted it is [alpha, s, zeta], so
        // the remap is s→1, zeta→2, alpha→0 and the entries [zeta, alpha] come back
        // as [alpha, zeta].
        let str_map = vec![1u16, 2, 0];
        assert_eq!(draft.test_entry_permutation(&str_map), vec![1, 0]);
    }
}

#[cfg(test)]
#[path = "function_payload_charge_tests.rs"]
mod function_payload_charge_tests;
