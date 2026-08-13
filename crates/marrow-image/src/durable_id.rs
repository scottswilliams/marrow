//! The `DurableContractId` durable-graph identity (kernel identity rule).
//!
//! A [`DurableContractId`] is the stable 32-byte identity of a program's whole
//! durable graph — the application, the roots, their key columns, and each root
//! record's stored field profile — computed over the graph's **ledger ids**, the
//! entropy-minted identities the committed `.marrow/ids` artifact binds to each
//! durable declaration. Because the payload carries ids rather than names, a
//! rename (which moves a ledger anchor while its id stays) preserves the
//! contract identity, while every semantic graph change — a retyped key, a field
//! made required, a field added, removed, or re-minted — changes it. It crosses
//! the compiler → image → verifier boundary and will later cross the
//! store-admission boundary, so it is a distinct typed 32-byte domain-separated
//! SHA-256 over a length-delimited canonical payload, exactly as the kernel
//! identity rule requires: one owning phase (D00), one frozen `kind`, one
//! canonical payload, one known-answer test, and one independent-decoder
//! reconstruction test.
//!
//! The ledger ids themselves are the separate entropy-minted identity family;
//! this id is a deterministic hash *over* them. The compiler mints it and
//! carries it in the image; the verifier never trusts the carried bytes as
//! authoritative — it independently rebuilds the graph from the decoded tables,
//! recomputes the id over its own view of it, and rejects a mismatch. Anyone can mint a valid
//! id, so trust comes only from that recomputation.
//!
//! ```text
//! DurableContractId = SHA-256( KIND ‖ u64_be(len(payload)) ‖ payload )
//!   KIND    = b"marrow.durable.v0"
//!   payload = LP(lineage) ‖ LP(graph)
//!   LP(b)   = u64_be(b.len()) ‖ b
//!   lineage = the durable graph's package lineage. The local project root is the
//!             single tag byte 0x00; a dependency package is 0x01 ‖ <32-byte package
//!             id> at a later phase. The tag byte keeps the two disjoint, so packages
//!             are later breadth rather than an identity/format break — mirroring the
//!             `ExportId` lineage seam.
//!   graph   = u16_be(root_count)
//!             ‖ [ IDREF(0x00, application) when root_count > 0 ]
//!             ‖ root*                                          (roots in image order)
//!   root    = IDREF(0x03, placement) ‖ IDREF(0x01, product)
//!             ‖ u16_be(key_count) ‖ key*                       (key columns in tuple order)
//!             ‖ members                                        (the resource's durable member tree)
//!             ‖ indexes                                        (the root's managed indexes)
//!   indexes = u16_be(index_count) ‖ index*                    (in source declaration order)
//!   index   = IDREF(0x08, index) ‖ u8(unique?1:0)
//!             ‖ u16_be(component_count) ‖ component*           (projection leaves in projection order)
//!   component = IDREF(0x02, field) | IDREF(0x04, key)          (a projected top-level field or identity key)
//!   members = u16_be(member_count) ‖ member*                  (in source declaration order)
//!   member  = u8(member_tag) ‖ member_body
//!     field(0)  = IDREF(0x02, field) ‖ u8(required?1:0) ‖ value
//!     group(1)  = IDREF(0x07, group) ‖ members
//!     branch(2) = IDREF(0x03, placement) ‖ u16_be(key_count) ‖ key* ‖ members
//!   key     = u8(key_scalar_tag) ‖ IDREF(0x04, key)
//!   value   = u8(value_tag) ‖ value_body                       (a durable field's stored value shape)
//!     scalar(0) = u8(scalar_tag)                               (a nominal erases to its base scalar)
//!     struct(1) = u16_be(leaf_count) ‖ value*                  (dense struct leaves, all required; names are not identity)
//!     enum(2)   = IDREF(0x05, sum) ‖ u16_be(member_count) ‖ evalue*
//!   evalue  = IDREF(0x06, member) ‖ u16_be(payload_count) ‖ value*   (one enum member: its id and dense payload leaves)
//!   IDREF(k, id) = u8(k) ‖ u64_be(16) ‖ id                     (kind-tagged, LP 16 bytes)
//! ```
//!
//! A durable field's stored `value` is drawn from the closed acyclic durable value
//! set: a nominal scalar (erased to its base scalar), a dense `struct` (its leaves
//! recorded positionally as shape bytes — a nested product leaf mints no ledger id
//! of its own, because the containing field is the renamable durable declaration),
//! a closed `enum` (`Option`/`Result`/a user `enum`, each carrying a sum identity
//! (kind 5) and one member identity (kind 6) per variant so append-only member
//! evolution has stable per-member codes), or an `Option`, which is itself a closed
//! enum (`none`/`some`). Collections and nested sparse/place/function/handle leaves
//! are not durable value leaves. Only a durable-reachable enum contributes sum and
//! member ids; a storeless enum stays ledger-free.
//!
//! A key tuple is length-prefixed, so a singleton root (`key_count = 0`) and a
//! composite root (`key_count > 1`) are the same shape as the ordinary
//! single-column root, and key-column order is part of the identity.
//!
//! A resource's durable shape is a **member tree**: its top-level fields plus any
//! static `group` field-path namespaces and keyed `branch` placements, each of
//! which recursively holds its own members. A group is an unkeyed namespace (a
//! `Group` identity); a branch is a keyed placement (its own `Root`-kind placement
//! identity and key tuple), so a nested keyed subtree is a distinct graph node
//! with a complete identity, just like a root. Member order is source declaration
//! order and is part of the identity. Only the flat single-column-keyed root with
//! no groups or branches is executable in this preview; the wider shapes complete
//! their identity and verify but run at E01.
//!
//! A root's **managed indexes** follow its member tree: each is a narrow
//! compiler-maintained ordered projection from the keyed root, contributing its own
//! `Index` identity (kind 8), its `unique` flag, and its ordered projection of leaf
//! references — each a top-level stored `field` (kind 2) or an identity `key`
//! (kind 4) of the same root. An index stores no data of its own; it is derived from
//! the source leaves it projects, so its identity payload carries only leaf
//! references, never a value shape. Index order is source declaration order and is
//! part of the identity.
//!
//! The `IDREF` kind tags mirror the ledger's frozen kind space (application 0,
//! product 1, field 2, root/branch placement 3, key 4, group 7, index 8; 5-6 durable
//! enum sum/member). An
//! empty graph (no roots) has no application component: a storeless project needs
//! no ledger, so its contract commits to nothing. Scalar tags are the frozen
//! [`Scalar::tag`] bytes. The `member_tag` bytes (field 0, group 1, branch 2) are
//! internal to this payload and independent of the ledger kind space. Operation
//! *sites* are deliberately excluded: they are derivable access points over the
//! graph, not part of its durable identity.

use sha2::{Digest, Sha256};

use crate::bounds;
use crate::draft::{KeyColumn, StrId, TypeId};
use crate::product::{
    DeclarationMemberShape, DeclarationNode, ProductDeclaration, ProductDeclarationGraph,
    ProductDeclarationTable, RootOccurrence, RootOccurrenceTable,
};
use crate::semantic::{
    SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep, SemanticStepKind,
};
use crate::value_dag::{
    CanonicalValueShapeDag, ImageByteSink, ValueShapeNodeId, ValueShapeWireForm, expand, push_u16,
};

/// The domain-separation tag for the durable-contract identity. Distinct from every
/// other Marrow identity's `kind`, so a `DurableContractId` can never collide with
/// an `ImageId` or `ExportId` computed over the same bytes.
pub const DURABLE_CONTRACT_KIND: &[u8; 17] = b"marrow.durable.v0";

/// The lineage of a durable graph declared in the local project root: the single tag
/// byte `0x00`. A dependency package's lineage begins with `0x01` at a later phase,
/// so the tag byte alone keeps local and package lineages disjoint.
const LOCAL_ROOT_LINEAGE: &[u8] = &[0x00];

/// The frozen `IDREF` kind tags, mirroring the ledger's kind space.
const IDREF_APPLICATION: u8 = 0;
const IDREF_PRODUCT: u8 = 1;
const IDREF_FIELD: u8 = 2;
const IDREF_ROOT: u8 = 3;
const IDREF_KEY: u8 = 4;
/// The two kinds a durable value shape carries. The expansion owner
/// ([`crate::value_dag`]) writes them; they are declared here with the rest of the
/// mirror so the frozen kind space has one home.
pub(crate) const IDREF_SUM: u8 = 5;
pub(crate) const IDREF_MEMBER: u8 = 6;
const IDREF_GROUP: u8 = 7;
const IDREF_INDEX: u8 = 8;

/// The width of one raw ledger id, as the image's DURABLE section spells a reference.
const LEDGER_ID_BYTES: usize = 16;

/// The width of a length prefix: `u64_be(len)`.
const LP_HEADER_BYTES: usize = size_of::<u64>();

/// The width of one ledger reference in the canonical payload: `u8(kind) ‖ u64_be(16) ‖ id`.
const PREIMAGE_IDREF_BYTES: usize = 1 + LP_HEADER_BYTES + LEDGER_ID_BYTES;

/// The longest DURABLE body the measure core's capped counting admits: the whole-image
/// ceiling less the closing contract identity, which measurement charges against the ceiling
/// before the body is allocated.
const MAX_FITTING_DURABLE_BODY_BYTES: usize = bounds::MAX_IMAGE_BYTES - DurableContractId::BYTES;

/// The longest canonical graph payload a DURABLE body an image can carry can produce, and
/// so the length past which [`DurableContractView::contract_id`] refuses.
///
/// # The implication, term by term
///
/// The payload and the DURABLE body are the **same walk over the same rows**: the same
/// root occurrences in the same order, each projecting the same retained declaration and
/// the same value arena. They differ only in how a ledger reference is spelled, so each
/// payload term has a distinct body counterpart, and every body term the payload has no
/// term for only adds to the body:
///
/// | payload term | payload | body counterpart | body |
/// |---|---|---|---|
/// | root count, key count, member count, index count, component count | 2 | the same `u16` | 2 |
/// | application, root placement, root product, key, field, group, branch placement, index, enum sum, enum member | 25 | the raw id | 16 |
/// | index component (kind inside the `IDREF`) | 25 | `u8(kind)` and the raw id | 17 |
/// | member tag, required flag, unique flag, key scalar tag, value tags and scalar tags | 1 | the same byte | 1 |
/// | — | 0 | root name, root record, branch name, branch record, the site table | ≥ 0 |
///
/// Every row has `payload ≤ 25/16 × body`, and the ratio is reached only by a bare
/// reference; summing the rows gives `payload ≤ 25/16 × body` for the whole walk. The
/// measured plan admits a body only when `body + DurableContractId::BYTES ≤ MAX_IMAGE_BYTES`, so
/// the amplifiable term is [`MAX_FITTING_DURABLE_BODY_BYTES`] — the 32 identity bytes are
/// not body bytes and are not amplified, which is why the subtraction sits **inside** the
/// ratio rather than outside it.
///
/// # What the bound is not
///
/// It is far below what a caller holding an arena can *state*. A value shape shared across
/// nesting levels expands geometrically in its depth and the payload spells the expansion,
/// so the length of a graph's payload is not bounded by the size of the graph that
/// describes it. Past this length the identity is refused rather than computed, which is
/// what makes the cost of asking for an identity a property of this owner instead of a
/// promise each of its callers must keep.
const MAX_FITTING_CONTRACT_PREIMAGE_BYTES: usize =
    (MAX_FITTING_DURABLE_BODY_BYTES * PREIMAGE_IDREF_BYTES).div_ceil(LEDGER_ID_BYTES);

