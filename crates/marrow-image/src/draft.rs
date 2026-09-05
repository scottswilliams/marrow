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
//! Those unconditional owners mint a logical id as the table's current length carried in
//! the wide `u32` ordinal every owned pre-seal id newtype holds, so no mint is ever a
//! narrowed table length and an over-policy table still mints the N+1 id (the
//! nonblocking provisional-commit law). The id itself never carries a wire width: the
//! only narrowing to the wire's `u16` spelling is the measure core's policy-clean
//! checked path (`crate::measure::wire_ordinal`/`wire_len`), which runs strictly after
//! the policy walk has refused any draft past its §E bound (`MAX_STRINGS`,
//! `MAX_CONSTS`, `MAX_TYPES`, `MAX_ENUMS`, `MAX_COLLECTIONS`, `MAX_FUNCTIONS`) — every
//! one at or below `u16::MAX` by the `const _` encoded-width block in
//! [`crate::bounds`].

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
use crate::policy_ledger::{CurrentValidationOccurrence, TablePolicyKind, TablePolicyLedger};
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
use crate::value_dag::{CanonicalValueShapeDag, ImageByteSink, ValueShapeNodeId};

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

    /// Admit a construction budget of `products` Product declarations, `roots` root
    /// occurrences, and `commands` member commands in any one declaration.
    ///
    /// **Exactly what this enforces.** Each term is checked against the public ceiling of
    /// the same name, and nothing else: a term beyond what may be handed to the durable
    /// graph at all is not a budget, and admitting one would leave the unbounded,
    /// uncounted command stream this type exists to close. Each ceiling follows the
    /// admitted-intake rule stated on [`bounds::MAX_ADMITTED_ROOT_OCCURRENCES`] — one past
    /// the bound whose refusal owner keeps its refusal — so an over-wide declaration still
    /// reaches [`ImageBuildError::TooManyDurableMembers`] and an over-count graph still
    /// reaches [`ImageBuildError::TooManyRoots`] over a complete graph. It does **not**
    /// check a caller's numbers against a census, an identity ledger, or any other owner's
    /// state: no such input reaches here.
    ///
    /// What makes an admitted budget bind is the other half, spent where it is presented:
    /// the construction entry points check each arrival *cumulatively* against the graph
    /// already receiving it — declarations against the rows the table holds, occurrences
    /// against the rows the occurrence table holds, one command vector against the admitted
    /// width — and there is no third route into those tables. A plan is therefore a
    /// count-frozen budget, publicly minted and publicly bounded, rather than a claim about
    /// what its holder counted.
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

    /// The budget for a census whose counts may exceed what any image could carry: each
    /// term is admitted at its own ceiling instead of refusing the whole census.
    ///
    /// It states the same counts [`Self::admit`] would and grants nothing more for any
    /// number either accepts. It exists because an admission owner that reports source
    /// problems as typed diagnostics has no abort to take when its census overruns, and
    /// the overrun already has an owner — the encoder, reporting it over a *complete*
    /// graph. Saturating is what leaves that graph complete; refusing would truncate the
    /// graph the overrun is reported over.
    ///
    /// Every budget still comes from stated counts. There is deliberately no ceiling
    /// constant to name instead of counting.
    pub fn admit_saturating(products: usize, roots: usize, commands: usize) -> Self {
        Self {
            products: products.min(bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS),
            roots: roots.min(bounds::MAX_ADMITTED_ROOT_OCCURRENCES),
            commands: commands.min(bounds::MAX_ADMITTED_DECLARATION_COMMANDS),
        }
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

/// Mint the next logical ordinal for an owned pre-seal table of `len` rows.
///
/// The wide-ordinal issuance check: the carrier domain is the `u32` the id newtypes
/// hold, not a public policy maximum — an over-policy table still mints the N+1 id
/// (the nonblocking provisional-commit law) and the image is refused at the encode
/// fence by the policy walk. A caller at the `u32` boundary receives the closed
/// carrier-domain refusal before any owner mutates; the production compiler's
/// envelope proof makes that arm unreachable and maps it to a compiler invariant.
pub(crate) fn wide_ordinal(len: usize) -> Result<u32, DraftStateError> {
    u32::try_from(len).map_err(|_| DraftStateError::CarrierDomain)
}

/// The function-slot ordinal, checked at its mint.
///
/// Function width is `IMGFUNC01`'s to widen and stays `u16` here, but *unchecked* is a
/// different property from *narrow*: a draft carrying more functions than the carrier
/// spells would wrap and alias slot zero, and an arbitrary external caller on this
/// `#[doc(hidden)] pub` surface can reach that. The mint therefore returns the same
/// closed builder-domain refusal every other id-minting mutator returns, rather than
/// leaving the bound to the encoder to notice afterwards.
fn function_ordinal(len: usize) -> Result<u16, DraftStateError> {
    u16::try_from(len).map_err(|_| DraftStateError::CarrierDomain)
}

/// A logical string-pool id, stable across the sort the encoder performs.
///
/// Like every owned pre-seal logical id it is a typed newtype over a private wide
/// `u32` ordinal: the id itself never carries a wire width, and the only narrowing
/// to the wire's `u16` spelling is the measure core's policy-clean checked path
/// (`crate::measure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(u32);

/// A logical constant-pool id, stable across the sort the encoder performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstId(pub(crate) u32);

impl ConstId {
    /// The constant-pool id at `index`, widened from a `u16` ordinal. Its only
    /// cross-crate consumers are the frozen legacy accepted-bytes pins
    /// (`marrow-verify/tests/legacy_ok_pins.rs`), which spell known-answer operands
    /// against a draft whose pool they built; production ids come from the draft's
    /// own checked interning mints.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical ordinal, as carried in a `ConstLoad` operand until the encoder
    /// rewrites it to the final sorted pool position. A logical ordinal, never a wire
    /// value: emission narrows only through the measure core's policy-clean path.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A record-type index (also the final container index; types keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub(crate) u32);

impl TypeId {
    /// The record-type index at `index`.
    ///
    /// A record-type index is a container-table position, not a capability, for the same
    /// reason [`StrId::from_index`] is: the independent verifier reads one from received
    /// bytes and every owner that resolves one range-checks it.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical record-type ordinal, as carried in a `RecordNew` operand and
    /// in an `ImageType::Record`. A logical ordinal, never a wire value.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// An enum-type index (also the final container index; enums keep insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub(crate) u32);

impl EnumId {
    /// The enum-type index at `index` — the widening of a received `u16` wire read.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical enum-type ordinal, as carried in `EnumConstruct` operands and
    /// in an `ImageType::Enum`. A logical ordinal, never a wire value.
    pub const fn index(self) -> u32 {
        self.0
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
    /// operands and in an `ImageType::Collection`. A logical ordinal, never a wire
    /// value.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A durable root reference: the wide logical ordinal of one row in the flat
/// root-occurrence table — the reference an `ImageType::Identity` and a
/// `MakeIdentity` instruction embed, and the fact [`AdmittedRoot::root_id`]
/// publishes. The wire's `u16` RootId discriminant is the policy-clean narrowing of
/// this ordinal, performed only on the measure core's fitting arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RootId(pub(crate) u32);

impl RootId {
    /// The root reference at `index` — the widening of a received `u16` wire read.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical occurrence ordinal. A logical ordinal, never a wire value.
    pub const fn index(self) -> u32 {
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
/// [`PlannedSiteRef`] the plan mints, never by a number a caller can write.
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
    /// whose policy-clean `u16` narrowing is the discriminant an entry identity
    /// `Id(^root)` carries on the wire. It is a fact the compiler embeds into identity
    /// instructions, not a way to name the occurrence row: naming it is what the
    /// selector is for.
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
    /// A divergent application ledger identity was set after one was recorded: two
    /// applications wearing one draft. The first identity is retained and the
    /// divergence is latched as a sticky coherence fact the fence reports.
    ApplicationIdentityConflict,
    /// The table-policy ledger disagrees with the final draft state the independent
    /// audit recomputed, or the legacy policy walk's verdict disagrees with the
    /// ledger's canonical minimum. Unreachable from any input by construction — the
    /// one mutation surface records every crossing — so an occurrence is a producer
    /// defect, named by the check that caught it.
    LedgerDrift(&'static str),
    InvalidReference(&'static str),
    /// An emitted section's byte length disagrees with the length the measure core
    /// counted for it through the same writer. Unreachable from any input by
    /// construction — the counted==emitted KATs pin per-section length invariance — so
    /// an occurrence is a producer defect, named by the section that drifted.
    EncodeDrift(crate::measure::EncodeDriftSection),
}

impl std::fmt::Display for ImageBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The drifted region renders by name, so the one invariant that names a
            // section reads as one.
            ImageBuildError::EncodeDrift(section) => {
                write!(f, "image build error: encode drift in {section}")
            }
            other => write!(f, "image build error: {other:?}"),
        }
    }
}

impl std::error::Error for ImageBuildError {}

/// The mutable image builder.
///
/// Every owned mutation flows through the one admitted, journaled, failure-atomic
/// transaction surface: [`Self::savepoint`] mints a pre-admission token and
/// [`Self::begin_transaction`] consumes it into the armed [`DraftTxn`]. The
/// once-checked generic template pass holds such an armed guard and discards
/// everything it appended by dropping it.
///
/// A draft is deliberately not `Clone`. Every selector, handle, and site operand it mints
/// carries its identity, so a copied draft would carry a copied identity and a copied stamp
/// cursor: every capability minted afterwards would authenticate against both copies, and
/// the row-stamp check that makes a discarded row's reused ordinal detectable would answer
/// for a row that a different draft appended.
///
/// ```compile_fail,E0599
/// // A draft is not `Clone`: copying one would copy its identity and its stamp cursor.
/// let mut draft = marrow_image::ImageDraft::new();
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
/// let _ = draft.bind_occurrence_site(&0usize, &0usize, marrow_image::SemanticTarget::WholePayload);
/// ```
///
/// A handle already carries the one target it was bound for, so requesting a site takes no
/// second target input.
///
/// ```compile_fail,E0061
/// // A handle carries its target; there is no second target input to disagree with it.
/// let mut draft = marrow_image::ImageDraft::new();
/// let handle: marrow_image::OccurrenceSiteHandle = unimplemented!();
/// let _ = draft.request_site(&handle, marrow_image::SemanticTarget::WholePayload);
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
/// A transaction is a guard over one draft, not a mark a caller holds, so there is
/// no value to carry from one draft to another. The armed guard borrows its draft
/// exclusively for its whole lifetime.
///
/// ```compile_fail,E0505
/// // A guard cannot be separated from the draft it rolls back.
/// let mut first = marrow_image::ImageDraft::new();
/// let sp = first.savepoint();
/// let txn = first.begin_transaction(sp);
/// let moved = first;
/// drop(txn);
/// ```
#[derive(Debug)]
pub struct ImageDraft {
    /// The one durable-graph owner: this draft's strong identity and stamp source, its
    /// application identity, its canonical Product declaration table, its flat
    /// root-occurrence table, and the one value-shape arena their fields reference.
    ///
    /// The draft does not hold a second copy of any of them. It is the same owner the
    /// independent verifier builds from received bytes, so a Product declared here and one
    /// reconstructed there are admitted, stamped, and bounded by one implementation rather
    /// than by two that could drift.
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
    /// Two occurrences of one Product identity that claim different graphs are two
    /// declarations wearing one identity: the draft cannot represent that, and
    /// the measurement/coherence pass refuses to encode rather than silently
    /// canonicalizing one of them away.
    product_conflict: Option<ProductClaimConflict>,
    /// The sticky application-identity divergence latch (the set-once-or-same law).
    application_conflict: Option<ApplicationIdentityConflict>,
    /// The one owner of the operation-site table, its demand map, and its capacity
    /// policy. Every site an image carries is requested through it.
    sites: SiteDemandPlan,
    functions: Vec<FunctionDef>,
    /// The bytes the retained function bodies alone commit the image to, saturated at
    /// one past [`bounds::MAX_IMAGE_BYTES`] (see [`Self::function_payload_exceeds_image_limit`]).
    function_payload_charge: usize,
    exports: Vec<ExportDef>,
    test_entries: Vec<TestEntryDef>,
    /// The current one-shot transaction epoch (see [`TransactionEpoch`]). Its nested
    /// draft anchor distinguishes foreign savepoints without a second brand per token.
    epoch: Rc<TransactionEpoch>,
    /// The eight-slot policy-crossing observer the one mutation surface maintains.
    ledger: TablePolicyLedger,
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

/// The sticky application-identity divergence latch: the retained first identity and
/// the first divergent replacement. A coherence fact beside the Product claim
/// conflict, reported once at the fence by the owner that refuses artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplicationIdentityConflict {
    first: LedgerIdBytes,
    divergent: LedgerIdBytes,
}

/// The allocation-identity anchor of one draft: savepoint validation compares this
/// allocation by `Rc::ptr_eq`, and a savepoint's strong retention is what makes the
/// pointer comparison sound — the compared allocation cannot have been freed and
/// reused, so there is no address, counter, generation, or hash ABA. The numeric
/// [`DraftIdentity`] survives only where selectors and operands embed it as
/// predecessor provenance, never as a savepoint-comparison key.
#[derive(Debug)]
struct DraftIdentityCell;

/// The one-shot transaction epoch. Its nested allocation is the draft identity, so one
/// strong token carries both classifications: a different nested anchor is foreign and a
/// different epoch allocation over the same anchor is stale. Admission installs a fresh allocation before any
/// table mutation, staling every sibling savepoint of the consumed epoch. It is
/// monotone authentication state, not part of the logical inverse: commit and armed
/// rollback both retain the rotated epoch, so a sibling stays stale even when every
/// logical draft byte again equals the pre-transaction state.
#[derive(Debug)]
struct TransactionEpoch {
    draft: Rc<DraftIdentityCell>,
}

/// A hostile-state refusal of the transaction surface: the closed set the mutation
/// entry points return before any owner changes. Never a policy maximum — crossing a
/// public image policy is not a returned error anywhere on this surface.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftStateError {
    /// The token (savepoint, id, or reference) was minted by another draft.
    ForeignDraft,
    /// The transaction token's one-shot epoch belongs to another admission.
    StaleEpoch,
    /// The token is internally incoherent: its snapshot or authenticated fill state
    /// disagrees with the state it claims to describe, or the id it names no longer
    /// admits the operation.
    IncoherentToken,
    /// The argument exceeds the proved carrier/layout domain of the builder surface.
    /// The production compiler's envelope proof makes this unreachable, so it maps
    /// the refusal to a compiler invariant — never a policy or source refusal.
    CarrierDomain,
}

impl std::fmt::Display for DraftStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DraftStateError::ForeignDraft => "the token was minted by another draft",
            DraftStateError::StaleEpoch => "the transaction token belongs to another epoch",
            DraftStateError::IncoherentToken => "the token is internally incoherent",
            DraftStateError::CarrierDomain => {
                "the argument exceeds the proved carrier domain of the builder surface"
            }
        })
    }
}

