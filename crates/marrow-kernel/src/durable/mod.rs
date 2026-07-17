//! The typed durable runtime the VM drives (design §G).
//!
//! The kernel sits below the language. It consumes verified sites and typed
//! scalars — never source — and turns durable operations into ordered-byte engine
//! calls through the narrow [`marrow_store::ByteEngine`] seam. It owns the durable
//! operation algebra outcomes, the authority triple, the store profile, the
//! name-keyed physical layout, and the commit witness.
//!
//! The kernel provides the flat read/write kernel and the ephemeral-memory
//! attachment: a fresh in-memory store minted from a verified image's schema,
//! sites, and deployment ceiling, driving read and single-write sessions bounded
//! by `demand ∩ ceiling ∩ grant`. The executable physical layout is the
//! name-keyed root — its fields each a scalar or a widened value (`struct`/`enum`/
//! `Option`, framed inline) — plus keyed branches nested to any depth; groups,
//! composite-keyed branches, nominal-typed fields, and composite root keys stay
//! parked until their owners land them.

mod attach;
mod physical;
mod plan;
mod profile;
mod store;

pub use attach::{
    AttachError, AttachmentId, CeilingIdToken, DeploymentCeiling, EphemeralAttachment,
};
pub use store::{Durable, DurableStore, ReadSession, TxnSession};

use std::num::NonZeroU32;

use marrow_store::StoreError;

use crate::codec::key::KeyScalar;
use crate::codec::value::{ScalarKind, ValueShape};
use crate::equality::ValueDomain;

/// The schema descriptor the store profile records and every session revalidates.
/// One root; its top-level fields and its keyed branches in declaration (image)
/// order. A branch is a keyed subtree nested beneath every root entry; the schema is
/// recursive, so scalar-field branches with one or more key columns nest to any depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSchema {
    pub root_name: String,
    /// The root's ordered key columns (one scalar kind per column), the whole composite
    /// key. A single-column root is the one-element case.
    pub key: Vec<ScalarKind>,
    pub fields: Vec<FieldSchema>,
    pub branches: Vec<BranchSchema>,
    /// The root's compiler-maintained managed indexes, in stable declaration order — the
    /// order every maintenance pass visits them so a whole-entry write's index writes are
    /// deterministic. Empty for a root that declares none.
    pub indexes: Vec<IndexSchema>,
}

/// One managed index the kernel maintains over a keyed root: its stable durable identity
/// (the physical cell discriminator that separates one index's cells from another's under
/// the same root, and survives a rename), its `unique` flag, and its ordered projection
/// resolved to record/key positions. The kernel stores the identity as raw bytes and
/// stays free of any image dependency; the executor derives it from a verified
/// [`SealedIndex`](marrow_verify::SealedIndex). An index stores no data of its own and has
/// no application write path — maintenance is a consequence of the source write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSchema {
    /// The index's stable 16-byte identity, the physical discriminator of its cell family.
    pub id: [u8; 16],
    /// Whether a complete-key lookup yields at most one source key (a unique index) rather
    /// than an ordered non-unique index whose rows carry the identity suffix.
    pub unique: bool,
    /// The ordered projection: each component names a root key column or a top-level field
    /// by position. A non-unique index's projection ends with the identity key columns (the
    /// row-distinguishing suffix); a unique index's may omit them.
    pub projection: Vec<IndexComponent>,
}

/// One component of a managed index's ordered projection, naming a durable-key leaf of the
/// root by position: an identity key column or a top-level field. The physical projection
/// of a verified [`SealedIndexComponent`](marrow_verify::SealedIndexComponent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexComponent {
    /// An identity key column, by its index into the root's key tuple.
    Key(u16),
    /// A top-level field, by its index into the root's materialized record.
    Field(u16),
}

/// One field of a node's record: its name, value shape, and required flag. A field's
/// shape is the closed storable durable value set — a scalar, a dense product
/// (`struct`/record), or a closed sum (`enum`/`Option`/`Result`). The value codec
/// (`codec::value`) frames a composite within the one field-leaf cell; a scalar field
/// stays byte-identical to the pre-widening form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub shape: ValueShape,
    pub required: bool,
}

impl FieldSchema {
    /// A scalar-shaped field. Bounds the churn of the value-shape widening: a call site
    /// that knows a field is scalar names its kind rather than wrapping a [`ValueShape`].
    pub fn scalar(name: impl Into<String>, kind: ScalarKind, required: bool) -> Self {
        Self {
            name: name.into(),
            shape: ValueShape::Scalar(kind),
            required,
        }
    }
}