/// The derivation is exact arithmetic, not a saturating estimate.
const _: () = assert!(MAX_FITTING_DURABLE_BODY_BYTES <= usize::MAX / PREIMAGE_IDREF_BYTES);

/// A measurement-admitted body is inside the bound, so the refusal can only ever answer for a
/// graph no image could carry. Widening the image ceiling widens this one with it.
const _: () = assert!(MAX_FITTING_CONTRACT_PREIMAGE_BYTES >= MAX_FITTING_DURABLE_BODY_BYTES);

/// A durable graph no image could carry, refused before its identity is computed: its
/// canonical payload runs past [`MAX_FITTING_CONTRACT_PREIMAGE_BYTES`], or one of its nodes
/// states more positions than the `u16` both v0 wire forms count with can spell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableGraphTooLarge;

/// The length of the canonical payload, counted without writing it.
///
/// This sink writes nothing. It saturates one byte past
/// [`MAX_FITTING_CONTRACT_PREIMAGE_BYTES`] rather than at it, so "full" and "fits" stay
/// distinguishable, and every walk writing into it stops at [`ImageByteSink::is_full`] —
/// the work a graph stating an expansion no image could carry costs is the bytes the bound
/// admits, not the bytes the expansion would have produced, and no buffer is allocated to
/// find that out.
#[derive(Default)]
struct FittingPreimageLength(usize);

impl ImageByteSink for FittingPreimageLength {
    fn push(&mut self, _byte: u8) {
        self.0 += 1;
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        self.0 += bytes.len();
    }

    fn is_full(&self) -> bool {
        self.0 > MAX_FITTING_CONTRACT_PREIMAGE_BYTES
    }
}

/// The canonical payload streamed straight into the identity hash.
///
/// The construction is `SHA-256(KIND ‖ u64_be(len(payload)) ‖ payload)` over
/// `payload = LP(lineage) ‖ LP(graph)`, so both lengths are due before the first graph
/// byte. [`FittingPreimageLength`] supplies the one that is not a constant, which is what
/// lets the walk feed the hasher directly: no payload buffer is allocated at any size, for
/// any graph.
struct ContractPreimageHash(Sha256);

impl ContractPreimageHash {
    /// Open the hash over a payload whose graph is `graph_len` bytes long, writing the
    /// domain-separation tag, the payload length, the lineage component, and the graph
    /// component's own length prefix.
    fn opened(lineage: &[u8], graph_len: usize) -> Self {
        let payload_len = LP_HEADER_BYTES + lineage.len() + LP_HEADER_BYTES + graph_len;
        let mut hasher = Sha256::new();
        hasher.update(DURABLE_CONTRACT_KIND);
        hasher.update((payload_len as u64).to_be_bytes());
        let mut sink = Self(hasher);
        push_lp(&mut sink, lineage);
        sink.extend_bytes(&(graph_len as u64).to_be_bytes());
        sink
    }

    /// The identity of the graph just streamed through.
    fn finish(self) -> DurableContractId {
        DurableContractId(self.0.finalize().into())
    }
}

impl ImageByteSink for ContractPreimageHash {
    fn push(&mut self, byte: u8) {
        self.0.update([byte]);
    }

    fn extend_bytes(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

/// The frozen member-tag bytes distinguishing the three durable member kinds
/// within the canonical payload. They are internal to this encoding and separate
/// from the ledger `IDREF` kind space.
const MEMBER_FIELD: u8 = 0;
const MEMBER_GROUP: u8 = 1;
const MEMBER_BRANCH: u8 = 2;

/// An entropy-minted 128-bit ledger id as the image carries it: 16 opaque bytes.
/// The artifact-side semantics (anchors, tombstones, hex spelling) live with the
/// ledger owner; the image only transports and hashes the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LedgerIdBytes([u8; 16]);

impl LedgerIdBytes {
    /// Wrap 16 raw id bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The 16 id bytes.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Declares one durable ledger identity kind as its own type over [`LedgerIdBytes`].
///
/// The ledger mints every durable identity into the same opaque 16-byte space, so a
/// Product identity, a placement, a key, a field, a group, a branch placement, an
/// index, an enum sum, and an enum member are structurally indistinguishable once
/// they are bytes. A declaration identity and an occurrence identity mean different
/// things at every boundary they cross, and mistaking one for the other is a
/// soundness fault, not a typo. Each kind is therefore its own type with a private
/// field. There is deliberately no `From`, `Into`, or shared trait between any two
/// of them: the only way to obtain one is to mint it where the ledger kind is
/// already known, and the only way to leave the type is to read its bytes for
/// encoding or hashing.
macro_rules! durable_identity {
    ($(#[$meta:meta])* $name:ident, $mint:expr) => {
        $(#[$meta])*
        ///
        #[doc = $mint]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(LedgerIdBytes);

        impl $name {
            /// Mint this identity from the ledger id resolved for its own kind.
            pub fn minted(id: LedgerIdBytes) -> Self {
                Self(id)
            }

            /// The ledger id bytes, for encoding and hashing only.
            pub fn ledger_id(&self) -> LedgerIdBytes {
                self.0
            }
        }
    };
}

durable_identity!(
    /// The ledger identity of one durable Product DECLARATION: the resource type a
    /// `store` root or a nested keyed branch projects. One Product declaration has
    /// one canonical member/value graph and one runtime surface however many roots
    /// occur over it.
    ///
    /// It is never a placement, key, field, group, branch, index, sum, or member
    /// identity, and it is never a package declaration identity: a dependency
    /// package cannot declare or mint a durable Product, and no part of a package
    /// identity is reserved by or convertible to this type.
    DurableProductIdentity,
    "Minted where the ledger resolves the `Product` kind (tag 1), anchored at the resource-type spelling."
);

durable_identity!(
    /// The ledger identity of one outer `store` root placement — an OCCURRENCE of
    /// the Product it names, carrying its own spelling, key tuple, and managed
    /// indexes.
    RootPlacementIdentity,
    "Minted where the ledger resolves the `Root` kind (tag 3) for a `store` root."
);

durable_identity!(
    /// The ledger identity of one nested keyed branch placement. A branch placement
    /// is a Product declaration fact — it belongs to the resource's member graph —
    /// even though it shares the ledger's `Root` kind with an outer store root.
    BranchPlacementIdentity,
    "Minted where the ledger resolves the `Root` kind (tag 3) for a nested keyed branch."
);

durable_identity!(
    /// The ledger identity of one key column of a placement.
    DurableKeyIdentity,
    "Minted where the ledger resolves the `Key` kind (tag 4)."
);

durable_identity!(
    /// The ledger identity of one stored field declaration of a resource, group, or
    /// branch.
    DurableFieldIdentity,
    "Minted where the ledger resolves the `Field` kind (tag 2)."
);

durable_identity!(
    /// The ledger identity of one unkeyed static field-path namespace (`group`).
    DurableGroupIdentity,
    "Minted where the ledger resolves the `Group` kind (tag 7)."
);

durable_identity!(
    /// The ledger identity of one compiler-maintained managed index of a keyed store
    /// root. An index belongs to the root occurrence that declares it, not to the
    /// Product.
    ManagedIndexIdentity,
    "Minted where the ledger resolves the `Index` kind (tag 8)."
);

durable_identity!(
    /// The ledger identity of one durable-reachable closed enum (sum) type.
    DurableSumIdentity,
    "Minted where the ledger resolves the `Sum` kind (tag 5)."
);

durable_identity!(
    /// The ledger identity of one variant of a durable-reachable closed enum.
    DurableMemberIdentity,
    "Minted where the ledger resolves the `Member` kind (tag 6)."
);

/// One projected leaf of a managed index, as it contributes to the contract
/// identity: a top-level stored `field` or an identity `key` of the index's root,
/// referenced by its ledger id. An index stores no data of its own, so a component
/// is a leaf reference, never a value shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableIndexComponent {
    Field(LedgerIdBytes),
    Key(LedgerIdBytes),
}

impl DurableIndexComponent {
    /// The referenced leaf's ledger id.
    pub fn id(self) -> LedgerIdBytes {
        match self {
            DurableIndexComponent::Field(id) | DurableIndexComponent::Key(id) => id,
        }
    }
}

/// One narrow compiler-maintained managed index of a keyed root, as it contributes
/// to the contract identity: its own `Index` ledger id, its `unique` flag, and its
/// ordered projection of leaf references. Projection order is the declared component
/// order and is part of the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableIndexShape {
    pub id: LedgerIdBytes,
    pub unique: bool,
    pub components: Vec<DurableIndexComponent>,
}

/// A zero-allocation, non-owning view over one program's whole durable contract graph:
/// the application identity, the canonical Product declaration table, the flat root
/// occurrence table, and the one value-shape DAG their fields reference.
///
/// This is the single owner of the contract's canonical payload and of the graph's
/// derived path identity. The compiler views the tables its draft already holds and the
/// independent verifier views the ones it rebuilt from received bytes, so there is
/// exactly one canonical encoding and agreement between the two sides is a
/// recomputation rather than a trusted transfer.
///
/// It owns nothing and allocates nothing. A root occurrence projects the Product
/// declaration it names, so one Product's member graph is walked once per occurrence
/// and stored once however many occurrences project it — the graph is never copied per
/// root. Every borrowed child view below has private fields and no constructor, so a
/// caller reads the graph a producer built and can state none of its own.
#[derive(Debug, Clone, Copy)]
pub struct DurableContractView<'a> {
    application: Option<LedgerIdBytes>,
    products: &'a ProductDeclarationTable,
    occurrences: &'a RootOccurrenceTable,
    values: &'a CanonicalValueShapeDag,
}

/// One root occurrence as the contract sees it: its own placement, spelling, key tuple
/// and managed indexes, projected onto the Product declaration it names.
#[derive(Debug, Clone, Copy)]
pub struct DurableRootView<'a> {
    occurrence: &'a RootOccurrence,
    declaration: &'a ProductDeclaration,
}

/// One member row of a Product declaration, borrowed in place.
#[derive(Debug, Clone, Copy)]
pub struct DurableMemberView<'a> {
    graph: &'a ProductDeclarationGraph,
    node: &'a DeclarationNode,
}