impl std::error::Error for DraftStateError {}

/// A site operation the draft did not answer for is an incoherent token at the builder
/// surface: the reference names a row that no longer admits the operation. The site
/// error's own private cases stay private — this crossing carries the classification, not
/// the cause.
impl From<crate::site_plan::SitePlanStateError> for DraftStateError {
    fn from(_: crate::site_plan::SitePlanStateError) -> Self {
        DraftStateError::IncoherentToken
    }
}

/// One prepared string mint: the id the row will carry, and — when the row is new — the
/// spelling to append and the policy observations appending it crosses.
struct PreparedString {
    id: StrId,
    fresh: Option<FreshString>,
}

struct FreshString {
    text: String,
    observations: Vec<(TablePolicyKind, CurrentValidationOccurrence)>,
}

/// One prepared constant mint (see [`PreparedString`]).
struct PreparedConst {
    id: ConstId,
    fresh: Option<FreshConst>,
}

struct FreshConst {
    value: ConstValue,
    crosses: bool,
}

/// The private structural image a savepoint carries and a transaction's journal
/// restores to: every owner's append-only length, the durable graph's checkpoint,
/// and the conflict/receipt slots. Deliberately **not** a ledger copy or fill-state
/// scan: only an unarmed draft can mint a savepoint, and admission immediately rotates
/// the one-shot epoch before a transaction can fill anything. The armed inverse takes
/// its own fixed ledger copy at admission.
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