/// One keyed branch nested beneath a parent entry: its name, its single key column's
/// scalar kind, its own record's fields, and its own nested branches. A branch entry is
/// addressed by extending the parent's key-path with the branch key and carries its own
/// marker and field leaves, so it is a distinct durable node reusing the parent entry's
/// marker/field topology one level down. The schema is recursive — a branch may itself
/// declare keyed branches — so the store profile describes a whole nested branch shape
/// and a sub-branch shape change is a profile mismatch at session open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSchema {
    pub name: String,
    /// The branch's ordered key columns (one scalar kind per column). A single-column
    /// branch is the one-element case; a branch entry's key-path extends its parent's
    /// with this whole tuple.
    pub key: Vec<ScalarKind>,
    pub fields: Vec<FieldSchema>,
    pub branches: Vec<BranchSchema>,
}

/// A verified operation site the kernel maps to physical layout, indexed by the
/// image's site index. Its root is the single T01 root; the target is the sealed
/// [`SemanticTarget`](marrow_verify::SemanticTarget) projected to the physical flat
/// root — the whole payload or one of the root's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSpec {
    pub target: SiteTarget,
}

/// The closed operation-target set the kernel serves: the root's whole keyed payload,
/// one of the root's field leaves (by field index into [`StoreSchema::fields`]), or a
/// keyed branch entry's whole payload or one of its field leaves. The kernel owns the
/// mapping from this sealed semantic target to the name-keyed physical layout (see
/// `physical`); it is the physical projection of the verifier's closed `SemanticTarget`.
///
/// A branch target names its node by a *branch path*: the per-level branch indices from
/// the root down to the addressed branch node, each an index into that level's
/// declaration-ordered branch list (the root's [`StoreSchema::branches`], then each
/// branch's [`BranchSchema::branches`]). A single-element path names a direct child
/// branch; a longer path names a nested branch one level deeper per element. The node's
/// key-path is the root key followed by one key per path element, so a path of length
/// `d` addresses a `(1 + d)`-element key-path `d` levels below the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteTarget {
    WholePayload,
    FieldLeaf(u16),
    /// The whole payload of a keyed branch entry, named by its branch path (per-level
    /// branch indices from the root down).
    BranchEntry(Box<[u16]>),
    /// One field leaf of a keyed branch entry: the branch node's branch path and the
    /// field's index into that branch's [`BranchSchema::fields`]. Its field-exact
    /// operations address the `(1 + path.len())`-element key-path
    /// `[root_key, branch_key, …]` one or more levels below the root.
    BranchField {
        branch: Box<[u16]>,
        field: u16,
    },
    /// A managed index's nonunique progressive-prefix scan read, named by its position
    /// in [`StoreSchema::indexes`]. The read is bounded and yields the next distinct
    /// projected component; it observes only the index cell family and never a source
    /// entry.
    IndexScan(u16),
    /// A managed index's unique complete-key lookup read, named by its position in
    /// [`StoreSchema::indexes`]. The read is a single exact probe yielding the one
    /// matching source key tuple or absent.
    IndexLookup(u16),
}

/// The read/write coverage of a durable demand: whether it observes or mutates the
/// store at all. This is the projection of the compiler-side
/// `marrow_image::ExportDemand` atom set (its `reads()`/`writes()`) that the T01
/// store ceiling checks; the store ceiling is read/write granular, so a
/// path-granular ceiling reserves finer intersection for a later lane. An input to
/// the authority check, never a source of rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemandCoverage {
    pub read: bool,
    pub write: bool,
}

/// The invocation grant minted independently by the CLI runner from the user's
/// invocation — never computed from demand or effect class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationGrant {
    pub read: bool,
    pub write: bool,
}

impl InvocationGrant {
    /// A full grant on the store.
    pub fn full_store() -> Self {
        Self {
            read: true,
            write: true,
        }
    }
}

/// A pre-execution authority denial: the export's demand is not covered by the
/// deployment ceiling intersected with the invocation grant. Source-uncatchable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Denied;

/// A failure to open a durable session, before any instruction runs.
#[derive(Debug)]
pub enum SessionError {
    /// The export's demand exceeds ceiling ∩ grant (`run.authority`).
    Denied,
    /// The store's recorded profile does not match this program's schema.
    ProfileMismatch,
    /// The ordered-byte engine failed while setting up the session.
    Engine(StoreError),
}