/// What one member row declares: a stored field, a static `group` namespace, or a
/// keyed `branch` placement.
///
/// Each variant carries a borrowed view with private fields, so the closed set can be
/// read and matched outside this crate but stated only by a producer that built the
/// rows. It carries no member vector of its own: a group's and a branch's members are
/// reached through [`DurableMemberView::members`], so this type cannot express a tree.
///
/// Those two properties together are what closed the abort this row exists for. The
/// family this replaced was a public recursive tree with public fields: an external caller
/// could nest a hundred thousand groups from struct literals alone, and the process died
/// either while building the chain or in its recursive `Drop` after the entry function had
/// already refused it. No entry function can bound an argument its caller already built —
/// only unconstructibility can, and this is where it is enforced.
///
/// ```compile_fail,E0451
/// // A caller outside the crate cannot state a member kind: the payload has no public
/// // field and no constructor, so there is no literal to write.
/// let forged = marrow_image::DurableGroupView { id: unimplemented!() };
/// ```
///
/// ```compile_fail,E0609
/// // Nor can one be taken apart into an owned run to nest by hand.
/// fn nest(kind: marrow_image::DurableMemberViewKind<'_>) {
///     if let marrow_image::DurableMemberViewKind::Group(group) = kind {
///         let _members = group.members;
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub enum DurableMemberViewKind<'a> {
    Field(DurableFieldView),
    Group(DurableGroupView),
    Branch(DurableBranchView<'a>),
}

/// One stored field of a durable resource, group, or branch: its ledger id, whether it
/// is required, and a reference to its stored value shape in the graph's one arena. The
/// field's *name* is not part of the identity — a rename preserves it — but its value
/// shape is.
///
/// Every fact it carries is a small copied value, so it borrows the graph for nothing and
/// takes no lifetime: a caller may keep one past the walk that produced it.
#[derive(Debug, Clone, Copy)]
pub struct DurableFieldView {
    id: LedgerIdBytes,
    required: bool,
    value: ValueShapeNodeId,
}

/// One static field-path namespace (`group`): its `Group` ledger id. A group is an
/// unkeyed pathing construct; it stores no data of its own beyond its members.
#[derive(Debug, Clone, Copy)]
pub struct DurableGroupView {
    id: LedgerIdBytes,
}

/// One keyed subtree (`branch`): its own placement id and ordered key tuple, plus the
/// surface facts the physical layer needs. A branch is a distinct keyed graph node
/// nested under its containing resource, branch, or group — the same placement/key
/// shape as a root, without a separate product.
#[derive(Debug, Clone, Copy)]
pub struct DurableBranchView<'a> {
    placement: &'a LedgerIdBytes,
    name: StrId,
    record: TypeId,
    keys: &'a [KeyColumn],
}

impl DurableFieldView {
    /// The field's `Field` ledger id.
    pub fn id(&self) -> LedgerIdBytes {
        self.id
    }

    /// Whether the field is required.
    pub fn required(&self) -> bool {
        self.required
    }

    /// The field's stored value shape, in the graph's one arena.
    pub fn value(&self) -> ValueShapeNodeId {
        self.value
    }
}

impl DurableGroupView {
    /// The namespace's `Group` ledger id.
    pub fn id(&self) -> LedgerIdBytes {
        self.id
    }
}

impl<'a> DurableBranchView<'a> {
    /// The branch's own `Root`-kind placement id.
    pub fn placement(&self) -> LedgerIdBytes {
        *self.placement
    }

    /// The branch's interned source name — surface, not identity.
    pub fn name(&self) -> StrId {
        self.name
    }

    /// The branch entry's materialized record type — surface, not identity.
    pub fn record(&self) -> TypeId {
        self.record
    }

    /// The branch's ordered key tuple.
    pub fn keys(&self) -> &'a [KeyColumn] {
        self.keys
    }
}

impl<'a> DurableMemberView<'a> {
    /// What this row declares.
    pub fn kind(&self) -> DurableMemberViewKind<'a> {
        match self.node.shape() {
            DeclarationMemberShape::Field {
                id,
                required,
                value,
            } => DurableMemberViewKind::Field(DurableFieldView {
                id: *id,
                required: *required,
                value: *value,
            }),
            DeclarationMemberShape::Group { id } => {
                DurableMemberViewKind::Group(DurableGroupView { id: *id })
            }
            DeclarationMemberShape::Branch {
                placement,
                name,
                record,
                keys,
            } => DurableMemberViewKind::Branch(DurableBranchView {
                placement,
                name: *name,
                record: *record,
                keys,
            }),
        }
    }

    /// This row's direct members, in declaration order. A field declares none.
    pub fn members(&self) -> DurableMemberViews<'a> {
        DurableMemberViews::over(self.graph, self.graph.members_of(self.node))
    }
}

impl<'a> DurableRootView<'a> {
    /// The root's interned source name — surface, not identity.
    pub fn name(&self) -> StrId {
        self.occurrence.name()
    }

    /// The root's own occurrence placement identity.
    pub fn placement(&self) -> RootPlacementIdentity {
        self.occurrence.placement()
    }

    /// The Product declaration this occurrence projects.
    pub fn product(&self) -> DurableProductIdentity {
        self.declaration.identity()
    }

    /// The materialized entry record the Product's roots read and write.
    pub fn entry_record(&self) -> TypeId {
        self.declaration.root_entry_record()
    }

    /// The root's ordered key tuple; empty for a singleton root.
    pub fn keys(&self) -> &'a [KeyColumn] {
        self.occurrence.keys()
    }

    /// The root's ordered managed indexes.
    pub fn indexes(&self) -> &'a [DurableIndexShape] {
        self.occurrence.indexes()
    }

    /// The projected Product's direct members, in declaration order.
    pub fn members(&self) -> DurableMemberViews<'a> {
        let graph = self.declaration.graph();
        DurableMemberViews::over(graph, graph.members())
    }
}

/// One contiguous run of member rows, borrowed as views over the graph that holds them.
///
/// It is one named type rather than two opaque ones because a walk of the graph carries
/// a run of any level on one stack.
#[derive(Debug, Clone)]
pub struct DurableMemberViews<'a> {
    graph: &'a ProductDeclarationGraph,
    run: std::slice::Iter<'a, DeclarationNode>,
}

impl<'a> DurableMemberViews<'a> {
    pub(crate) fn over(graph: &'a ProductDeclarationGraph, run: &'a [DeclarationNode]) -> Self {
        Self {
            graph,
            run: run.iter(),
        }
    }
}

impl<'a> Iterator for DurableMemberViews<'a> {
    type Item = DurableMemberView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let graph = self.graph;
        self.run
            .next()
            .map(|node| DurableMemberView { graph, node })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.run.size_hint()
    }
}

impl ExactSizeIterator for DurableMemberViews<'_> {}

impl<'a> DurableContractView<'a> {
    /// View the durable contract graph held by these four owners.
    ///
    /// Crate-internal: the compiler's draft and the verifier's accepted graph are the
    /// only owners of a canonical Product/occurrence/value-shape table set, so a view
    /// exists only over rows one of them built.
    pub(crate) fn over(
        application: Option<LedgerIdBytes>,
        products: &'a ProductDeclarationTable,
        occurrences: &'a RootOccurrenceTable,
        values: &'a CanonicalValueShapeDag,
    ) -> Self {
        Self {
            application,
            products,
            occurrences,
            values,
        }
    }

    /// The graph's application identity, absent exactly when it has no root.
    pub fn application(&self) -> Option<LedgerIdBytes> {
        self.application
    }

    /// The one value-shape DAG every field of this graph references.
    pub fn value_shapes(&self) -> &'a CanonicalValueShapeDag {
        self.values
    }

    /// The graph's root occurrences, in image order.
    pub fn roots(&self) -> impl ExactSizeIterator<Item = DurableRootView<'a>> + use<'a> {
        let products = self.products;
        self.occurrences
            .rows()
            .iter()
            .map(move |occurrence| DurableRootView {
                occurrence,
                declaration: products.declaration(occurrence.declaration()),
            })
    }

    /// The stable 32-byte identity of this durable graph in the local project root, or
    /// [`DurableGraphTooLarge`] for a graph no image could carry: one whose canonical
    /// payload is longer than [`MAX_FITTING_CONTRACT_PREIMAGE_BYTES`], or one stating an
    /// arity wider than the `u16` the payload spells a count with.
    ///
    /// The refusal is the whole reason asking for an identity is bounded work: a view is
    /// a view over an arena, and an arena can state a value shape whose expansion —
    /// which is what the payload spells — is exponential in its declared levels. No
    /// caller has to establish that before asking, and none has to establish the graph's
    /// arities either.
    ///
    /// The payload is never materialized. Its length is counted by one walk and its bytes
    /// are streamed into the hash by a second walk over the same rows, so the cost of an
    /// identity is the walk, whatever the graph.
    pub fn contract_id(&self) -> Result<DurableContractId, DurableGraphTooLarge> {
        let mut length = FittingPreimageLength::default();
        self.write_graph(&mut length)?;
        if length.is_full() {
            return Err(DurableGraphTooLarge);
        }
        let mut hash = ContractPreimageHash::opened(LOCAL_ROOT_LINEAGE, length.0);
        self.write_graph(&mut hash)?;
        Ok(hash.finish())
    }

    /// Enumerate every durable graph node paired with its derived [`SemanticPath`]:
    /// each root placement, static `group` namespace, keyed `branch` placement, and
    /// stored field, in a stable pre-order (a node before its descendants, members in
    /// declaration order). The path is the chain of kind-tagged ledger ids from the
    /// application to the node, so a rename that only moves ledger anchors leaves
    /// every path unchanged while any structural or id change alters exactly the
    /// paths through it. The empty graph yields no nodes.
    ///
    /// This is the single owner of the derived path identity; the compiler views its
    /// resolved graph and the verifier views the one it rebuilt from the decoded image
    /// tables, so both enumerate identical paths.
    pub fn semantic_nodes(&self) -> Vec<SemanticNode> {
        let Some(application) = self.application else {
            return Vec::new();
        };
        let mut nodes = Vec::new();
        for root in self.roots() {
            let root_path = SemanticPath::root(application, root.placement().ledger_id());
            nodes.push(SemanticNode {
                kind: SemanticNodeKind::Root,
                path: root_path.clone(),
            });
            collect_member_nodes(&root_path, root.members(), &mut nodes);
            // A managed index is a graph node too: its path is the root path extended
            // by the index step, so a rename that only moves the index anchor leaves it
            // unchanged. Index nodes follow the member nodes, in declaration order.
            for index in root.indexes() {
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Index,
                    path: root_path.extend(SemanticStep::new(SemanticStepKind::Index, index.id)),
                });
            }
        }
        nodes
    }

    /// Write the canonical graph bytes (the `graph` production above) into `out`.
    ///
    /// One owner writes the payload for both of its readers: the length that only counts
    /// the bytes, and the hash that consumes them. The identity is therefore computed over
    /// the payload whose length framed it, because it is the same walk over the same rows.
    ///
    /// The walk stops at the first byte past a sink's own bound, so a graph stating an
    /// expansion no image could carry costs the bytes that bound admits rather than the
    /// bytes it would have produced.
    fn write_graph(&self, out: &mut impl ImageByteSink) -> Result<(), DurableGraphTooLarge> {
        push_u16(out, payload_count(self.occurrences.len())?);
        if let Some(application) = &self.application {
            push_idref(out, IDREF_APPLICATION, application);
        }
        for root in self.roots() {
            if out.is_full() {
                break;
            }
            push_idref(out, IDREF_ROOT, &root.placement().ledger_id());
            push_idref(out, IDREF_PRODUCT, &root.product().ledger_id());
            push_keys(out, root.keys())?;
            push_members(out, root.members(), self.values)?;
            push_indexes(out, root.indexes())?;
        }
        Ok(())
    }
}