/// A pre-admission, affine draft savepoint: it strongly retains the draft's
/// current one-shot epoch (which owns the draft's allocation-identity anchor) and carries
/// the exact private restore snapshot. Sibling-mintable; consumed whole by
/// [`ImageDraft::begin_transaction`], which validates it by allocation identity
/// before any mutation. Deliberately neither `Clone` nor `Copy`: a savepoint is an
/// affine admission token, and copying one is how a consumed epoch gets re-presented.
///
/// ```compile_fail,E0599
/// // A savepoint is affine: it cannot be cloned.
/// let mut draft = marrow_image::ImageDraft::new();
/// let sp = draft.savepoint();
/// let _copy = sp.clone();
/// ```
/// ```compile_fail,E0382
/// // A consumed admitted savepoint cannot be re-presented. Admission takes the token by
/// // value and the type is neither `Clone` nor `Copy`, so a second admission with the
/// // same token does not reach a staleness check at all — it does not compile. That is
/// // strictly stronger than refusing it at run time, and it is why no runtime test can
/// // exercise "the reused admitted savepoint": the reuse is unrepresentable.
/// let mut draft = marrow_image::ImageDraft::new();
/// let sp = draft.savepoint();
/// drop(draft.begin_transaction(sp));
/// drop(draft.begin_transaction(sp));
/// ```
///
/// Savepoints and element references occupy separate domains, and the boundary is a
/// type fact rather than a convention. A savepoint authorizes mutation over a whole
/// draft for one epoch; it names no element and can neither mint nor validate one, so
/// holding one grants none of the handle-provenance authority an element reference
/// carries.
///
/// ```compile_fail,E0599
/// // A savepoint cannot mint, carry, or validate an element reference. The method named
/// // here is the transaction's real one, so the refusal is that a *savepoint* does not
/// // have it — not that no type does. A spelling no type carries would fail to compile
/// // for a reason that pins nothing about this boundary.
/// let mut draft = marrow_image::ImageDraft::new();
/// let sp = draft.savepoint();
/// let handle: marrow_image::OccurrenceSiteHandle = unimplemented!();
/// let _ = sp.request_site(&handle);
/// ```
///
/// The separation runs both ways: an element reference authenticates against the draft
/// and plan identity it was minted under, and carries no transaction epoch, so it can
/// neither open a transaction nor validate one.
///
/// ```compile_fail,E0599
/// // An element reference cannot open or validate a transaction epoch.
/// let mut draft = marrow_image::ImageDraft::new();
/// let element: marrow_image::PlannedSiteRef = unimplemented!();
/// let _ = draft.begin_transaction(element.savepoint());
/// ```
#[doc(hidden)]
pub struct DraftSavepoint {
    epoch: Rc<TransactionEpoch>,
    snapshot: DraftSnapshot,
}

impl std::fmt::Debug for DraftSavepoint {
    /// One fixed marker: the snapshot and tokens are the authority the value carries.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("draft savepoint")
    }
}

/// One journaled one-time fill of a pre-transaction row, holding the displaced
/// definition so the armed inverse restores it by moving it back — no allocation on
/// the `Drop` path. A fill of a row appended inside the transaction needs no entry:
/// its row truncates with the suffix.
#[derive(Debug)]
enum FillInverse {
    Record {
        row: usize,
        fields: Vec<FieldDef>,
    },
    Enum {
        row: usize,
        variants: Vec<VariantDef>,
    },
}

/// The pre-reserved inverse journal the armed guard holds from admission: the
/// admission-time structural image, the fixed ledger copy, and the one-time-fill
/// inverses. Every element's storage is reserved in the preflight of the mutation
/// that needs it, so the armed `Drop` inverse is a closed total operation —
/// allocation-free, assertion-free, indexing-free, and non-panicking.
#[derive(Debug)]
struct DraftJournal {
    at: DraftSnapshot,
    ledger: TablePolicyLedger,
    fills: Vec<FillInverse>,
}