/// The whole-entry value read, created, or replaced at an entry site: one slot per
/// field in schema order, present or vacant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryValue {
    pub fields: Vec<Option<ValueDomain>>,
}

/// The presence of the cell a site addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
}

/// The outcome of `create_entry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateOutcome {
    Created,
    AlreadyPresent,
}

/// The outcome of `replace_entry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceOutcome {
    Replaced,
    Missing,
}

/// The outcome of an erase (field or entry). Both are legal (no-op on absent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseOutcome {
    Erased,
    Missing,
}

/// The result of one forward marker-walk step over a durable layer. Kernel-internal:
/// the bounded acquisition consumes it to build a [`BoundedKeys`]; no unbounded
/// next-key op crosses the language boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NextKey {
    Next(KeyScalar),
    End,
}

/// A positive traversal bound `N` from an `at most N` clause: the count of immediate
/// keys a bounded acquisition freezes before probing one beyond to decide the
/// `on more` arm. `NonZeroU32` makes the invariant's positivity unrepresentable when
/// violated; the verifier additionally caps the compile-time constant, and the kernel
/// bounds its frozen-key allocation by it (campaign law 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedLimit(NonZeroU32);

impl BoundedLimit {
    /// A bound from a positive count, or `None` for zero (which the verifier rejects
    /// before an image ever reaches the kernel).
    pub fn new(count: u32) -> Option<Self> {
        NonZeroU32::new(count).map(Self)
    }

    /// The bound as a `usize` frozen-key capacity.
    pub fn get(self) -> usize {
        self.0.get() as usize
    }
}

/// The outcome of a bounded acquisition over one durable layer: the frozen immediate
/// keys in ascending key order (at most the [`BoundedLimit`]), and whether a further
/// key existed beyond them (the `on more` bit). No cursor, page, continuation, or
/// lease escapes — the frozen keys are the whole result, and because they are acquired
/// before any loop body runs they are immune to writes those bodies perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedKeys {
    /// The frozen keys, ascending, `len() <= limit`.
    pub keys: Vec<KeyScalar>,
    /// Whether a `(limit + 1)`th present key existed beyond the frozen set.
    pub more: bool,
}

/// The result of committing a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitResult {
    /// The engine confirmed the commit.
    Committed,
    /// An entry the transaction created or staged still leaves a required field
    /// unset; the transaction rolled back instead of committing a partial entry.
    RequiredMissing { key: KeyScalar, field: String },
    /// The commit did not confirm; the handle is poisoned and must be reopened.
    CommitFault,
}

/// A source-mapped, source-uncatchable kernel fault raised during execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelFault {
    /// The store is internally inconsistent (orphan leaf, undecodable cell).
    Corruption,
    /// The handle was poisoned by an earlier failed commit.
    Poisoned,
    /// A value reaching the store codec is outside its supported range.
    ValueRange,
    /// A durable write would place two distinct entries into one `unique` managed index.
    /// The maintenance write detects the equal-projection collision and faults; the
    /// transaction rolls back without poisoning the store.
    UniqueIndexViolation,
    /// The ordered-byte engine failed mid-operation.
    Engine(StoreError),
}

impl KernelFault {
    /// The stable dotted code a tool reports for this fault.
    pub fn code(&self) -> &'static str {
        match self {
            KernelFault::Corruption => marrow_codes::Code::RunCorruption.as_str(),
            KernelFault::Poisoned => marrow_codes::Code::RunCommit.as_str(),
            KernelFault::ValueRange => marrow_codes::Code::ValueRange.as_str(),
            KernelFault::UniqueIndexViolation => marrow_codes::Code::RunUniqueIndex.as_str(),
            KernelFault::Engine(error) => error.code(),
        }
    }
}

/// The classification of a store after reopening it following an indeterminate
/// commit: the witness cell holds the intended token (the commit completed) or does
/// not (it did not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reopen {
    /// The witness matches the intended token: the commit completed.
    CompleteNew,
    /// The witness is absent or a different token: the commit did not complete.
    CompleteOld,
}