/// The canonical payload's `u16` count for `count` graph positions, or
/// [`DurableGraphTooLarge`] for a count the payload cannot spell.
///
/// Every count the payload spells — roots, key columns, members, index components — is
/// bounded far below `u16::MAX` in [`crate::bounds`], and the encoder rechecks each of
/// them before a view is taken. A view's public constructor takes the graph
/// a caller states, though, so a wider one can reach here directly. A wrapping cast would
/// let it present a narrower graph's count and so share its identity; refusing instead
/// keeps that answer typed and keeps it this owner's.
///
/// [`MAX_FITTING_CONTRACT_PREIMAGE_BYTES`] cannot answer for these graphs: a count is
/// spelled before the positions it counts are walked, so the bound has seen none of their
/// bytes when the count is due. The refusal is the same one because the conclusion is the same — the
/// image's DURABLE section spells the identical arity as a `u16`, so a graph refused here
/// has no encodable image either.
fn payload_count(count: usize) -> Result<u16, DurableGraphTooLarge> {
    u16::try_from(count).map_err(|_| DurableGraphTooLarge)
}

/// Append a root's managed indexes: `u16_be(count) ‖ index*`, each an `Index` IDREF,
/// its `unique` flag byte, and its ordered projection of leaf-reference IDREFs (a
/// `field` (2) or `key` (4) IDREF per component). Projection order is load-bearing.
fn push_indexes(
    out: &mut impl ImageByteSink,
    indexes: &[DurableIndexShape],
) -> Result<(), DurableGraphTooLarge> {
    push_u16(out, payload_count(indexes.len())?);
    for index in indexes {
        push_idref(out, IDREF_INDEX, &index.id);
        out.push(u8::from(index.unique));
        push_u16(out, payload_count(index.components.len())?);
        for component in &index.components {
            match component {
                DurableIndexComponent::Field(id) => push_idref(out, IDREF_FIELD, id),
                DurableIndexComponent::Key(id) => push_idref(out, IDREF_KEY, id),
            }
        }
    }
    Ok(())
}

/// Walk one member run under `container`'s path, appending a [`SemanticNode`] for
/// each field, group, and branch in declaration order (a node before its
/// descendants). A group and a branch each extend the path with their own step and
/// descend; a field is a leaf. Key columns are placement identity attributes, not
/// nodes, so they are not walked.
///
/// The descent is an explicit stack of member runs, not recursion: nesting depth is a
/// property of the rows a producer built, and this walk's own stack use must not be.
///
/// Each node's path is materialized by cloning its container's step chain, so the walk
/// costs `nodes x depth` steps rather than `nodes`. Both factors are enforced bounds —
/// `MAX_DURABLE_MEMBERS` rows per declaration, each at most `MAX_DURABLE_DEPTH` deep,
/// refused at construction — so the product is a fixed ceiling (8192 x 16 steps) and not a
/// term an input can grow.
fn collect_member_nodes(
    container: &SemanticPath,
    members: DurableMemberViews<'_>,
    nodes: &mut Vec<SemanticNode>,
) {
    let mut stack = vec![(container.clone(), members)];
    while let Some((path, run)) = stack.last_mut() {
        let Some(member) = run.next() else {
            stack.pop();
            continue;
        };
        let container = path.clone();
        match member.kind() {
            DurableMemberViewKind::Field(field) => {
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Field,
                    path: container.extend(SemanticStep::new(SemanticStepKind::Field, field.id())),
                });
            }
            DurableMemberViewKind::Group(group) => {
                let path = container.extend(SemanticStep::new(SemanticStepKind::Group, group.id()));
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Group,
                    path: path.clone(),
                });
                stack.push((path, member.members()));
            }
            DurableMemberViewKind::Branch(branch) => {
                let path = container.extend(SemanticStep::new(
                    SemanticStepKind::Placement,
                    branch.placement(),
                ));
                nodes.push(SemanticNode {
                    kind: SemanticNodeKind::Branch,
                    path: path.clone(),
                });
                stack.push((path, member.members()));
            }
        }
    }
}

/// Append `u16_be(count) ‖ [u8(scalar_tag) ‖ IDREF(key)]*` — a placement's key
/// tuple, shared by roots and branches. Column order is load-bearing.
fn push_keys(out: &mut impl ImageByteSink, keys: &[KeyColumn]) -> Result<(), DurableGraphTooLarge> {
    push_u16(out, payload_count(keys.len())?);
    for key in keys {
        out.push(key.scalar.tag());
        push_idref(out, IDREF_KEY, &key.id);
    }
    Ok(())
}

/// Append a member run: `u16_be(count) ‖ member*`, each member a tag byte and its
/// body, descending through groups and branches so a whole durable shape has one
/// canonical byte image.
///
/// This is the walk that can amplify: a field's value is spelled as its expansion, so a
/// full sink ends the walk rather than expanding the members behind it into bytes nothing
/// will read. Once a sink is full every remaining member is skipped at every level, which
/// is what the single early return does.
///
/// The descent is an explicit stack of member runs, not recursion, for the same reason as
/// [`collect_member_nodes`]: nesting depth belongs to the rows, never to this walk's own
/// stack use.
fn push_members(
    out: &mut impl ImageByteSink,
    members: DurableMemberViews<'_>,
    values: &CanonicalValueShapeDag,
) -> Result<(), DurableGraphTooLarge> {
    push_u16(out, payload_count(members.len())?);
    let mut stack = vec![members];
    while let Some(run) = stack.last_mut() {
        let Some(member) = run.next() else {
            stack.pop();
            continue;
        };
        if out.is_full() {
            return Ok(());
        }
        match member.kind() {
            DurableMemberViewKind::Field(field) => {
                out.push(MEMBER_FIELD);
                push_idref(out, IDREF_FIELD, &field.id());
                out.push(u8::from(field.required()));
                expand(
                    values,
                    field.value(),
                    ValueShapeWireForm::ContractPayload,
                    out,
                )?;
            }
            DurableMemberViewKind::Group(group) => {
                out.push(MEMBER_GROUP);
                push_idref(out, IDREF_GROUP, &group.id());
                let inner = member.members();
                push_u16(out, payload_count(inner.len())?);
                stack.push(inner);
            }
            DurableMemberViewKind::Branch(branch) => {
                out.push(MEMBER_BRANCH);
                push_idref(out, IDREF_ROOT, &branch.placement());
                push_keys(out, branch.keys())?;
                let inner = member.members();
                push_u16(out, payload_count(inner.len())?);
                stack.push(inner);
            }
        }
    }
    Ok(())
}

/// The stable 32-byte identity of a program's durable graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurableContractId(pub(crate) [u8; 32]);

impl DurableContractId {
    /// The width of the identity on the wire. The measure core's DURABLE counting run
    /// counts these bytes without computing them, so the width has one owner and the
    /// value has another.
    pub(crate) const BYTES: usize = 32;

    /// Reconstruct an id from its 32 raw bytes. The verifier decodes the id carried
    /// in an untrusted image with this, then compares it against the id it recomputes
    /// from the decoded graph; it never treats the carried bytes as authoritative.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 identity bytes, as carried in the image DURABLE section.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The lowercase hex spelling of the identity, for diagnostics and tests.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }
}

/// Append `u8(kind) ‖ u64_be(16) ‖ id` — a kind-tagged, length-delimited ledger id.
fn push_idref(out: &mut impl ImageByteSink, kind: u8, id: &LedgerIdBytes) {
    out.push(kind);
    push_lp(out, id.bytes());
}

/// Append `u64_be(len) ‖ bytes`.
fn push_lp(out: &mut impl ImageByteSink, bytes: &[u8]) {
    out.extend_bytes(&(bytes.len() as u64).to_be_bytes());
    out.extend_bytes(bytes);
}