/// The sole cross-crate mutation surface over one [`ImageDraft`]: an armed guard
/// admitted by [`ImageDraft::begin_transaction`] that mutates the borrowed draft
/// immediately and in place — it never batches, defers, schedules, or reorders a
/// call, which is what preserves mint-order-is-the-wire — while journaling the
/// inverses an armed rollback needs. [`DraftTxn::commit`] disarms and retains every
/// accepted observation; the armed `Drop` performs the total admitted inverse.
///
/// Reads pass through [`std::ops::Deref`] to the draft's read surface; the guard
/// exposes no `&mut ImageDraft`, so no mutation can bypass the journal.
///
/// ```compile_fail,E0599
/// // Only the unarmed draft can mint an admission token. A transaction cannot create a
/// // mid-transaction savepoint whose meaning would depend on later rollback.
/// let mut draft = marrow_image::ImageDraft::new();
/// let savepoint = draft.savepoint();
/// let mut txn = draft.begin_transaction(savepoint).unwrap();
/// let _ = txn.savepoint();
/// ```
#[doc(hidden)]
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

    /// Run the total admitted inverse now.
    ///
    /// This is the explicit spelling of the ordinary-refusal path. Dropping the guard
    /// restores exactly the same owners; a producer-owning aggregate then drops its
    /// still-private payload with this armed guard on an unwind or early `?` return.
    pub fn rollback(mut self) {
        self.rollback_armed();
        self.armed = false;
    }

    pub fn intern_string(&mut self, text: &str) -> Result<StrId, DraftStateError> {
        self.draft.intern_string(text)
    }

    pub fn intern_int(&mut self, value: i64) -> Result<ConstId, DraftStateError> {
        self.draft.intern_int(value)
    }

    pub fn intern_bool(&mut self, value: bool) -> Result<ConstId, DraftStateError> {
        self.draft.intern_bool(value)
    }

    pub fn intern_text(&mut self, text: &str) -> Result<ConstId, DraftStateError> {
        self.draft.intern_text(text)
    }

    pub fn intern_date(&mut self, days: i32) -> Result<ConstId, DraftStateError> {
        self.draft.intern_date(days)
    }

    pub fn intern_instant(&mut self, nanos: i128) -> Result<ConstId, DraftStateError> {
        self.draft.intern_instant(nanos)
    }

    pub fn intern_duration(&mut self, nanos: i128) -> Result<ConstId, DraftStateError> {
        self.draft.intern_duration(nanos)
    }

    pub fn add_record_type(&mut self, def: RecordTypeDef) -> Result<TypeId, DraftStateError> {
        self.draft.add_record_type(def)
    }

    pub fn add_enum_type(&mut self, def: EnumTypeDef) -> Result<EnumId, DraftStateError> {
        self.draft.add_enum_type(def)
    }

    /// Reserve a `Vacant` record row for a two-pass forward reference; it admits
    /// exactly one later fill and is the fence's coherence invariant if never filled.
    pub fn reserve_record_type(&mut self, name: StrId) -> Result<TypeId, DraftStateError> {
        self.draft.reserve_record_type(name)
    }

    /// Reserve a `Vacant` enum row (see [`Self::reserve_record_type`]).
    pub fn reserve_enum_type(&mut self, name: StrId) -> Result<EnumId, DraftStateError> {
        self.draft.reserve_enum_type(name)
    }

    pub fn add_collection_type(
        &mut self,
        def: CollectionTypeDef,
    ) -> Result<CollTypeId, DraftStateError> {
        self.draft.add_collection_type(def)
    }

    /// Fill the fields of an already-reserved record type, exactly once: a checked
    /// lookup — a foreign or stale id is the typed refusal, never a panic — and a
    /// one-time fill — a second fill is the typed refusal, never an overwrite. A fill
    /// of a pre-transaction row journals its displaced definition first.
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
            return Err(DraftStateError::IncoherentToken);
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
            return Err(DraftStateError::IncoherentToken);
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

    pub fn declare_product(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        entry_record: TypeId,
        members: Vec<DeclarationMemberDef>,
    ) -> Result<Vec<DeclarationMember>, SitePlanStateError> {
        self.draft
            .declare_product(plan, product, entry_record, members)
    }

    pub fn add_root_occurrence(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        def: RootOccurrenceDef,
    ) -> Result<AdmittedRoot, SitePlanStateError> {
        self.draft.add_root_occurrence(plan, product, def)
    }

    pub fn set_application_identity(&mut self, id: LedgerIdBytes) {
        self.draft.set_application_identity(id);
    }

    pub fn request_site(
        &mut self,
        handle: &OccurrenceSiteHandle,
    ) -> Result<PlannedSiteRef, SitePlanStateError> {
        self.draft.request_site(handle)
    }

    pub fn add_function(&mut self, def: FunctionDef) -> Result<FuncId, DraftStateError> {
        self.draft.add_function(def)
    }

    pub fn add_export(&mut self, id: ExportId, func: FuncId) {
        self.draft.add_export(id, func);
    }

    pub fn add_test_entry(&mut self, name: StrId, func: FuncId) {
        self.draft.add_test_entry(name, func);
    }

    /// Mint one scalar durable value shape into the draft's one arena — the typed
    /// appender that replaces the deleted raw `&mut` arena escape.
    pub fn value_scalar(&mut self, scalar: Scalar) -> Result<ValueShapeNodeId, DraftStateError> {
        self.draft.value_shapes_mut().scalar(scalar)
    }

    /// Mint one dense composite durable value shape into the draft's one arena.
    ///
    /// Checked at the surface: an arity past [`crate::bounds::MAX_STRUCT_LEAVES`] is the
    /// typed carrier-domain refusal — a coherence/logical-domain decision, not a policy
    /// kind. Leaf provenance is the arena's own decision, so a leaf minted by another
    /// arena is its [`DraftStateError::ForeignDraft`] rather than a second copy of the
    /// predicate here. Neither refusal mutates the arena, and the fence's whole-arena
    /// walk keeps the same bounds as defense in depth.
    pub fn value_struct(
        &mut self,
        leaves: Vec<ValueShapeNodeId>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        if leaves.len() > bounds::MAX_STRUCT_LEAVES {
            return Err(DraftStateError::CarrierDomain);
        }
        self.draft.value_shapes_mut().struct_shape(leaves)
    }

    /// Mint one enum durable value shape into the draft's one arena (checked at the
    /// surface exactly like [`Self::value_struct`], over the variant and payload
    /// bounds).
    pub fn value_enum(
        &mut self,
        identity: LedgerIdBytes,
        members: Vec<(LedgerIdBytes, Vec<ValueShapeNodeId>)>,
    ) -> Result<ValueShapeNodeId, DraftStateError> {
        if members.len() > bounds::MAX_VARIANTS {
            return Err(DraftStateError::CarrierDomain);
        }
        for (_, payload) in &members {
            if payload.len() > bounds::MAX_PAYLOAD_FIELDS {
                return Err(DraftStateError::CarrierDomain);
            }
        }
        self.draft.value_shapes_mut().enum_shape(identity, members)
    }

    /// The total admitted inverse, in reverse dependency order. Called only by the
    /// armed `Drop`: allocation-free, assertion-free, indexing-free, `drain`-free,
    /// and non-panicking on ordinary exit and during an existing unwind.
    fn rollback_armed(&mut self) {
        let at = &self.journal.at;
        let draft = &mut *self.draft;
        // 1. Dependent code suffixes first (preservation coverage until the
        //    function-slot refounding).
        draft.test_entries.truncate(at.test_entries);
        draft.exports.truncate(at.exports);
        draft.functions.truncate(at.functions);
        draft.function_payload_charge = at.function_payload_charge;
        // 2. The site plan: suffix pop with retained-map key removal, receipt restore.
        draft.sites.pop_suffix_to(at.sites, at.receipt);
        // 3. The durable graph: occurrence/product/value-arena suffix restore plus the
        //    application slot; the row-stamp counter is deliberately not restored.
        draft.durable.rewind_total(&at.durable);
        // 4. The sticky conflict latches: exact prior values.
        draft.product_conflict = at.product_conflict;
        draft.application_conflict = at.application_conflict;
        // 5. One-time fills of pre-transaction rows revert to their displaced
        //    definitions, moved back without allocating.
        while let Some(fill) = self.journal.fills.pop() {
            match fill {
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
        // 6/7. Table suffixes, with each interned owner's index key removed while the
        //      popped row is still live.
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
        // 8. The admission-time fixed ledger copy, byte for byte.
        draft.ledger = self.journal.ledger;
        // The consumed epoch is deliberately not restored: it is monotone
        // authentication state outside the logical inverse.
    }
}
// drop-path audit sentinel: end of DraftTxn::rollback_armed

impl Drop for DraftTxn<'_> {
    /// The armed inverse. A committed guard was disarmed and restores nothing.
    fn drop(&mut self) {
        if self.armed {
            self.rollback_armed();
        }
    }
}
// drop-path audit sentinel: end of DraftTxn::drop

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
        let draft = Rc::new(DraftIdentityCell);
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
            epoch: Rc::new(TransactionEpoch { draft }),
            ledger: TablePolicyLedger::vacant(),
        }
    }

    /// The live tables a site binding is validated and projected against.
    fn graph(&self) -> OccurrenceGraph<'_> {
        self.durable.occurrence_graph()
    }

    /// Intern a string, returning its logical id. Repeated interning of the same
    /// text returns the same id — the duplicate hit mutates nothing, including at a
    /// full table, so dedup runs before any policy observation. The mint is checked:
    /// a table at the `u32` carrier boundary is the closed carrier-domain refusal
    /// before any owner mutates.
    pub(crate) fn intern_string(&mut self, text: &str) -> Result<StrId, DraftStateError> {
        let prepared = self.prepare_string(text)?;
        Ok(self.commit_string(prepared))
    }

    /// Derive what interning `text` would append — the row's id, the spelling to append if
    /// it is new, and every policy kind the append would cross — without touching an owner.
    ///
    /// This is the read-only half of a string mint, and it is what lets a *compound*
    /// operation prepare both of its rows before either lands. Every way the mint can fail
    /// lives here, so the matching commit cannot stop partway.
    fn prepare_string(&self, text: &str) -> Result<PreparedString, DraftStateError> {
        if let Some(&id) = self.string_index.get(text) {
            return Ok(PreparedString { id, fresh: None });
        }
        let id = StrId(wide_ordinal(self.strings.len())?);
        let mut observations = Vec::new();
        if text.len() > bounds::MAX_STRING_BYTES {
            observations.push((
                TablePolicyKind::StringBytes,
                CurrentValidationOccurrence::at_row(id.0),
            ));
        }
        if self.strings.len() + 1 > bounds::MAX_STRINGS {
            observations.push((
                TablePolicyKind::Strings,
                CurrentValidationOccurrence::at_row(bounds::MAX_STRINGS as u32),
            ));
        }
        Ok(PreparedString {
            id,
            fresh: Some(FreshString {
                text: text.to_string(),
                observations,
            }),
        })
    }

    /// Apply a prepared string mint. Infallible by construction.
    fn commit_string(&mut self, prepared: PreparedString) -> StrId {
        let PreparedString { id, fresh } = prepared;
        let Some(FreshString { text, observations }) = fresh else {
            return id;
        };
        for (kind, occurrence) in observations {
            self.ledger.observe(kind, occurrence);
        }
        self.string_index.insert(text.clone(), id);
        self.strings.push(text);
        id
    }

    pub(crate) fn intern_int(&mut self, value: i64) -> Result<ConstId, DraftStateError> {
        self.intern_const(ConstValue::Int(value))
    }

    pub(crate) fn intern_bool(&mut self, value: bool) -> Result<ConstId, DraftStateError> {
        self.intern_const(ConstValue::Bool(value))
    }

    /// Intern a text constant, interning its backing string as needed: the whole
    /// compound — string row, constant row, both index entries, and every newly
    /// crossed policy kind — lands as one unit, and the whole compound's coupled
    /// preparation (both dedup lookups and both carrier domains) is read-only and
    /// complete before the first insert, so a refusal can never leave half the
    /// compound behind.
    pub(crate) fn intern_text(&mut self, text: &str) -> Result<ConstId, DraftStateError> {
        // One read-only delta for the whole compound: the string row, the constant row,
        // both index entries, and every policy kind either append crosses — all derived
        // before a single owner is touched. Deriving and mutating one row and then the
        // other would leave the string appended if the constant's own derivation refused.
        //
        // A fresh string implies a fresh `Text` constant: its `StrId` does not exist yet,
        // so no constant can already hold it. The constant's ordinal does not depend on the
        // string commit either, so both halves are derivable from the same preimage.
        let string = self.prepare_string(text)?;
        let konst = self.prepare_const(ConstValue::Text(string.id))?;
        // Nothing above mutated an owner, and nothing below can fail.
        self.commit_string(string);
        Ok(self.commit_const(konst))
    }

    /// Intern a `date` constant (days since the Unix epoch).
    pub(crate) fn intern_date(&mut self, days: i32) -> Result<ConstId, DraftStateError> {
        self.intern_const(ConstValue::Date(days))
    }

    /// Intern an `instant` constant (signed nanoseconds since the epoch).
    pub(crate) fn intern_instant(&mut self, nanos: i128) -> Result<ConstId, DraftStateError> {
        self.intern_const(ConstValue::Instant(nanos))
    }

    /// Intern a `duration` constant (signed nanoseconds).
    pub(crate) fn intern_duration(&mut self, nanos: i128) -> Result<ConstId, DraftStateError> {
        self.intern_const(ConstValue::Duration(nanos))
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
        let crosses = self.consts.len() + 1 > bounds::MAX_CONSTS;
        Ok(PreparedConst {
            id,
            fresh: Some(FreshConst { value, crosses }),
        })
    }

    /// Apply a prepared constant mint. Infallible by construction.
    fn commit_const(&mut self, prepared: PreparedConst) -> ConstId {
        let PreparedConst { id, fresh } = prepared;
        let Some(FreshConst { value, crosses }) = fresh else {
            return id;
        };
        self.consts.push(value);
        self.const_index.insert(value, id);
        if crosses {
            self.ledger.observe(
                TablePolicyKind::Consts,
                CurrentValidationOccurrence::at_row(bounds::MAX_CONSTS as u32),
            );
        }
        id
    }

    /// Add a record type with its **complete** definition: the row never spends a
    /// fill, so a later "fill" is the typed double-fill refusal, never a
    /// replacement. A two-pass forward reference reserves with
    /// [`Self::reserve_record_type`] instead.
    pub(crate) fn add_record_type(
        &mut self,
        def: RecordTypeDef,
    ) -> Result<TypeId, DraftStateError> {
        self.append_record_type(def, FillState::Filled)
    }

    /// Reserve a record row for a two-pass forward reference: the row is `Vacant` —
    /// distinct from a valid filled-empty definition — admits exactly one later
    /// fill, and is the fence's coherence invariant if it is never filled.
    pub(crate) fn reserve_record_type(&mut self, name: StrId) -> Result<TypeId, DraftStateError> {
        self.append_record_type(
            RecordTypeDef {
                name,
                fields: Vec::new(),
            },
            FillState::Unfilled,
        )
    }

    fn append_record_type(
        &mut self,
        def: RecordTypeDef,
        fill: FillState,
    ) -> Result<TypeId, DraftStateError> {
        let id = TypeId(wide_ordinal(self.types.len())?);
        self.types.push(def);
        self.types_fill.push(fill);
        if self.types.len() > bounds::MAX_TYPES {
            self.ledger.observe(
                TablePolicyKind::Types,
                CurrentValidationOccurrence::at_row(bounds::MAX_TYPES as u32),
            );
        }
        Ok(id)
    }

    /// Add an enum type with its **complete** definition (see
    /// [`Self::add_record_type`]).
    pub(crate) fn add_enum_type(&mut self, def: EnumTypeDef) -> Result<EnumId, DraftStateError> {
        self.append_enum_type(def, FillState::Filled)
    }

    /// Reserve an enum row for a two-pass forward reference (see
    /// [`Self::reserve_record_type`]).
    pub(crate) fn reserve_enum_type(&mut self, name: StrId) -> Result<EnumId, DraftStateError> {
        self.append_enum_type(
            EnumTypeDef {
                name,
                variants: Vec::new(),
            },
            FillState::Unfilled,
        )
    }

    fn append_enum_type(
        &mut self,
        def: EnumTypeDef,
        fill: FillState,
    ) -> Result<EnumId, DraftStateError> {
        let id = EnumId(wide_ordinal(self.enums.len())?);
        self.enums.push(def);
        self.enums_fill.push(fill);
        if self.enums.len() > bounds::MAX_ENUMS {
            self.ledger.observe(
                TablePolicyKind::Enums,
                CurrentValidationOccurrence::at_row(bounds::MAX_ENUMS as u32),
            );
        }
        Ok(id)
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
    pub(crate) fn add_collection_type(
        &mut self,
        def: CollectionTypeDef,
    ) -> Result<CollTypeId, DraftStateError> {
        let id = CollTypeId(wide_ordinal(self.colls.len())?);
        self.colls.push(def);
        if self.colls.len() > bounds::MAX_COLLECTIONS {
            self.ledger.observe(
                TablePolicyKind::Collections,
                CurrentValidationOccurrence::at_row(bounds::MAX_COLLECTIONS as u32),
            );
        }
        Ok(id)
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
    pub(crate) fn declare_product(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        entry_record: TypeId,
        members: Vec<DeclarationMemberDef>,
    ) -> Result<Vec<DeclarationMember>, SitePlanStateError> {
        let (row, conflict) = self
            .durable
            .admit_product(plan, product, entry_record, members)
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
    /// the coherence causes here, none of them projected: the named Product is not
    /// declared here, the completed row's published selectors exceed what a canonical
    /// path can address, or the occurrence is past the plan's admitted root count —
    /// each refused before the row is pushed. Crossing the public Roots policy is not
    /// refused anywhere on this surface: the N+1 occurrence commits with its ledger
    /// observation and the fence reports [`ImageBuildError::TooManyRoots`].
    pub(crate) fn add_root_occurrence(
        &mut self,
        plan: &AdmittedGraphInputPlan,
        product: LedgerIdBytes,
        def: RootOccurrenceDef,
    ) -> Result<AdmittedRoot, SitePlanStateError> {
        // Publication preflight: an occurrence whose managed-index ordinals cannot
        // all be spelled in the canonical addressable path domain is refused as one
        // typed error before any row is pushed and before any budget is spent —
        // admission is failure-atomic, and no refusal path leaves a live row.
        if def.indexes.len() > usize::from(u16::MAX) + 1 {
            return Err(SitePlanStateError::new(SitePlanState::InvalidDemand));
        }
        let occurrence = self
            .durable
            .admit_root_occurrence(plan, product, def)
            .map_err(|_| SitePlanStateError::new(SitePlanState::InvalidDemand))?;
        let root_id = occurrence.wire_root_id();
        // Preflighted above, so the just-pushed row always publishes; the refusal arm
        // stays as typed defense in depth, never a panic and never an orphan row.
        let (placement, indexes) = self
            .graph()
            .publish(&occurrence)
            .ok_or_else(|| SitePlanStateError::new(SitePlanState::StaleBinding))?;
        if self.root_occurrences().len() > bounds::MAX_ROOTS {
            self.ledger.observe(
                TablePolicyKind::Roots,
                CurrentValidationOccurrence::at_row(bounds::MAX_ROOTS as u32),
            );
        }
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
    /// Reading takes no construction budget: it appends nothing, and a draft that was never
    /// admitted any construction holds no declaration to read.
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
    /// occurrence's Product or exactly this occurrence's own root-scoped case, and that
    /// the one supplied target is the target that node admits. No later call accepts a
    /// second target, and the returned handle borrows nothing: the immutable borrow ends
    /// here, before [`Self::request_site`] takes the draft mutably.
    ///
    /// Binding takes no construction budget: it appends nothing, and the two selectors it
    /// proves live could only have been published by a construction that was admitted one.
    pub fn bind_occurrence_site(
        &self,
        root: &RootOccurrenceSelector,
        path: &CanonicalDeclarationPathSelector,
        target: SemanticTarget,
    ) -> Result<OccurrenceSiteHandle, SitePlanStateError> {
        let demand = self.graph().validate(root, path, target)?;
        Ok(OccurrenceSiteHandle::new(self.durable.identity(), demand))
    }

    /// Record the application's ledger id, set-once-or-same: the first set stores it,
    /// an equal reset is an idempotent no-op, and a divergent replacement latches the
    /// sticky [`ApplicationIdentityConflict`] the fence reports — the first identity
    /// is retained and never silently overwritten. Required exactly when the draft
    /// has a durable root; a storeless image carries none.
    pub(crate) fn set_application_identity(&mut self, id: LedgerIdBytes) {
        match self.durable.application() {
            None => self.durable.set_application_identity(id),
            Some(first) if first == id => {}
            Some(first) => {
                self.application_conflict
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
    /// The returned [`PlannedSiteRef`] is the only way an instruction can name a
    /// site: it is opaque, has no constructor of its own, and carries either the id the
    /// plan minted or the plan's refusal. A refusal is not a failure — the crossing is
    /// nonblocking, and the encoder refuses the image through the Sites bound — but there
    /// is no id that would not alias a fitting site, so none is carried.
    ///
    /// A request takes no construction budget: the site table is its own bounded owner —
    /// a construction plan admits graph input, not site capacity — and the handle a
    /// request spends could only have been bound against rows an admitted construction
    /// published.
    pub(crate) fn request_site(
        &mut self,
        handle: &OccurrenceSiteHandle,
    ) -> Result<PlannedSiteRef, SitePlanStateError> {
        if handle.draft() != self.durable.identity() {
            return Err(SitePlanStateError::new(SitePlanState::WrongPlan));
        }
        // The rows the handle was bound against may have been discarded since; rebinding
        // the same triple against the live tables is what proves they were not.
        let demand = handle.demand();
        let stamp = self.durable.next_stamp();
        let live = self.graph().revalidate(&demand)?;
        let site = self.sites.request(self.durable.identity(), live, stamp);
        // The one mint path is the one Sites observation point: a crossing is present
        // exactly when the plan holds its earliest receipt, recorded at the virtual
        // zero-based N+1 ordinal `MAX_SITES` — never a physical row index or wire id.
        if self.sites.receipt().is_some() {
            self.ledger.observe(
                TablePolicyKind::Sites,
                CurrentValidationOccurrence::at_row(bounds::MAX_SITES as u32),
            );
        }
        Ok(site)
    }

    /// Append a function body, validating every operation site its code names first.
    ///
    /// A site operand is evidence that *this* draft answered for a place. Appending code
    /// is where that evidence is spent, so it is checked here rather than trusted: an
    /// operand minted by another draft, or one whose site row or policy receipt was
    /// discarded, is refused and **no** row is appended. The success carrier is unchanged
    /// — a function is still named by its [`FuncId`] — so the check widens neither
    /// function identity nor the site-binding state error's authority.
    pub(crate) fn add_function(&mut self, def: FunctionDef) -> Result<FuncId, DraftStateError> {
        for instr in &def.code {
            if let Some(site) = instr.site_operand() {
                self.validate_site_ref(site)?;
            }
        }
        let id = FuncId(function_ordinal(self.functions.len())?);
        let floor = function_payload_floor(&def);
        self.functions.push(def);
        self.function_payload_charge = self
            .function_payload_charge
            .saturating_add(floor)
            .min(DECISIVE_FUNCTION_PAYLOAD);
        Ok(id)
    }

    /// Whether the retained function bodies alone already exceed
    /// [`bounds::MAX_IMAGE_BYTES`], so no completion of this draft can encode. A
    /// producer polls this after each settled body to stop retaining work the image
    /// cannot carry; the encoder's measurement remains the verdict on a draft that
    /// passes it.
    pub fn function_payload_exceeds_image_limit(&self) -> bool {
        self.function_payload_charge > bounds::MAX_IMAGE_BYTES
    }

    /// Bind the export identity `id` to function `func`. The compiler mints `id`
    /// with [`ExportId::of_local`] from the export's declaration path; at v0 each
    /// public function is one export, so `func` is unique across the table.
    pub(crate) fn add_export(&mut self, id: ExportId, func: FuncId) {
        self.exports.push(ExportDef { id, func });
    }

    /// Bind the report name `name` to the storeless test function `func`. Test names
    /// are unique across the project (the compiler rejects a duplicate), so the
    /// encoder sorts entries by their final name-string index.
    pub(crate) fn add_test_entry(&mut self, name: StrId, func: FuncId) {
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

    /// The draft's one durable value-shape arena, for the compiler to mint a field's
    /// value shape into. A declaration row can only carry a reference minted here, so
    /// there is no second place a value shape can come from.
    pub(crate) fn value_shapes_mut(&mut self) -> &mut CanonicalValueShapeDag {
        self.durable.value_shapes_mut()
    }

    /// The draft's one durable value-shape arena, for reading a minted shape's depth,
    /// kind, and references.
    pub fn value_shapes(&self) -> &CanonicalValueShapeDag {
        self.durable.value_shapes()
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
        self.durable.contract_view()
    }

    /// Mint one pre-admission savepoint of this draft's current state. Savepoints are
    /// sibling-mintable; each is an affine admission token
    /// [`Self::begin_transaction`] consumes.
    #[doc(hidden)]
    pub fn savepoint(&mut self) -> DraftSavepoint {
        DraftSavepoint {
            epoch: Rc::clone(&self.epoch),
            snapshot: self.snapshot(),
        }
    }

    /// Consume and validate `savepoint`, rotate the one-shot epoch, and return the
    /// armed [`DraftTxn`] — the sole cross-crate mutation surface.
    ///
    /// A foreign, stale, or internally incoherent token is the closed
    /// [`DraftStateError`] before any mutation, without rotating the epoch or
    /// changing any owner. On success the fresh epoch is installed before any table
    /// mutation, staling every sibling savepoint of the consumed epoch; the fixed
    /// ledger copy and the pre-reserved inverse journal arm the guard.
    #[doc(hidden)]
    pub fn begin_transaction(
        &mut self,
        savepoint: DraftSavepoint,
    ) -> Result<DraftTxn<'_>, DraftStateError> {
        if !Rc::ptr_eq(&self.epoch.draft, &savepoint.epoch.draft) {
            return Err(DraftStateError::ForeignDraft);
        }
        if !Rc::ptr_eq(&self.epoch, &savepoint.epoch) {
            return Err(DraftStateError::StaleEpoch);
        }
        if self.snapshot() != savepoint.snapshot {
            return Err(DraftStateError::IncoherentToken);
        }
        self.epoch = Rc::new(TransactionEpoch {
            draft: Rc::clone(&self.epoch.draft),
        });
        let ledger = self.ledger;
        Ok(DraftTxn {
            journal: DraftJournal {
                at: savepoint.snapshot,
                ledger,
                fills: Vec::new(),
            },
            draft: self,
            armed: true,
        })
    }

    /// The private structural image the savepoint carries and the journal restores
    /// to. The draft is destructured exhaustively, so a new owner stops this
    /// compiling until it is recorded here or deliberately excluded beside the
    /// owners whose exclusion [`DraftSnapshot`] states.
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
            epoch: _,
            ledger: _,
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

    /// The eight-slot policy ledger, for the fence's independent audit.
    pub(crate) fn policy_ledger(&self) -> &TablePolicyLedger {
        &self.ledger
    }

    /// The sticky application-identity divergence, if one was latched.
    pub(crate) fn application_conflict(&self) -> Option<ApplicationIdentityConflict> {
        self.application_conflict
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
    /// Write the site-table rows into `sink`: per retained row, the semantic path of
    /// the node the demand addresses — `u8(step_count) ‖ [u8(ledger_kind) ‖ 16 id
    /// bytes]*`, the same frozen ledger `IDREF` kinds a durable node's identity uses —
    /// then the one-byte operation target. The one site-row codec, driven by the
    /// measure core's counting run and by emission alike.
    ///
    /// Each row's steps are streamed twice through the one projection grammar
    /// ([`crate::product::OccurrenceGraph::project_steps`]): once to spell the
    /// count-first prefix, once to spell the steps — so no path is materialized and
    /// no row retains anything. The projection is the site table's only path source:
    /// a row retains ordinals into the occurrence and declaration tables, so the path
    /// a site encodes to is derived from the same rows the DURABLE member graph is
    /// written from and cannot disagree with them.
    ///
    /// The step count fits one byte off the bound the projection itself carries: a
    /// chain is at most `2 + MAX_DURABLE_DEPTH = MAX_SITE_PATH_STEPS` steps, which
    /// `bounds` const-asserts against one byte; the checked conversion keeps the
    /// totality tied to that bound rather than to a silent cast.
    pub(crate) fn write_site_rows(
        &self,
        sink: &mut impl ImageByteSink,
    ) -> Result<(), ImageBuildError> {
        if self.sites.rows().is_empty() {
            return Ok(());
        }
        let application = self
            .application_identity()
            .ok_or(ImageBuildError::InvalidReference("application identity"))?;
        let graph = self.graph();
        for row in self.sites.rows() {
            // A count already past the ceiling is decided; further rows only grow it.
            if sink.is_full() {
                return Ok(());
            }
            let mut steps = 0usize;
            graph
                .project_steps(application, row.key(), |_| steps += 1)
                .ok_or(ImageBuildError::InvalidReference("operation site"))?;
            let step_count = u8::try_from(steps)
                .expect("a bounded semantic path's step count fits the site-path width");
            sink.push(step_count);
            graph
                .project_steps(application, row.key(), |step| {
                    sink.push(step.kind.ledger_kind());
                    sink.extend_bytes(step.id.bytes());
                })
                .ok_or(ImageBuildError::InvalidReference("operation site"))?;
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

    /// Prove that every retained site row still projects, streaming each row's steps
    /// through the one projection grammar and retaining nothing — the coherence
    /// walk's validation-only twin of [`Self::write_site_rows`].
    pub(crate) fn validate_site_projection(&self) -> Result<(), ImageBuildError> {
        if self.sites.rows().is_empty() {
            return Ok(());
        }
        let application = self
            .application_identity()
            .ok_or(ImageBuildError::InvalidReference("application identity"))?;
        let graph = self.graph();
        for row in self.sites.rows() {
            graph
                .project_steps(application, row.key(), |_| {})
                .ok_or(ImageBuildError::InvalidReference("operation site"))?;
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
    /// live. An over-policy ref with intact provenance is valid here: it is live
    /// provenance the Sites policy candidate reports, never a coherence fault.
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

    /// The earliest site-policy crossing the plan has recorded, if any — the fence
    /// audit's recomputation source for the Sites ledger slot.
    pub(crate) fn site_receipt(&self) -> Option<SitePolicyReceipt> {
        self.sites.receipt()
    }

    /// The exact wire ordinal of one validated fitting site ref — the policy-clean final
    /// projection. Reached only through the measured wire plan's site projection, so no
    /// numeric site id exists before fitting policy-clean capped measurement.
    pub(crate) fn site_wire_ordinal(&self, site: &PlannedSiteRef) -> Result<u16, ImageBuildError> {
        if self.graph().revalidate(&site.demand()).is_err() {
            return Err(ImageBuildError::InvalidReference("operation site"));
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
            .map(|function| function.code.as_slice())
    }

    pub(crate) fn functions(&self) -> &[FunctionDef] {
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

    /// The canonical test-entry permutation (row law): the base-row indices ascending
    /// by remapped name index, computed by the table's one comparator.
    ///
    /// The raw map reads are owner-safe and deliberately outside the token seal: they
    /// are the comparator's keys, resolved once per base row in row order — so a name
    /// reference outside the pool reports exactly where the old sorted copy reported
    /// it — never a section writer resolving a reference it could branch on.
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
    /// reads one from received bytes and must be able to state it, and every owner that
    /// resolves one checks it against the pool it indexes.
    pub const fn from_index(index: u16) -> Self {
        Self(index as u32)
    }

    /// The wide logical ordinal. A logical ordinal, never a wire value: emission
    /// narrows only through the measure core's policy-clean path.
    pub const fn index(self) -> u32 {
        self.0
    }

    pub(crate) fn raw(self) -> u32 {
        self.0
    }
}

impl ConstValue {
    /// A sort key `(tag, payload-bytes)` where the Text payload is the *final*
    /// string index resolved through `str_map`.
    ///
    /// This raw map read is owner-safe and deliberately outside the token seal: it is
    /// the canonical-order *comparator* itself, fed by the checked base rows, not a
    /// section writer resolving a reference it could branch on.
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
mod ledger_corruption_tests {
    use super::ImageDraft;
    use crate::draft::ImageBuildError;
    use crate::policy_ledger::{CurrentValidationOccurrence, TablePolicyKind, TablePolicyLedger};

    /// A draft whose constant pool has crossed `MAX_CONSTS` through the production
    /// transaction surface.
    fn consts_crossed_owner() -> ImageDraft {
        let mut owner = ImageDraft::new();
        let savepoint = owner.savepoint();
        let mut txn = owner
            .begin_transaction(savepoint)
            .expect("a fresh savepoint admits");
        for value in 0..=(crate::bounds::MAX_CONSTS as i64) {
            txn.intern_int(value).expect("a within-domain mint");
        }
        txn.commit();
        owner
    }

    /// A missing, wrong-occurrence, or extra ledger state — unreachable through the one
    /// mutation surface, planted here through the owner's own private field — is refused
    /// at the fence as the ledger-drift invariant, before any policy verdict.
    #[test]
    fn a_corrupted_ledger_is_invariant_at_the_fence() {
        // Missing: a crossed draft whose ledger observed nothing.
        let mut owner = consts_crossed_owner();
        assert_eq!(
            owner.encode().map(|_| ()),
            Err(ImageBuildError::TooManyConsts),
            "the true ledger reaches the policy verdict",
        );
        owner.ledger = TablePolicyLedger::vacant();
        assert!(
            matches!(owner.encode(), Err(ImageBuildError::LedgerDrift(_))),
            "a vacant ledger over a crossed draft is the drift invariant",
        );

        // Wrong occurrence: the right slot at a coordinate the walk would never report.
        let mut owner = consts_crossed_owner();
        let mut wrong = TablePolicyLedger::vacant();
        wrong.observe(
            TablePolicyKind::Consts,
            CurrentValidationOccurrence::at_row(crate::bounds::MAX_CONSTS as u32 + 7),
        );
        owner.ledger = wrong;
        assert!(matches!(
            owner.encode(),
            Err(ImageBuildError::LedgerDrift(_))
        ));

        // Extra: a clean draft whose ledger claims a crossing that never happened.
        let mut owner = ImageDraft::new();
        let mut extra = TablePolicyLedger::vacant();
        extra.observe(
            TablePolicyKind::Strings,
            CurrentValidationOccurrence::at_row(crate::bounds::MAX_STRINGS as u32),
        );
        owner.ledger = extra;
        assert!(matches!(
            owner.encode(),
            Err(ImageBuildError::LedgerDrift(_))
        ));
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

        let list = draft
            .add_collection_type(CollectionTypeDef::List {
                elem: ImageType::scalar(Scalar::Int),
            })
            .expect("a within-domain mint");
        assert_eq!(list.index(), 0);
        assert_eq!(draft.collection_type_count(), 1);

        let map = draft
            .add_collection_type(CollectionTypeDef::Map {
                key: ImageType::scalar(Scalar::Text),
                value: ImageType::scalar(Scalar::Bool),
            })
            .expect("a within-domain mint");
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
        let name = draft.intern_string("r").expect("a within-domain mint");
        let value = draft
            .value_shapes_mut()
            .scalar(Scalar::Int)
            .expect("the test arena mints");
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
                    indexes: Vec::new().into(),
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
        draft.set_application_identity(LedgerIdBytes::from_bytes([0x01; 16]));
        // Build the rows inside an armed transaction, so dropping it discards them.
        let handle = {
            let savepoint = draft.savepoint();
            let mut proof = draft
                .begin_transaction(savepoint)
                .expect("a fresh savepoint admits");
            let name = proof.intern_string("r").expect("a within-domain mint");
            let value = proof
                .value_scalar(Scalar::Int)
                .expect("the test arena mints");
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
                        indexes: Vec::new().into(),
                    },
                )
                .expect("the Product is declared");
            proof
                .bind_occurrence_site(
                    admitted.occurrence(),
                    admitted.placement_path(),
                    SemanticTarget::WholePayload,
                )
                .expect("the root admits a whole-payload site")
        };

        assert_eq!(
            draft
                .request_site(&handle)
                .expect_err("the occurrence row was discarded"),
            SitePlanStateError::new(SitePlanState::StaleBinding),
        );

        // The same ordinals are re-minted deterministically; the handle must still not
        // authenticate the replacement.
        let name = draft.intern_string("r").expect("a within-domain mint");
        let value = draft
            .value_shapes_mut()
            .scalar(Scalar::Int)
            .expect("the test arena mints");
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
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");

        assert_eq!(
            draft
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
        use crate::durable_id::{DurableIndexComponent, DurableIndexShape};

        let mut draft = ImageDraft::new();
        draft.set_application_identity(LedgerIdBytes::from_bytes([0x01; 16]));
        let name = draft.intern_string("r").expect("a within-domain mint");
        let value = draft
            .value_shapes_mut()
            .scalar(Scalar::Int)
            .expect("the test arena mints");
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
                    indexes: vec![DurableIndexShape {
                        id: LedgerIdBytes::from_bytes([0x44; 16]),
                        unique: false,
                        components: vec![DurableIndexComponent::Field(field())],
                    }]
                    .into(),
                },
            )
            .expect("the Product is declared");
        let members = draft.product_members(product()).expect("declared");

        use crate::semantic::SemanticStepKind;
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
        assert!(draft.product_members(product()).is_none());
        assert!(draft.root_occurrences().is_empty());
    }
}

#[cfg(test)]
mod row_access_tests {
    use super::{FunctionDef, ImageDraft};
    use crate::export_id::ExportId;
    use crate::instr::Instr;
    use crate::ty::ImageType;

    #[test]
    fn function_code_borrows_the_appended_allocation_and_tracks_rollback() {
        let mut draft = ImageDraft::new();
        let name = draft.intern_string("body").expect("name fits");
        let source = draft.intern_string("source").expect("source fits");
        let code = vec![Instr::Return];
        let allocation = code.as_ptr();
        let savepoint = draft.savepoint();
        let mut txn = draft.begin_transaction(savepoint).expect("fresh savepoint");
        let func = txn
            .add_function(FunctionDef {
                name,
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                code,
                spans: Vec::new(),
            })
            .expect("no operation sites");
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
        let source = draft.intern_string("s").expect("a within-domain mint");
        let alpha = draft.intern_string("alpha").expect("a within-domain mint");
        let zeta = draft.intern_string("zeta").expect("a within-domain mint");
        let mut funcs = Vec::new();
        for name in [zeta, alpha] {
            funcs.push(
                draft
                    .add_function(FunctionDef {
                        name,
                        source,
                        params: Vec::new(),
                        ret: ImageType::Unit,
                        local_count: 0,
                        code: vec![Instr::Return],
                        spans: Vec::new(),
                    })
                    .expect("no site operand needs validating"),
            );
        }
        let first = ExportId::of_local("m", "zeta");
        let second = ExportId::of_local("m", "alpha");
        draft.add_export(first, funcs[0]);
        draft.add_export(second, funcs[1]);
        draft.add_test_entry(zeta, funcs[0]);

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
        let source = draft.intern_string("s").expect("a within-domain mint");
        let zeta = draft.intern_string("zeta").expect("a within-domain mint");
        let alpha = draft.intern_string("alpha").expect("a within-domain mint");
        for name in [zeta, alpha] {
            let func = draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: Vec::new(),
                    ret: ImageType::Unit,
                    local_count: 0,
                    code: vec![Instr::Return],
                    spans: Vec::new(),
                })
                .expect("no site operand needs validating");
            draft.add_test_entry(name, func);
        }
        // The pool interned [s, zeta, alpha]; byte-sorted it is [alpha, s, zeta], so
        // the remap is s→1, zeta→2, alpha→0 and the entries [zeta, alpha] come back
        // as [alpha, zeta].
        let str_map = vec![1u16, 2, 0];
        assert_eq!(draft.test_entry_permutation(&str_map), vec![1, 0]);
    }
}

#[cfg(test)]
mod function_payload_charge_tests {
    use super::{DECISIVE_FUNCTION_PAYLOAD, DraftStateError, FunctionDef, ImageDraft, SpanEntry};
    use crate::bounds::MAX_IMAGE_BYTES;
    use crate::encode::SPAN_ROW_BYTES;
    use crate::instr::Instr;
    use crate::ty::ImageType;

    fn body(draft: &mut ImageDraft, instructions: usize, spans: usize) -> FunctionDef {
        let name = draft.intern_string("body").expect("a within-domain mint");
        let source = draft.intern_string("src").expect("a within-domain mint");
        FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code: vec![Instr::Return; instructions],
            spans: (0..spans)
                .map(|index| SpanEntry {
                    instr_index: index as u32,
                    line: 1,
                    column: 1,
                })
                .collect(),
        }
    }

    /// The charge is the instruction count plus one span row per span, and the
    /// predicate flips exactly one byte past the image ceiling.
    #[test]
    fn a_successful_append_charges_its_instructions_and_span_rows() {
        let mut draft = ImageDraft::new();
        assert_eq!(draft.function_payload_charge, 0);
        let def = body(&mut draft, 7, 3);
        draft.add_function(def).expect("no site operand");
        assert_eq!(draft.function_payload_charge, 7 + 3 * SPAN_ROW_BYTES);
        assert!(!draft.function_payload_exceeds_image_limit());

        let remaining = MAX_IMAGE_BYTES - (7 + 3 * SPAN_ROW_BYTES);
        let def = body(&mut draft, remaining, 0);
        draft.add_function(def).expect("no site operand");
        assert_eq!(draft.function_payload_charge, MAX_IMAGE_BYTES);
        assert!(!draft.function_payload_exceeds_image_limit());

        let def = body(&mut draft, 1, 0);
        draft.add_function(def).expect("no site operand");
        assert_eq!(draft.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
        assert!(draft.function_payload_exceeds_image_limit());
    }

    /// The charge saturates one past the ceiling and stays there, however much more is
    /// appended.
    #[test]
    fn the_charge_saturates_at_the_decisive_total() {
        let mut draft = ImageDraft::new();
        let def = body(&mut draft, 1, MAX_IMAGE_BYTES);
        draft.add_function(def).expect("no site operand");
        assert_eq!(draft.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
        let def = body(&mut draft, MAX_IMAGE_BYTES, MAX_IMAGE_BYTES);
        draft.add_function(def).expect("no site operand");
        assert_eq!(draft.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
        assert!(draft.function_payload_exceeds_image_limit());
    }

    /// A refused append changes nothing: the function-slot carrier refusal leaves the
    /// charge at the accepted total.
    #[test]
    fn a_refused_append_leaves_the_charge_unchanged() {
        let mut draft = ImageDraft::new();
        for _ in 0..=u16::MAX {
            let def = body(&mut draft, 1, 0);
            draft.add_function(def).expect("a within-carrier ordinal");
        }
        let accepted = draft.function_payload_charge;
        assert_eq!(accepted, usize::from(u16::MAX) + 1);
        let def = body(&mut draft, 1, 0);
        assert!(matches!(
            draft.add_function(def),
            Err(DraftStateError::CarrierDomain)
        ));
        assert_eq!(draft.function_payload_charge, accepted);
        assert!(!draft.function_payload_exceeds_image_limit());
    }

    /// A transaction that saturated the charge rolls back to the exact pre-admission
    /// total; a committed one keeps it.
    #[test]
    fn rollback_restores_a_saturated_charge() {
        let mut draft = ImageDraft::new();
        let def = body(&mut draft, 5, 5);
        draft.add_function(def).expect("no site operand");
        let before = draft.function_payload_charge;
        let def = body(&mut draft, 1, MAX_IMAGE_BYTES);

        let savepoint = draft.savepoint();
        let mut txn = draft.begin_transaction(savepoint).expect("fresh savepoint");
        txn.add_function(def).expect("no site operand");
        assert!(txn.function_payload_exceeds_image_limit());
        txn.rollback();
        assert_eq!(draft.function_payload_charge, before);
        assert!(!draft.function_payload_exceeds_image_limit());

        let def = body(&mut draft, 1, MAX_IMAGE_BYTES);
        let savepoint = draft.savepoint();
        let mut txn = draft.begin_transaction(savepoint).expect("fresh savepoint");
        txn.add_function(def).expect("no site operand");
        txn.commit();
        assert_eq!(draft.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
        assert!(draft.function_payload_exceeds_image_limit());
    }
}