/// An opaque authorized site: a kernel-minted token carrying a site's full shape,
/// resolved once from the sealed site table at session setup. Every kernel op takes
/// one of these plus a key-path, never a caller-asserted address or expected type.
///
/// A site addresses one durable node: the root entry (`branch` empty) or a keyed
/// branch entry nested beneath it (one hop per nested branch, to any depth). The node's
/// marker stem is the root marker
/// followed by one `branch_child_stem` per hop, over the operation's key-path
/// (`[root_key]` for a root node, `[root_key, branch_key, …]` for a branch node); the
/// key-path's scalar kinds must match `key` and each hop's `key`, which the kernel
/// asserts as defense in depth over the verifier's proof.
#[derive(Debug, Clone)]
pub struct AuthorizedSite {
    root: String,
    /// The root's ordered key column kinds, checked against the leading columns of an
    /// operation's key-path.
    key: Vec<ScalarKind>,
    /// The branch path from the root down to the addressed node, one hop per nested
    /// keyed branch. Empty for a root-level node.
    branch: Vec<BranchHop>,
    target: AuthTarget,
}

/// One hop of a site's branch path: the branch's name (which keys its physical child
/// stem) and its ordered key column kinds (checked against the operation key columns).
#[derive(Debug, Clone)]
struct BranchHop {
    name: String,
    key: Vec<ScalarKind>,
}

impl BranchHop {
    fn new(name: String, key: Vec<ScalarKind>) -> Self {
        Self { name, key }
    }
}

#[derive(Debug, Clone)]
enum AuthTarget {
    /// A whole-entry target: the addressed node's own record fields, resolved once at
    /// session setup so the whole-entry ops enumerate its footprint without the schema.
    Entry(Vec<FieldSchema>),
    Field {
        name: String,
        shape: ValueShape,
        required: bool,
        /// The addressed field's containing node record — the root's fields for a
        /// top-level field, a branch's fields for a branch field. A staged sparse or
        /// required set carries this so the commit reconcile validates the *node's*
        /// marker and required fields, node-parametrically, one level down for a branch.
        record: Vec<FieldSchema>,
    },
    /// A managed-index read target: the index's cell-family identity, whether it is a
    /// unique complete-key lookup or a nonunique progressive-prefix scan, and the scalar
    /// kind of each ordered projected component (resolved once from the root's key
    /// columns and top-level fields). An index read never addresses a source node, so it
    /// carries no record or branch path; it validates its operand components against this
    /// projection and reads only the `0x02` index cell family.
    Index {
        id: [u8; 16],
        unique: bool,
        projection: Vec<ScalarKind>,
    },
}

impl AuthTarget {
    /// A field target from a resolved field schema and its containing node record.
    fn field(field: &FieldSchema, record: &[FieldSchema]) -> Self {
        Self::Field {
            name: field.name.clone(),
            shape: field.shape.clone(),
            required: field.required,
            record: record.to_vec(),
        }
    }

    /// A managed-index read target from its cell-family identity, read kind, and
    /// resolved projection component kinds.
    fn index(id: [u8; 16], unique: bool, projection: Vec<ScalarKind>) -> Self {
        Self::Index {
            id,
            unique,
            projection,
        }
    }
}

impl AuthorizedSite {
    /// Assemble a resolved site from its root, root key column kinds, branch path, and
    /// target. Kernel-internal; the store's site resolver is the sole constructor.
    fn new(root: String, key: Vec<ScalarKind>, branch: Vec<BranchHop>, target: AuthTarget) -> Self {
        Self {
            root,
            key,
            branch,
            target,
        }
    }

    /// A root-level managed-index read site: no branch path, an [`AuthTarget::Index`]
    /// target. The store's site resolver is the sole constructor.
    fn index(root: String, key: Vec<ScalarKind>, target: AuthTarget) -> Self {
        Self {
            root,
            key,
            branch: Vec::new(),
            target,
        }
    }

    /// The number of key columns the whole key-path this site addresses carries: the
    /// root's key columns plus every branch hop's key columns, to any depth. The VM pops
    /// exactly this many key operands and assembles them root-first before calling an op.
    pub fn key_arity(&self) -> usize {
        self.key.len() + self.branch.iter().map(|hop| hop.key.len()).sum::<usize>()
    }

    /// The index-read shape this site addresses — its cell-family identity, unique flag,
    /// and ordered projection component kinds — or `None` for a source-node site. The
    /// index ops read this to bound and validate a read without the schema.
    fn index_read(&self) -> Option<(&[u8; 16], bool, &[ScalarKind])> {
        match &self.target {
            AuthTarget::Index {
                id,
                unique,
                projection,
            } => Some((id, *unique, projection)),
            AuthTarget::Entry(_) | AuthTarget::Field { .. } => None,
        }
    }
}