#[cfg(test)]
mod tests {
    use super::{
        DURABLE_CONTRACT_KIND, DurableContractId, DurableContractView, DurableIndexComponent,
        DurableIndexShape, LOCAL_ROOT_LINEAGE, LedgerIdBytes,
    };
    use crate::bounds;
    use crate::draft::{
        AdmittedGraphInputPlan, ImageDraft, KeyColumn, RecordTypeDef, RootOccurrenceDef, TypeId,
    };
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape, DeclarationNode};
    use crate::ty::Scalar;
    use crate::value_dag::{CanonicalValueShapeDag, ValueShapeNodeId, ValueShapeView};
    use sha2::{Digest, Sha256};

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// The construction budget these graphs are stated under.
    ///
    /// Every graph below is built through the draft's own flat entry points, which is the
    /// only path there is: a contract graph exists because an admitted plan let one be
    /// built, so a test states its ids and its member commands and never a member tree.
    fn plan() -> AdmittedGraphInputPlan {
        AdmittedGraphInputPlan::admit(
            bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
            bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
            bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
        )
        .expect("the image's own ceilings are admitted counts")
    }

    /// The contract identity of a graph these tests state. Every one of them is a handful
    /// of bytes, far inside the canonical payload ceiling, so the refusal that ceiling
    /// exists for is not what any of them is about.
    fn cid(view: DurableContractView<'_>) -> DurableContractId {
        view.contract_id()
            .expect("a stated test graph is far inside the payload ceiling")
    }

    /// One flat field command.
    fn field_cmd(
        parent: Option<u32>,
        byte: u8,
        required: bool,
        value: ValueShapeNodeId,
    ) -> DeclarationMemberDef {
        DeclarationMemberDef {
            parent,
            shape: DeclarationMemberShape::Field {
                id: id(byte),
                required,
                value,
            },
        }
    }

    fn group_cmd(parent: Option<u32>, byte: u8) -> DeclarationMemberDef {
        DeclarationMemberDef {
            parent,
            shape: DeclarationMemberShape::Group { id: id(byte) },
        }
    }

    fn branch_cmd(
        draft: &mut ImageDraft,
        parent: Option<u32>,
        byte: u8,
        keys: Vec<KeyColumn>,
    ) -> DeclarationMemberDef {
        let name = draft.intern_string("nested").expect("a within-domain mint");
        DeclarationMemberDef {
            parent,
            shape: DeclarationMemberShape::Branch {
                placement: id(byte),
                name,
                record: entry_record(draft),
                keys,
            },
        }
    }

    fn key(scalar: Scalar, byte: u8) -> KeyColumn {
        KeyColumn {
            scalar,
            id: id(byte),
        }
    }

    /// A materialized entry record for a stated graph.
    ///
    /// The record is surface, not identity — it is excluded from the contract preimage —
    /// so every graph below binds the same empty one and no hex moves with it.
    fn entry_record(draft: &mut ImageDraft) -> TypeId {
        let name = draft.intern_string("Entry").expect("a within-domain mint");
        draft
            .add_record_type(RecordTypeDef {
                name,
                fields: Vec::new(),
            })
            .expect("a within-domain mint")
    }

    /// State one Product and one root occurrence over it in a fresh draft.
    fn one_root(
        members: impl FnOnce(&mut ImageDraft) -> Vec<DeclarationMemberDef>,
        keys: Vec<KeyColumn>,
        indexes: Vec<DurableIndexShape>,
    ) -> ImageDraft {
        one_root_of_application(id(0x0a), members, keys, indexes)
    }

    fn one_root_of_application(
        application: LedgerIdBytes,
        members: impl FnOnce(&mut ImageDraft) -> Vec<DeclarationMemberDef>,
        keys: Vec<KeyColumn>,
        indexes: Vec<DurableIndexShape>,
    ) -> ImageDraft {
        let mut draft = ImageDraft::new();
        draft.set_application_identity(application);
        let record = entry_record(&mut draft);
        let commands = members(&mut draft);
        let name = draft.intern_string("root").expect("a within-domain mint");
        draft
            .declare_product(&plan(), id(0x0d), record, commands)
            .expect("a well-formed flat declaration");
        draft
            .add_root_occurrence(
                &plan(),
                id(0x0d),
                RootOccurrenceDef {
                    name,
                    keys,
                    placement: id(0x0b),
                    indexes: indexes.into(),
                },
            )
            .expect("the Product is declared");
        draft
    }

    /// The tracer's `counters` graph with fixed test ids: application `0x0a`,
    /// placement `0x0b`, key `0x0c`, product `0x0d`, fields `0x0e`/`0x0f`. A flat
    /// single-column-keyed resource: its member graph is two top-level fields.
    fn counters_graph() -> ImageDraft {
        one_root(
            |draft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                vec![
                    field_cmd(None, 0x0e, true, int),
                    field_cmd(None, 0x0f, false, text),
                ]
            },
            vec![key(Scalar::Text, 0x0c)],
            Vec::new(),
        )
    }

    /// A richer graph exercising every member kind: a top-level field, a static
    /// `group` namespace holding a field, and a keyed `branch` placement holding a
    /// field and its own nested group. This is the shape the branch/group slice
    /// makes identity-complete.
    fn library_graph() -> ImageDraft {
        one_root(
            |draft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                let instant = draft.value_shapes_mut().scalar(Scalar::Instant);
                let branch = branch_cmd(draft, None, 0x30, vec![key(Scalar::Text, 0x31)]);
                vec![
                    field_cmd(None, 0x0e, true, text),
                    group_cmd(None, 0x20),
                    field_cmd(Some(1), 0x21, false, int),
                    branch,
                    field_cmd(Some(3), 0x32, true, text),
                    group_cmd(Some(3), 0x33),
                    field_cmd(Some(5), 0x34, false, instant),
                ]
            },
            vec![key(Scalar::Int, 0x0c)],
            Vec::new(),
        )
    }

    #[test]
    fn kind_is_seventeen_bytes_and_distinct_from_the_other_kinds() {
        assert_eq!(DURABLE_CONTRACT_KIND.len(), 17);
        assert_ne!(
            DURABLE_CONTRACT_KIND.as_slice(),
            crate::digest::IMAGE_DIGEST_KIND.as_slice(),
        );
        assert_ne!(
            DURABLE_CONTRACT_KIND.as_slice(),
            crate::export_id::EXPORT_ID_KIND.as_slice(),
        );
    }

    /// Known-answer test for the frozen canonical payload of the tracer's `counters`
    /// graph over ledger ids. Freezing this hex pins the domain-separation,
    /// length-delimiting, IDREF kind tags, and member layout so a later reader
    /// can reconstruct it independently. If this value must change, the durable-contract
    /// identity has changed and every stored/derived id changes with it.
    #[test]
    fn durable_contract_id_known_answer() {
        let draft = counters_graph();
        assert_eq!(
            cid(draft.contract_view()).to_hex(),
            independent_id(draft.contract_view())
        );
        // The frozen value itself.
        assert_eq!(
            cid(draft.contract_view()).to_hex(),
            "db84b11b9d27fce7931f308596a8fcb20eb7e2c6bfbd5709c12148a54e41ee1f",
        );
    }

    /// Known-answer test for a graph with a group and a keyed branch: pins the
    /// member-tag bytes (field 0, group 1, branch 2), the `Group` IDREF tag (7), and
    /// the branch placement/key-tuple layout.
    #[test]
    fn durable_contract_id_with_group_and_branch_known_answer() {
        let library = library_graph();
        let counters = counters_graph();
        assert_eq!(
            cid(library.contract_view()).to_hex(),
            independent_id(library.contract_view())
        );
        assert_eq!(
            cid(library.contract_view()).to_hex(),
            "51aa42f995a8c81ece8146d417bc1f574680cb985a9024de72e81a1d47a3b714",
        );
        assert_ne!(cid(library.contract_view()), cid(counters.contract_view()));
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes. It shares no code with
    /// [`DurableContractView::write_graph`] and its hashing sink, so a change to the owner
    /// that silently altered the layout would diverge here.
    fn independent_id(view: DurableContractView<'_>) -> String {
        // Rebuild the graph bytes by hand from what the view publishes, sharing none of
        // the encoding code.
        fn idref(out: &mut Vec<u8>, kind: u8, id: &LedgerIdBytes) {
            out.push(kind);
            lp(out, id.bytes());
        }
        fn keys(out: &mut Vec<u8>, columns: &[KeyColumn]) {
            out.extend_from_slice(&(columns.len() as u16).to_be_bytes());
            for column in columns {
                out.push(column.scalar.tag());
                idref(out, 4, &column.id);
            }
        }
        fn value(out: &mut Vec<u8>, values: &CanonicalValueShapeDag, shape: ValueShapeNodeId) {
            match values.view(shape) {
                ValueShapeView::Scalar(scalar) => {
                    out.push(0);
                    out.push(scalar.tag());
                }
                ValueShapeView::Struct(leaves) => {
                    out.push(1);
                    out.extend_from_slice(&(leaves.len() as u16).to_be_bytes());
                    for leaf in leaves {
                        value(out, values, *leaf);
                    }
                }
                ValueShapeView::Enum { sum, members } => {
                    out.push(2);
                    idref(out, 5, &sum);
                    out.extend_from_slice(&(members.len() as u16).to_be_bytes());
                    for member in members {
                        idref(out, 6, &member.id());
                        out.extend_from_slice(&(member.payload().len() as u16).to_be_bytes());
                        for leaf in member.payload() {
                            value(out, values, *leaf);
                        }
                    }
                }
            }
        }
        fn member_run(
            out: &mut Vec<u8>,
            members: super::DurableMemberViews<'_>,
            values: &CanonicalValueShapeDag,
        ) {
            out.extend_from_slice(&(members.len() as u16).to_be_bytes());
            for member in members {
                match member.kind() {
                    super::DurableMemberViewKind::Field(f) => {
                        out.push(0);
                        idref(out, 2, &f.id());
                        out.push(u8::from(f.required()));
                        value(out, values, f.value());
                    }
                    super::DurableMemberViewKind::Group(g) => {
                        out.push(1);
                        idref(out, 7, &g.id());
                        member_run(out, member.members(), values);
                    }
                    super::DurableMemberViewKind::Branch(b) => {
                        out.push(2);
                        idref(out, 3, &b.placement());
                        keys(out, b.keys());
                        member_run(out, member.members(), values);
                    }
                }
            }
        }
        fn indexes(out: &mut Vec<u8>, list: &[DurableIndexShape]) {
            out.extend_from_slice(&(list.len() as u16).to_be_bytes());
            for index in list {
                idref(out, 8, &index.id);
                out.push(u8::from(index.unique));
                out.extend_from_slice(&(index.components.len() as u16).to_be_bytes());
                for component in &index.components {
                    match component {
                        DurableIndexComponent::Field(id) => idref(out, 2, id),
                        DurableIndexComponent::Key(id) => idref(out, 4, id),
                    }
                }
            }
        }
        let values = view.value_shapes();
        let mut graph: Vec<u8> = Vec::new();
        graph.extend_from_slice(&(view.roots().len() as u16).to_be_bytes());
        if let Some(application) = &view.application() {
            idref(&mut graph, 0, application);
        }
        for root in view.roots() {
            idref(&mut graph, 3, &root.placement().ledger_id());
            idref(&mut graph, 1, &root.product().ledger_id());
            keys(&mut graph, root.keys());
            member_run(&mut graph, root.members(), values);
            indexes(&mut graph, root.indexes());
        }
        let mut payload: Vec<u8> = Vec::new();
        lp(&mut payload, LOCAL_ROOT_LINEAGE);
        lp(&mut payload, &graph);
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(DURABLE_CONTRACT_KIND);
        framed.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        framed.extend_from_slice(&payload);
        let bytes: [u8; 32] = Sha256::digest(&framed).into();
        let mut hex = String::with_capacity(64);
        for byte in bytes {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }

    fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    /// The load-bearing D00 property: identity follows the ledger ids, not the
    /// spelling. A graph whose ids are unchanged keeps its contract id (a rename
    /// moves only the ledger anchor); a re-minted field id, a retyped key, or a
    /// flipped required flag changes it.
    #[test]
    fn identity_follows_ledger_ids_not_shape_spelling() {
        let counters = counters_graph();
        let base = cid(counters.contract_view());

        // The same ids and shape: stable (this is what a rename looks like here —
        // names are simply not part of the payload).
        assert_eq!(base, cid(counters_graph().contract_view()));

        let two_fields = |first_id: u8, first_required: bool, second: bool| {
            move |draft: &mut ImageDraft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                let mut members = vec![field_cmd(None, first_id, first_required, int)];
                if second {
                    members.push(field_cmd(None, 0x0f, false, text));
                }
                members
            }
        };
        let single_text_key = || vec![key(Scalar::Text, 0x0c)];

        // A re-minted top-level field id changes the id (delete-then-re-add mints fresh).
        let re_minted = one_root(two_fields(0x1e, true, true), single_text_key(), Vec::new());
        assert_ne!(base, cid(re_minted.contract_view()));

        // A changed key type changes the id.
        let rekeyed = one_root(
            two_fields(0x0e, true, true),
            vec![key(Scalar::Int, 0x0c)],
            Vec::new(),
        );
        assert_ne!(base, cid(rekeyed.contract_view()));

        // A re-minted key id changes the id.
        let rekey_id = one_root(
            two_fields(0x0e, true, true),
            vec![key(Scalar::Text, 0x2c)],
            Vec::new(),
        );
        assert_ne!(base, cid(rekey_id.contract_view()));

        // An added key column (single -> composite) changes the id.
        let composite = one_root(
            two_fields(0x0e, true, true),
            vec![key(Scalar::Text, 0x0c), key(Scalar::Int, 0x3c)],
            Vec::new(),
        );
        assert_ne!(base, cid(composite.contract_view()));

        // A field made required changes the id.
        let required = one_root(
            |draft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                vec![
                    field_cmd(None, 0x0e, true, int),
                    field_cmd(None, 0x0f, true, text),
                ]
            },
            single_text_key(),
            Vec::new(),
        );
        assert_ne!(base, cid(required.contract_view()));

        // A removed field changes the id.
        let narrowed = one_root(two_fields(0x0e, true, false), single_text_key(), Vec::new());
        assert_ne!(base, cid(narrowed.contract_view()));

        // A different application changes the id. (The slot is set-once-or-same, so
        // the divergent application is built fresh rather than overwritten.)
        let other_app = one_root_of_application(
            id(0x2a),
            two_fields(0x0e, true, true),
            single_text_key(),
            Vec::new(),
        );
        assert_ne!(base, cid(other_app.contract_view()));
    }

    /// Group and branch structure is part of the identity, distinct from a flat
    /// field of the same ledger id.
    #[test]
    fn group_and_branch_structure_is_part_of_the_identity() {
        let library = library_graph();
        let base = cid(library.contract_view());
        assert_eq!(base, cid(library_graph().contract_view()));

        /// The library graph with its group id, branch placement id, branch key tuple, and
        /// top-level member order all stated by the caller.
        fn variant(
            group: u8,
            placement: u8,
            branch_keys: Vec<KeyColumn>,
            swap_group_and_branch: bool,
        ) -> ImageDraft {
            one_root(
                move |draft| {
                    let int = draft.value_shapes_mut().scalar(Scalar::Int);
                    let text = draft.value_shapes_mut().scalar(Scalar::Text);
                    let instant = draft.value_shapes_mut().scalar(Scalar::Instant);
                    let (group_at, branch_at) = if swap_group_and_branch {
                        (2, 1)
                    } else {
                        (1, 2)
                    };
                    let branch = branch_cmd(draft, None, placement, branch_keys);
                    let mut members = vec![field_cmd(None, 0x0e, true, text)];
                    members.push(if swap_group_and_branch {
                        branch.clone()
                    } else {
                        group_cmd(None, group)
                    });
                    members.push(if swap_group_and_branch {
                        group_cmd(None, group)
                    } else {
                        branch
                    });
                    members.push(field_cmd(Some(group_at), 0x21, false, int));
                    members.push(field_cmd(Some(branch_at), 0x32, true, text));
                    members.push(group_cmd(Some(branch_at), 0x33));
                    members.push(field_cmd(Some(5), 0x34, false, instant));
                    members
                },
                vec![key(Scalar::Int, 0x0c)],
                Vec::new(),
            )
        }

        // Re-minting the group id changes the identity.
        let regrouped = variant(0x2f, 0x30, vec![key(Scalar::Text, 0x31)], false);
        assert_ne!(base, cid(regrouped.contract_view()));

        // Re-minting the branch placement id changes the identity.
        let rebranched = variant(0x20, 0x3f, vec![key(Scalar::Text, 0x31)], false);
        assert_ne!(base, cid(rebranched.contract_view()));

        // Adding a key column to the branch changes the identity.
        let wider = variant(
            0x20,
            0x30,
            vec![key(Scalar::Text, 0x31), key(Scalar::Int, 0x3d)],
            false,
        );
        assert_ne!(base, cid(wider.contract_view()));

        // Member order is load-bearing: swapping the group and the branch changes the
        // identity.
        let reordered = variant(0x20, 0x30, vec![key(Scalar::Text, 0x31)], true);
        assert_ne!(base, cid(reordered.contract_view()));

        // Promoting the group's field to a top-level field of the same id is a different
        // graph (nesting is load-bearing), even though the field id, scalar, and required
        // flag are unchanged.
        let flattened = one_root(
            |draft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                let instant = draft.value_shapes_mut().scalar(Scalar::Instant);
                let branch = branch_cmd(draft, None, 0x30, vec![key(Scalar::Text, 0x31)]);
                vec![
                    field_cmd(None, 0x0e, true, text),
                    field_cmd(None, 0x21, false, int),
                    branch,
                    field_cmd(Some(2), 0x32, true, text),
                    group_cmd(Some(2), 0x33),
                    field_cmd(Some(4), 0x34, false, instant),
                ]
            },
            vec![key(Scalar::Int, 0x0c)],
            Vec::new(),
        );
        assert_ne!(base, cid(flattened.contract_view()));
    }

    /// The widened value shapes one graph's fields reference, minted in one draft's arena.
    ///
    /// A value shape is an interned node, so a test states the shape it wants rather than
    /// editing one in place; the arena belongs to the draft that holds the graph, so no
    /// shape outlives the graph that references it.
    struct Shapes {
        int: ValueShapeNodeId,
        /// `struct { text, int }` and the same leaves in the other order.
        text_int: ValueShapeNodeId,
        int_text: ValueShapeNodeId,
        /// An `Option[int]`-shaped enum: `none` (empty) then `some` (int).
        option_int: ValueShapeNodeId,
        /// The same enum with a re-minted sum id.
        option_int_resummed: ValueShapeNodeId,
        /// A user enum with three members, the last carrying a text payload.
        user_enum: ValueShapeNodeId,
        /// The same enum with its first member's id re-minted.
        user_enum_re_membered: ValueShapeNodeId,
        /// The same enum with its first two members swapped.
        user_enum_reordered: ValueShapeNodeId,
        /// The same enum with its payload leaf retyped to int.
        user_enum_retyped: ValueShapeNodeId,
    }

    fn mint_shapes(draft: &mut ImageDraft) -> Shapes {
        let values = draft.value_shapes_mut();
        let int = values.scalar(Scalar::Int);
        let text = values.scalar(Scalar::Text);
        let text_int = values.struct_shape(vec![text, int]);
        let int_text = values.struct_shape(vec![int, text]);
        let option_members = || vec![(id(0x51), Vec::new()), (id(0x52), vec![int])];
        let option_int = values.enum_shape(id(0x50), option_members());
        let option_int_resummed = values.enum_shape(id(0x60), option_members());
        let user = |first: LedgerIdBytes, second: LedgerIdBytes, payload: ValueShapeNodeId| {
            vec![
                (first, Vec::new()),
                (second, Vec::new()),
                (id(0x56), vec![payload]),
            ]
        };
        let user_enum = values.enum_shape(id(0x53), user(id(0x54), id(0x55), text));
        let user_enum_re_membered = values.enum_shape(id(0x53), user(id(0x61), id(0x55), text));
        let user_enum_reordered = values.enum_shape(id(0x53), user(id(0x55), id(0x54), text));
        let user_enum_retyped = values.enum_shape(id(0x53), user(id(0x54), id(0x55), int));
        Shapes {
            int,
            text_int,
            int_text,
            option_int,
            option_int_resummed,
            user_enum,
            user_enum_re_membered,
            user_enum_reordered,
            user_enum_retyped,
        }
    }

    /// A graph whose resource stores widened value shapes: a dense `struct` leaf, an
    /// `Option`-shaped enum, and a user enum. Enum members carry sum (kind 5) and
    /// member (kind 6) ids; the struct records its leaves positionally with no
    /// per-leaf id. `pick` states the three widened shapes, so a variant is a fresh
    /// graph rather than an edited one.
    fn widened_graph_with(pick: impl FnOnce(&Shapes) -> [ValueShapeNodeId; 3]) -> ImageDraft {
        one_root(
            move |draft| {
                let shapes = mint_shapes(draft);
                let [first, second, third] = pick(&shapes);
                vec![
                    field_cmd(None, 0x0e, true, shapes.int),
                    field_cmd(None, 0x40, false, first),
                    field_cmd(None, 0x41, true, second),
                    field_cmd(None, 0x42, false, third),
                ]
            },
            vec![key(Scalar::Text, 0x0c)],
            Vec::new(),
        )
    }

    fn widened_graph() -> ImageDraft {
        widened_graph_with(|s| [s.text_int, s.option_int, s.user_enum])
    }

    /// Known-answer test for a durable graph with widened value shapes. Freezing
    /// this hex pins the value-shape tag bytes (scalar 0, struct 1, enum 2), the sum
    /// (5) and member (6) IDREF tags, and the payload layout.
    #[test]
    fn durable_contract_id_with_widened_values_known_answer() {
        let widened = widened_graph();
        assert_eq!(
            cid(widened.contract_view()).to_hex(),
            independent_id(widened.contract_view())
        );
        assert_eq!(
            cid(widened.contract_view()).to_hex(),
            "9e2b73b8c9e5f26ca656ed4c05b0468683b36b2753a483da8f960fc94cbd045e",
        );
        assert_ne!(
            cid(widened.contract_view()),
            cid(counters_graph().contract_view())
        );
    }

    /// Enum member identity is part of the durable identity: a rename preserves it
    /// (ids unchanged), while re-minting a member, reordering members (append is
    /// positional), or re-typing a member payload changes it.
    #[test]
    fn enum_member_identity_follows_the_ledger_ids() {
        let widened = widened_graph();
        let base = cid(widened.contract_view());
        assert_eq!(base, cid(widened_graph().contract_view()));

        // Re-minting the sum id (a delete-then-re-add of the enum) changes the id.
        let re_summed = widened_graph_with(|s| [s.text_int, s.option_int_resummed, s.user_enum]);
        assert_ne!(base, cid(re_summed.contract_view()));

        // Re-minting one member id (delete-then-re-add of a variant) changes the id.
        let re_membered =
            widened_graph_with(|s| [s.text_int, s.option_int, s.user_enum_re_membered]);
        assert_ne!(base, cid(re_membered.contract_view()));

        // Appending a member is positional: swapping two members changes the id, so a
        // member can never silently take another's code.
        let reordered = widened_graph_with(|s| [s.text_int, s.option_int, s.user_enum_reordered]);
        assert_ne!(base, cid(reordered.contract_view()));

        // Re-typing a member payload leaf changes the id.
        let retyped = widened_graph_with(|s| [s.text_int, s.option_int, s.user_enum_retyped]);
        assert_ne!(base, cid(retyped.contract_view()));

        // Re-ordering a struct leaf changes the id (leaf order is load-bearing).
        let struct_swapped = widened_graph_with(|s| [s.int_text, s.option_int, s.user_enum]);
        assert_ne!(base, cid(struct_swapped.contract_view()));
    }

    /// A singleton root (empty key tuple) and a composite root (two key columns)
    /// are ordinary shapes under the length-prefixed key encoding: each agrees with
    /// the independent decoder and is distinct from the single-key graph.
    #[test]
    fn singleton_and_composite_roots_encode_and_reconstruct() {
        let singleton = one_root(
            |draft| {
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                vec![field_cmd(None, 0x0e, true, text)]
            },
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            cid(singleton.contract_view()).to_hex(),
            independent_id(singleton.contract_view())
        );

        let composite_keys = || vec![key(Scalar::Text, 0x0c), key(Scalar::Int, 0x1c)];
        let composite = one_root(|_| Vec::new(), composite_keys(), Vec::new());
        assert_eq!(
            cid(composite.contract_view()).to_hex(),
            independent_id(composite.contract_view())
        );
        assert_ne!(
            cid(singleton.contract_view()),
            cid(composite.contract_view())
        );

        // Key-column order matters: swapping the two columns is a different graph.
        let mut swapped_keys = composite_keys();
        swapped_keys.swap(0, 1);
        let swapped = one_root(|_| Vec::new(), swapped_keys, Vec::new());
        assert_ne!(cid(composite.contract_view()), cid(swapped.contract_view()));
    }

    /// The `counters` graph plus two managed indexes: a nonunique `byLabel(label, name)`
    /// projecting the `label` field then the `name` key, and a unique `byValue(value)`
    /// projecting the `value` field. Fixed index ids `0x70`/`0x71`.
    fn indexed_graph_with(indexes: Vec<DurableIndexShape>) -> ImageDraft {
        one_root(
            |draft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                vec![
                    field_cmd(None, 0x0e, true, int),
                    field_cmd(None, 0x0f, false, text),
                ]
            },
            vec![key(Scalar::Text, 0x0c)],
            indexes,
        )
    }

    fn declared_indexes() -> Vec<DurableIndexShape> {
        vec![
            DurableIndexShape {
                id: id(0x70),
                unique: false,
                components: vec![
                    DurableIndexComponent::Field(id(0x0f)),
                    DurableIndexComponent::Key(id(0x0c)),
                ],
            },
            DurableIndexShape {
                id: id(0x71),
                unique: true,
                components: vec![DurableIndexComponent::Field(id(0x0e))],
            },
        ]
    }

    fn indexed_graph() -> ImageDraft {
        indexed_graph_with(declared_indexes())
    }

    /// Known-answer test for a durable graph carrying managed indexes: pins the
    /// `Index` IDREF tag (8), the `unique` flag byte, the projection encoding, and the
    /// per-root `u16(index_count)` that follows every root's member run.
    #[test]
    fn durable_contract_id_with_indexes_known_answer() {
        let indexed = indexed_graph();
        assert_eq!(
            cid(indexed.contract_view()).to_hex(),
            independent_id(indexed.contract_view())
        );
        assert_eq!(
            cid(indexed.contract_view()).to_hex(),
            "8965d84ad46e7cf0f31de46f0c3d45f2fb8fa555ab1ed99e30a76e694763e773",
        );
        // Indexes are part of the identity: dropping them is the plain counters graph.
        assert_ne!(
            cid(indexed.contract_view()),
            cid(counters_graph().contract_view())
        );
    }

    /// Managed-index identity follows the ledger ids: a rename preserves it (ids
    /// unchanged), while re-minting an index id, flipping `unique`, reordering
    /// components, adding a component, or reordering two indexes changes it.
    #[test]
    fn index_identity_follows_the_ledger_ids() {
        let indexed = indexed_graph();
        let base = cid(indexed.contract_view());
        assert_eq!(base, cid(indexed_graph().contract_view()));

        let varied = |mutate: fn(&mut Vec<DurableIndexShape>)| {
            let mut indexes = declared_indexes();
            mutate(&mut indexes);
            indexed_graph_with(indexes)
        };

        // Re-minting an index id (delete-then-re-add of the index) changes the id.
        let re_minted = varied(|indexes| indexes[0].id = id(0x7f));
        assert_ne!(base, cid(re_minted.contract_view()));

        // Flipping the unique flag changes the id.
        let uniqued = varied(|indexes| indexes[0].unique = true);
        assert_ne!(base, cid(uniqued.contract_view()));

        // Reordering the projection components changes the id (projection order is
        // load-bearing).
        let reordered = varied(|indexes| indexes[0].components.swap(0, 1));
        assert_ne!(base, cid(reordered.contract_view()));

        // Re-pointing a component at a different leaf changes the id.
        let re_pointed =
            varied(|indexes| indexes[1].components[0] = DurableIndexComponent::Field(id(0x0f)));
        assert_ne!(base, cid(re_pointed.contract_view()));

        // A field component and a key component of the same id are distinct.
        let field_vs_key =
            varied(|indexes| indexes[1].components[0] = DurableIndexComponent::Key(id(0x0e)));
        assert_ne!(base, cid(field_vs_key.contract_view()));

        // Index declaration order is load-bearing.
        let swapped = varied(|indexes| indexes.swap(0, 1));
        assert_ne!(base, cid(swapped.contract_view()));
    }

    /// Asking a stated graph for its identity is bounded work, whoever states it.
    ///
    /// This is the whole amplification path, and it survives unconstructibility: an arena
    /// is public and interning is what makes it compact, so seventeen minted nodes
    /// describe a value whose expansion — which is what the canonical payload spells — has
    /// `4^16` scalar leaves. The bounded flat builder that admits the *member* graph says
    /// nothing about how wide a value shape expands. The identity owner's own ceiling
    /// answers, in the bytes it admits rather than the bytes the expansion would have
    /// produced.
    ///
    /// That this test *returns* is the evidence: without the ceiling the same call walks
    /// the expansion, and no wall-clock budget is needed to tell the two apart.
    #[test]
    fn a_stated_graph_whose_payload_is_unbounded_is_refused_rather_than_expanded() {
        let forged = one_root(
            |draft| {
                let values = draft.value_shapes_mut();
                let mut level = values.scalar(Scalar::Int);
                for _ in 0..16 {
                    level = values.struct_shape(vec![level; 4]);
                }
                assert_eq!(values.len(), 17, "the stated graph is seventeen nodes");
                vec![field_cmd(None, 0x0e, true, level)]
            },
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(
            forged.contract_view().contract_id(),
            Err(super::DurableGraphTooLarge)
        );
    }

    /// The same refusal for the other count the walk spells: a value shape's own arity.
    ///
    /// A struct node's leaf count is written by the value-shape expansion owner rather
    /// than by this module, and its expansion is small enough that the payload ceiling
    /// never answers for it. The refusal is the arity's own.
    ///
    /// Its sibling — a *member* count the payload's `u16` cannot spell — is no longer
    /// reachable from any graph a caller can state: a declaration holds at most
    /// [`bounds::MAX_DURABLE_MEMBERS`] rows and a graph at most [`bounds::MAX_ROOTS`]
    /// occurrences, both far below `u16::MAX`. The arity check on those counts stays as
    /// the codec's own defense, and unconstructibility is why nothing can reach it.
    #[test]
    fn a_stated_value_shape_whose_arity_the_payload_cannot_spell_is_refused() {
        let forged = one_root(
            |draft| {
                let values = draft.value_shapes_mut();
                let int = values.scalar(Scalar::Int);
                let wide = values.struct_shape(vec![int; u16::MAX as usize + 1]);
                vec![field_cmd(None, 0x0e, true, wide)]
            },
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(
            forged.contract_view().contract_id(),
            Err(super::DurableGraphTooLarge)
        );
    }

    /// The sibling of the refusal: the same arena, referenced at a level whose expansion
    /// fits, still mints an identity. The ceiling refuses a payload, not an arena.
    #[test]
    fn a_shared_shape_whose_expansion_fits_still_mints_an_identity() {
        let fitting = one_root(
            |draft| {
                let values = draft.value_shapes_mut();
                let mut level = values.scalar(Scalar::Int);
                for _ in 0..7 {
                    level = values.struct_shape(vec![level; 4]);
                }
                vec![field_cmd(None, 0x0e, true, level)]
            },
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            cid(fitting.contract_view()).to_hex(),
            independent_id(fitting.contract_view())
        );
    }

    /// The fitting-preimage bound is the 25:16 ratio applied to a measurement-admitted body, and
    /// the subtraction of the closing identity sits **inside** the ratio.
    ///
    /// The two are separable: the identity's 32 bytes are ceiling headroom measurement
    /// reserves before the body is allocated, not body bytes, so they are never amplified
    /// by the reference-spelling ratio. Subtracting outside would state a bound 50 bytes
    /// wider than any measurement-admitted body can produce, and so would stop being the tight
    /// bound the row derives.
    #[test]
    fn the_fitting_preimage_bound_is_the_ratio_of_a_measurement_admitted_body() {
        let inside = (crate::bounds::MAX_IMAGE_BYTES - DurableContractId::BYTES) * 25 / 16;
        let outside = crate::bounds::MAX_IMAGE_BYTES * 25 / 16 - DurableContractId::BYTES;
        assert_eq!(super::MAX_FITTING_CONTRACT_PREIMAGE_BYTES, inside);
        assert_eq!(inside, 819_150, "the derived value at the current ceiling");
        assert!(
            inside < outside,
            "subtracting outside the ratio is the looser reading and is not what the \
             measured plan admits",
        );
        // The ratio's own terms, so a respelled reference fails here rather than silently
        // widening the bound.
        assert_eq!(super::PREIMAGE_IDREF_BYTES, 25);
        assert_eq!(super::LEDGER_ID_BYTES, 16);
    }

    /// The length the hash is framed with is the length the walk writes.
    ///
    /// The payload is never materialized, so its `LP` header is written before its bytes
    /// exist. A counted length that disagreed with the streamed one would frame every
    /// identity over a payload length no walk produces, and the frozen hexes would move
    /// together rather than diverge — the pinned values are the only witness that catches
    /// it, and this is the direct statement of the property they stand on.
    #[test]
    fn the_counted_preimage_length_is_the_length_the_walk_writes() {
        for draft in [
            counters_graph(),
            library_graph(),
            widened_graph(),
            indexed_graph(),
            ImageDraft::new(),
        ] {
            let graph = draft.contract_view();
            let mut counted = super::FittingPreimageLength::default();
            graph
                .write_graph(&mut counted)
                .expect("a stated test graph");
            let mut written: Vec<u8> = Vec::new();
            graph
                .write_graph(&mut written)
                .expect("a stated test graph");
            assert_eq!(counted.0, written.len());
        }
    }

    /// The saturating sentinel fires at the first byte past the bound, and only there.
    ///
    /// The boundary is stated in the graph's own member costs rather than in a copied
    /// number, so it moves with the bound: a keyless root frames 83 payload bytes; a field
    /// carrying a bare scalar costs 29 and one carrying a struct of `leaves` costs
    /// `30 + 2 * leaves`; a static `group` with no members costs 28. Swapping that group
    /// for one more scalar field is therefore exactly one byte, which is the only reason
    /// the pair below can straddle the bound rather than step over it.
    #[test]
    fn a_preimage_one_byte_past_the_fitting_bound_is_refused_and_the_one_below_mints() {
        const ROOT_FRAME_BYTES: usize = 83;
        const SCALAR_FIELD_BYTES: usize = 29;
        const EMPTY_GROUP_BYTES: usize = 28;
        const STRUCT_FIELD_FRAME_BYTES: usize = 30;
        // The widest struct arity the payload's `u16` leaf count can spell.
        const WIDE_LEAVES: usize = u16::MAX as usize;
        const WIDE_FIELDS: usize = 6;
        let wide_field_bytes = STRUCT_FIELD_FRAME_BYTES + 2 * WIDE_LEAVES;
        // What is left for the one tuning field once the frame, the wide fields, the
        // scalar field, and the group have been spent.
        let tuning = super::MAX_FITTING_CONTRACT_PREIMAGE_BYTES
            - ROOT_FRAME_BYTES
            - WIDE_FIELDS * wide_field_bytes
            - SCALAR_FIELD_BYTES
            - EMPTY_GROUP_BYTES
            - STRUCT_FIELD_FRAME_BYTES;
        assert_eq!(tuning % 2, 0, "a struct leaf is two payload bytes");
        let tuning_leaves = tuning / 2;

        // `extra` is the member that straddles the bound: a group is 28 bytes and a scalar
        // field is 29, so the two graphs differ by exactly one payload byte.
        let graph = |extra_is_a_field: bool| {
            one_root(
                move |draft| {
                    let values = draft.value_shapes_mut();
                    let int = values.scalar(Scalar::Int);
                    let wide = values.struct_shape(vec![int; WIDE_LEAVES]);
                    let tuned = values.struct_shape(vec![int; tuning_leaves]);
                    let mut members: Vec<_> = (0..WIDE_FIELDS)
                        .map(|_| field_cmd(None, 0x0e, true, wide))
                        .collect();
                    members.push(field_cmd(None, 0x0e, true, tuned));
                    members.push(field_cmd(None, 0x0e, true, int));
                    members.push(if extra_is_a_field {
                        field_cmd(None, 0x0e, true, int)
                    } else {
                        group_cmd(None, 0x20)
                    });
                    members
                },
                Vec::new(),
                Vec::new(),
            )
        };

        let fitting = graph(false);
        let mut counted = super::FittingPreimageLength::default();
        fitting
            .contract_view()
            .write_graph(&mut counted)
            .expect("the fitting graph");
        assert_eq!(
            counted.0,
            super::MAX_FITTING_CONTRACT_PREIMAGE_BYTES,
            "the fitting graph lands exactly on the bound",
        );
        assert!(
            fitting.contract_view().contract_id().is_ok(),
            "N mints an identity",
        );

        assert_eq!(
            graph(true).contract_view().contract_id(),
            Err(super::DurableGraphTooLarge),
            "N+1 saturates the sentinel",
        );
    }

    #[test]
    fn the_empty_graph_has_a_stable_id() {
        let empty = ImageDraft::new();
        assert_eq!(cid(empty.contract_view()), cid(empty.contract_view()));
        assert_ne!(
            cid(empty.contract_view()),
            cid(counters_graph().contract_view())
        );
    }

    /// One Product's member graph is stored once however many roots occur over it.
    ///
    /// This is what the per-occurrence member-tree allocation cost, and what the view
    /// spine replaces: the draft holds one declaration row and one flat occurrence row per
    /// root, and every occurrence's members resolve to the *same* rows, so the identity of
    /// the shared allocation — not merely equal spelling — is the evidence.
    #[test]
    fn many_roots_over_one_product_share_one_member_graph() {
        // The admitted maximum: exactly one occurrence past `MAX_ROOTS`, which is the
        // count the nonblocking `Roots` aggregate must still publish a complete graph for.
        const ROOTS: usize = bounds::MAX_ADMITTED_ROOT_OCCURRENCES;
        let mut draft = ImageDraft::new();
        draft.set_application_identity(id(0x0a));
        let record = entry_record(&mut draft);
        let int = draft.value_shapes_mut().scalar(Scalar::Int);
        let name = draft.intern_string("root").expect("a within-domain mint");
        draft
            .declare_product(
                &plan(),
                id(0x0d),
                record,
                vec![field_cmd(None, 0x0e, true, int)],
            )
            .expect("a well-formed flat declaration");
        for root in 0..ROOTS {
            let mut placement = [0u8; 16];
            placement[..8].copy_from_slice(&(root as u64).to_be_bytes());
            draft
                .add_root_occurrence(
                    &plan(),
                    id(0x0d),
                    RootOccurrenceDef {
                        name,
                        keys: vec![key(Scalar::Int, 0x0c)],
                        placement: LedgerIdBytes::from_bytes(placement),
                        indexes: Vec::new().into(),
                    },
                )
                .expect("the Product is declared");
        }

        let view = draft.contract_view();
        assert_eq!(view.roots().len(), ROOTS);
        // Every occurrence resolves to the *same* member row, by address: the declaration
        // graph the payload is written from was never copied per root. This module is a
        // child of the view's own, so it reads the borrowed row the view carries rather
        // than inferring sharing from equal spelling.
        let mut distinct: Vec<*const DeclarationNode> = Vec::new();
        for root in view.roots() {
            let mut members = root.members();
            let row = members.next().expect("one member");
            assert!(members.next().is_none(), "one member");
            let address = std::ptr::from_ref(row.node);
            if !distinct.contains(&address) {
                distinct.push(address);
            }
        }
        assert_eq!(
            distinct.len(),
            1,
            "all {ROOTS} occurrences read one Product's member rows",
        );
    }

    // --- Derived semantic paths (D02): every graph node's stable ledger-id chain. ---

    use crate::semantic::{SemanticNodeKind, SemanticStepKind};

    /// The `(node kind, step kinds, step ids)` fingerprint of every semantic node in
    /// pre-order, for exact structural assertions.
    fn node_shapes(
        view: DurableContractView<'_>,
    ) -> Vec<(SemanticNodeKind, Vec<SemanticStepKind>, Vec<[u8; 16]>)> {
        view.semantic_nodes()
            .into_iter()
            .map(|node| {
                let kinds = node.path.steps().iter().map(|s| s.kind).collect();
                let ids = node.path.steps().iter().map(|s| *s.id.bytes()).collect();
                (node.kind, kinds, ids)
            })
            .collect()
    }

    #[test]
    fn semantic_nodes_of_a_flat_root_are_the_root_and_its_fields() {
        use SemanticNodeKind::{Field, Root};
        use SemanticStepKind::{Application, Field as FieldStep, Placement};
        assert_eq!(
            node_shapes(counters_graph().contract_view()),
            vec![
                (
                    Root,
                    vec![Application, Placement],
                    vec![[0x0a; 16], [0x0b; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x0e; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x0f; 16]]
                ),
            ]
        );
    }

    #[test]
    fn semantic_nodes_cover_every_group_and_branch_node_in_pre_order() {
        use SemanticNodeKind::{Branch, Field, Group, Root};
        use SemanticStepKind::{Application, Field as FieldStep, Group as GroupStep, Placement};
        assert_eq!(
            node_shapes(library_graph().contract_view()),
            vec![
                (
                    Root,
                    vec![Application, Placement],
                    vec![[0x0a; 16], [0x0b; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x0e; 16]]
                ),
                (
                    Group,
                    vec![Application, Placement, GroupStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x20; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, GroupStep, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x20; 16], [0x21; 16]]
                ),
                // A branch step is a Placement, like a root — a keyed node.
                (
                    Branch,
                    vec![Application, Placement, Placement],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, Placement, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16], [0x32; 16]]
                ),
                (
                    Group,
                    vec![Application, Placement, Placement, GroupStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16], [0x33; 16]]
                ),
                (
                    Field,
                    vec![Application, Placement, Placement, GroupStep, FieldStep],
                    vec![[0x0a; 16], [0x0b; 16], [0x30; 16], [0x33; 16], [0x34; 16]]
                ),
            ]
        );
    }

    #[test]
    fn a_field_path_is_distinct_from_and_extends_its_container() {
        let library = library_graph();
        let nodes = library.contract_view().semantic_nodes();
        let group = nodes
            .iter()
            .find(|n| n.path.node_id() == id(0x20))
            .expect("the group node");
        let nested_field = nodes
            .iter()
            .find(|n| n.path.node_id() == id(0x21))
            .expect("the group-nested field node");
        assert_ne!(group.path, nested_field.path);
        // The field's path is exactly the group's path plus the field step.
        assert!(nested_field.path.steps().starts_with(group.path.steps()));
        assert_eq!(
            nested_field.path.steps().len(),
            group.path.steps().len() + 1
        );
    }

    #[test]
    fn re_minting_a_node_id_moves_only_paths_through_it() {
        let library = library_graph();
        let base = library.contract_view().semantic_nodes();

        // Re-mint the group id: the group node and its nested field node move to the
        // fresh id; every other node's path is untouched.
        let regrouped = one_root(
            |draft| {
                let int = draft.value_shapes_mut().scalar(Scalar::Int);
                let text = draft.value_shapes_mut().scalar(Scalar::Text);
                let instant = draft.value_shapes_mut().scalar(Scalar::Instant);
                let branch = branch_cmd(draft, None, 0x30, vec![key(Scalar::Text, 0x31)]);
                vec![
                    field_cmd(None, 0x0e, true, text),
                    group_cmd(None, 0x2f),
                    field_cmd(Some(1), 0x21, false, int),
                    branch,
                    field_cmd(Some(3), 0x32, true, text),
                    group_cmd(Some(3), 0x33),
                    field_cmd(Some(5), 0x34, false, instant),
                ]
            },
            vec![key(Scalar::Int, 0x0c)],
            Vec::new(),
        );
        let after = regrouped.contract_view().semantic_nodes();

        // The root and the top-level field keep identical paths.
        for terminal in [id(0x0b), id(0x0e), id(0x30)] {
            let before_path = base.iter().find(|n| n.path.node_id() == terminal);
            let after_path = after.iter().find(|n| n.path.node_id() == terminal);
            assert_eq!(
                before_path.map(|n| &n.path),
                after_path.map(|n| &n.path),
                "the node ending in {terminal:?} is unaffected by re-minting the group",
            );
        }
        // The group's own node now ends in the fresh id, and the old id is gone.
        assert!(after.iter().any(|n| n.path.node_id() == id(0x2f)));
        assert!(!after.iter().any(|n| n.path.node_id() == id(0x20)));
        // Its nested field's path now passes through the fresh group id.
        let nested = after
            .iter()
            .find(|n| n.path.node_id() == id(0x21))
            .expect("the nested field node");
        assert!(nested.path.steps().iter().any(|s| s.id == id(0x2f)));
    }

    #[test]
    fn the_empty_graph_has_no_semantic_nodes() {
        assert!(
            ImageDraft::new()
                .contract_view()
                .semantic_nodes()
                .is_empty()
        );
    }
}
