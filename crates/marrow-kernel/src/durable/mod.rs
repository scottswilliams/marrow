//! The typed durable runtime the VM drives (design §G).
//!
//! The kernel sits below the language. It consumes verified sites and typed
//! scalars — never source — and turns durable operations into ordered-byte engine
//! calls through the narrow [`marrow_store::ByteEngine`] seam. It owns the durable
//! operation algebra outcomes, the authority triple, the id-keyed physical layout
//! ([`number_store`] assigns every node its cell-key number), and the commit witness.
//!
//! The kernel provides the flat read/write kernel and the ephemeral-memory
//! attachment: a fresh in-memory store minted from a verified image's schema,
//! sites, and deployment ceiling, driving read and single-write sessions bounded
//! by `demand ∩ ceiling ∩ grant`. The executable physical layout is the
//! id-keyed root — its fields each a scalar or a widened value (`struct`/`enum`/
//! `Option`, framed inline) — plus keyed branches nested to any depth; groups,
//! composite-keyed branches, nominal-typed fields, and composite root keys stay
//! parked until their owners land them.

mod attach;
mod native_owner;
mod physical;
mod plan;
mod session_host;
mod store;

pub use attach::{
    AttachError, AttachmentId, CeilingIdToken, DeploymentCeiling, EphemeralAttachment,
};
pub use native_owner::NativeStoreOwner;
pub use session_host::SessionHost;
pub use store::{Durable, DurableStore, ReadSession, TxnSession};

/// The engine error the store surfaces, re-exported so a downstream lifecycle owner can
/// classify a native open/audit failure without a direct dependency on the byte-engine
/// crate (the path kernel stays the engine's only consumer).
pub use marrow_store::{
    NATIVE_ENGINE_FILE, NATIVE_LOCK_FILE, NativeLockError, NativeLockOwner, NativeOwnerOpenError,
    StoreError,
};

/// A native, redb-backed durable store — the concrete type [`DurableStore::open_native`]
/// yields. Named as an alias so a downstream lifecycle owner can hold a native store without
/// naming the byte-engine crate.
pub type NativeStore = NativeStoreOwner;

/// The native engine's on-disk format version, re-exported so a downstream lifecycle owner
/// records the engine tuple (FR01 R2) from the engine's single owner rather than a mirrored
/// literal — without a direct dependency on the byte-engine crate.
pub const NATIVE_ENGINE_FORMAT_VERSION: u32 = marrow_store::NATIVE_ENGINE_FORMAT_VERSION;

use std::num::NonZeroU32;

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
    /// The root's unkeyed groups, in declaration (image) order. A group is a static
    /// field-path namespace inside the entry's payload — not a keyed node — so it
    /// contributes leaves to every root entry rather than a keyed child layer. Empty for a
    /// root that declares none.
    pub groups: Vec<GroupSchema>,
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

/// One unkeyed group nested beneath a root entry: its name and its own record's fields.
/// A group is part of the entry's materialized value (a nested sub-record), not a keyed
/// durable node: it carries no marker and no key, and its presence is exactly its
/// containing entry's presence. Its leaves are stored as the entry's own payload,
/// namespaced under the group's number (`<marker> 0x28 num(group) 0x10 num(field)`; see
/// [`physical`](self)). A group holding nested groups or branches is not yet part of the
/// executable graph, so this schema carries fields only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSchema {
    pub name: String,
    pub fields: Vec<FieldSchema>,
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

/// A durable node's store-local cell-key number (FR01 §3): a store-wide, never-reused
/// `u32` assigned to each root, field, group, and branch. Cell keys are prefixed by these
/// numbers rather than by source spelling, so a rename is zero-cell metadata. The width is
/// `u32` for lifetime headroom, independent of the image's `u16` table rings (FR01 §4).
pub type NodeNumber = u32;

/// The store-local numbering of one root's durable nodes, mirroring its [`StoreSchema`]
/// structure: the root's own number, one number per top-level field (in order), one
/// [`GroupNumbering`] per group, and one [`BranchNumbering`] per branch. Computed once from
/// the schema at store construction by [`number_store`], and walked in lockstep with the
/// schema by the site resolver to number every addressed node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootNumbering {
    pub root: NodeNumber,
    pub fields: Vec<NodeNumber>,
    pub groups: Vec<GroupNumbering>,
    pub branches: Vec<BranchNumbering>,
}

/// The numbering of one unkeyed group: its own number and one number per field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupNumbering {
    pub number: NodeNumber,
    pub fields: Vec<NodeNumber>,
}

/// The numbering of one keyed branch, recursively: its own number, one number per field,
/// and one [`BranchNumbering`] per nested sub-branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchNumbering {
    pub number: NodeNumber,
    pub fields: Vec<NodeNumber>,
    pub branches: Vec<BranchNumbering>,
}

/// Assign store-wide pre-order [`NodeNumber`]s to every durable node of every root, the
/// single owner of the cell-key numbering (FR01 §3). One global counter starts at zero and
/// walks each root in declaration order; within a node it numbers the node itself, then its
/// fields in order, then each group (the group node, then the group's fields), then each
/// branch (the branch node, then its fields, then its sub-branches recursively). The order
/// is a deterministic function of the schema structure, so the ephemeral and native paths —
/// which derive the same schema from the same image — number identically, and no second
/// grammar exists. The result is store-wide unique, the bijection the head map (F02a
/// provision) persists against ledger ids.
pub fn number_store(schemas: &[StoreSchema]) -> Vec<RootNumbering> {
    let mut next = 0u32;
    let mut alloc = || {
        let n = next;
        next += 1;
        n
    };
    schemas
        .iter()
        .map(|schema| RootNumbering {
            root: alloc(),
            fields: schema.fields.iter().map(|_| alloc()).collect(),
            groups: schema
                .groups
                .iter()
                .map(|group| GroupNumbering {
                    number: alloc(),
                    fields: group.fields.iter().map(|_| alloc()).collect(),
                })
                .collect(),
            branches: number_branches(&schema.branches, &mut alloc),
        })
        .collect()
}

/// Number a level of branches in pre-order, recursing into sub-branches, through the shared
/// counter so the whole forest shares one store-wide number space.
fn number_branches(
    branches: &[BranchSchema],
    alloc: &mut impl FnMut() -> NodeNumber,
) -> Vec<BranchNumbering> {
    branches
        .iter()
        .map(|branch| BranchNumbering {
            number: alloc(),
            fields: branch.fields.iter().map(|_| alloc()).collect(),
            branches: number_branches(&branch.branches, alloc),
        })
        .collect()
}

/// One field of a resolved node: the cell-key [`NodeNumber`] the physical layer keys leaves
/// by (never the source spelling), the value shape and required flag the ops need, and the
/// field's source name retained for diagnostics only — the `RequiredMissing` reconcile fault
/// names the missing required field, but no cell key is ever built from the name. The
/// resolver produces these from a [`FieldSchema`] and its [`NodeNumber`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedField {
    pub(super) number: NodeNumber,
    pub(super) name: String,
    pub(super) shape: ValueShape,
    pub(super) required: bool,
}

/// One resolved unkeyed group: its cell-key [`NodeNumber`] and its resolved fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedGroup {
    pub(super) number: NodeNumber,
    pub(super) fields: Vec<ResolvedField>,
}

/// A verified operation site the kernel maps to physical layout, indexed by the
/// image's site index. `root` is the site's durable root by declaration position —
/// its index into the store's root-indexed schema table, so a per-root read or write
/// resolves against exactly that root's [`StoreSchema`] and name-keyed cell family. The
/// target is the sealed [`SemanticTarget`](marrow_verify::SemanticTarget) projected to
/// that root's physical layout — the whole payload, one field, a keyed branch, a group,
/// or a managed-index read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSpec {
    /// The site's durable root by declaration position: its index into the store's
    /// root-indexed schema table.
    pub root: u16,
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
    /// The materialized record of one unkeyed group of the root entry, named by the
    /// group's index into [`StoreSchema::groups`]. Its whole read/replace/erase confine
    /// to the group's own leaves under the group prefix, disjoint from the entry's
    /// top-level fields, its sibling groups, and its branches (the group-scoped
    /// payload-only law). Emitted by the verifier's group-site admission; until then the
    /// kernel's own tests are its only source.
    GroupEntry(u16),
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

/// The reserved fourth term of the authority intersection: a typed predicate a future
/// authenticated principal would further intersect with the effective authority, after
/// `demand ∩ ceiling ∩ grant` is resolved. Reserving the *place* in the order is a
/// cross-cutting invariant (the kernel authority law): the full intersection order is
/// `demand ∩ ceiling ∩ grant ∩ principal`, with the first three resolved before the first
/// engine call. This fourth term is a reserved *position*, not yet applied on the live
/// session path ([`Self::narrow`] exists but is not called by `resolve_authority` today);
/// because the only variant is ⊤ it would narrow nothing, so leaving it unapplied changes
/// no authority.
///
/// [`Any`](Self::Any) is the only variant today — the ⊤ predicate that narrows nothing, so
/// the reserved term is the identity of the intersection (`X ∩ ⊤ = X`) and adds no authority.
/// A future authenticated-principal design adds only *narrowing* variants: a principal
/// predicate can restrict the effective authority to a subset, never widen it, and never adds
/// an atom the earlier three terms did not already permit. This is a reserved slot, not a
/// compatibility promise; no principal system exists yet (no framework on credit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalPredicate {
    /// The reserved ⊤: admits exactly what `demand ∩ ceiling ∩ grant` already permits, adding
    /// nothing and removing nothing.
    Any,
}

impl PrincipalPredicate {
    /// Narrow the already-resolved effective authority by this principal predicate — the fourth
    /// and last intersection term. [`Any`](Self::Any) returns the effective authority unchanged
    /// (⊤ ∩ X = X): the reserved term never adds authority and, today, removes none. A future
    /// narrowing variant can only clear bits, never set them.
    pub fn narrow(self, effective: DemandCoverage) -> DemandCoverage {
        match self {
            PrincipalPredicate::Any => effective,
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
    /// The handle was poisoned by an earlier indeterminate commit: its durability is
    /// unknown, so no further session may open on it until the opaque recovery fact is
    /// resolved against a freshly opened store. Consulted at session open so a read or
    /// write against a poisoned handle refuses rather than observing an indeterminate
    /// state. Reachable only on a native handle whose engine can report an indeterminate
    /// commit; the ephemeral memory engine always confirms, so its handle is never
    /// poisoned. Renders `run.commit`, matching the execution-time
    /// [`KernelFault::Poisoned`] the same latch drives at commit.
    Poisoned,
    /// The ordered-byte engine failed while setting up the session.
    Engine(StoreError),
}

/// The record read, created, or replaced at an entry or group site: one slot per field
/// in schema order, present or vacant, plus one nested sub-record per schema group. An
/// entry site's record is the node's own top-level fields followed by its groups (aligned
/// to [`StoreSchema::groups`], each a group-scoped [`EntryValue`] over that group's own
/// field set); a group site's record is that group's own fields with no further groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryValue {
    pub fields: Vec<Option<ValueDomain>>,
    /// One materialized sub-record per schema group of the node, in [`StoreSchema::groups`]
    /// order. Each sub-record's `fields` align to that group's [`GroupSchema::fields`] and
    /// its own `groups` is empty (a group holding nested groups is not yet executable).
    /// Empty for a node that declares no group.
    pub groups: Vec<EntryValue>,
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

/// The durable state of an invocation that did not complete. This is independent of
/// whether the function returned: a commit can be known to have left the store old or
/// new even though instructions after the commit and the function return never ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableCommitState {
    /// The interrupted commit is proven not to have changed durable state.
    KnownOld,
    /// The interrupted commit is proven to have installed its proposed durable state.
    KnownNew,
    /// The durable state cannot be classified.
    Unknown,
}

/// The lifecycle scope of one attached store. It is not authority: it only prevents a
/// recovery fact minted for one store instance and retained path from classifying another
/// store. The fields stay private and no byte projection exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitRecoveryScope {
    instance: [u8; 16],
    path: std::path::PathBuf,
}

impl CommitRecoveryScope {
    /// Bind recovery to the lifecycle-owned store instance and the exact path retained by
    /// that open. A lifecycle recovery reuses this value while continuously holding the
    /// store's owner lock.
    pub(crate) fn persistent(instance: [u8; 16], path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            instance,
            path: path.into(),
        }
    }
}

/// The one opaque affine fact created by an indeterminate engine commit. It owns the exact
/// before and proposed-after witness-cell states plus the attached store's lifecycle scope.
/// There is deliberately no constructor, clone, copy, byte accessor, or serialization API;
/// only the kernel can mint it and classification consumes it.
///
/// ```compile_fail
/// use marrow_kernel::durable::CommitRecovery;
/// fn duplicate(fact: CommitRecovery) {
///     let copy = fact;
///     drop((fact, copy));
/// }
/// ```
///
/// ```compile_fail
/// use marrow_kernel::durable::CommitRecovery;
/// fn clone_fact(fact: CommitRecovery) {
///     let _copy: CommitRecovery = fact.clone();
/// }
/// ```
///
/// ```compile_fail
/// use marrow_kernel::durable::CommitRecovery;
/// fn compare(left: &CommitRecovery, right: &CommitRecovery) {
///     let _same = left == right;
/// }
/// ```
#[must_use = "an indeterminate commit recovery fact must be classified or its attached service retired"]
pub struct CommitRecovery {
    pub(super) scope: Option<CommitRecoveryScope>,
    pub(super) before: Option<Vec<u8>>,
    pub(super) after: Vec<u8>,
}

impl std::fmt::Debug for CommitRecovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommitRecovery { .. }")
    }
}

/// The result of committing a transaction.
///
/// ```compile_fail
/// use marrow_kernel::durable::CommitResult;
/// fn require_partial_eq<T: PartialEq>() {}
/// fn main() {
///     require_partial_eq::<CommitResult>();
/// }
/// ```
#[must_use = "a transaction commit outcome must be handled"]
#[derive(Debug)]
pub enum CommitResult {
    /// The engine confirmed the commit.
    Committed,
    /// An entry the transaction created or staged still leaves a required field
    /// unset; the transaction rolled back instead of committing a partial entry.
    RequiredMissing { key: KeyScalar, field: String },
    /// The transaction is proven not to have committed: a pre-commit operation failed or
    /// the engine explicitly reported an abort. The handle remains usable.
    Aborted,
    /// The engine could not say whether the commit landed. The handle is poisoned and the
    /// sole opaque recovery fact must be consumed by classification or the attached service
    /// retired.
    Indeterminate(CommitRecovery),
    /// This session no longer owns a live engine transaction because its commit boundary was
    /// already crossed. This is a caller-protocol fault and makes no claim about the durable
    /// outcome of the earlier attempt.
    SessionFinished,
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
    /// The addressed root's cell-key number (FR01 §3): the fixed-width component that keys
    /// the root's physical cell family, in place of its source spelling.
    root_number: NodeNumber,
    /// The addressed root's declaration position — its index into the store's
    /// root-indexed schema and per-root managed-index tables. A write op maintains
    /// exactly this root's indexes; the root *number* keys the physical cell family, and
    /// this index selects the root's schema-derived facts.
    root_index: u16,
    /// The root's ordered key column kinds, checked against the leading columns of an
    /// operation's key-path.
    key: Vec<ScalarKind>,
    /// The branch path from the root down to the addressed node, one hop per nested
    /// keyed branch. Empty for a root-level node.
    branch: Vec<BranchHop>,
    target: AuthTarget,
}

/// One hop of a site's branch path: the branch's cell-key number (which keys its physical
/// child stem) and its ordered key column kinds (checked against the operation key columns).
#[derive(Debug, Clone)]
struct BranchHop {
    number: NodeNumber,
    key: Vec<ScalarKind>,
}

impl BranchHop {
    fn new(number: NodeNumber, key: Vec<ScalarKind>) -> Self {
        Self { number, key }
    }
}

#[derive(Debug, Clone)]
enum AuthTarget {
    /// A whole-entry target: the addressed node's own resolved record fields and its
    /// resolved groups, numbered once at session setup so the whole-entry ops enumerate its
    /// footprint — marker, own field leaves, and every group's leaves — without the schema
    /// or any source spelling. A branch node carries no group (group-in-branch is not yet
    /// executable), so its group list is empty.
    Entry {
        fields: Vec<ResolvedField>,
        groups: Vec<ResolvedGroup>,
    },
    Field {
        number: NodeNumber,
        shape: ValueShape,
        required: bool,
        /// The addressed field's containing node record — the root's fields for a
        /// top-level field, a branch's fields for a branch field. A staged sparse or
        /// required set carries this so the commit reconcile validates the *node's*
        /// marker and required fields, node-parametrically, one level down for a branch.
        record: Vec<ResolvedField>,
    },
    /// A whole-group target: the group's cell-key number (which keys its physical leaf
    /// namespace under the containing entry) and its own resolved record fields. A group
    /// carries no marker and no key — its presence is its containing entry's presence — so
    /// the whole-group ops materialize, replace, or erase the group's leaves scoped to this
    /// field set, leaving the entry's marker, top-level fields, sibling groups, and branches
    /// intact.
    Group {
        number: NodeNumber,
        fields: Vec<ResolvedField>,
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
    /// A field target from a resolved field and its containing resolved record.
    fn field(field: &ResolvedField, record: &[ResolvedField]) -> Self {
        Self::Field {
            number: field.number,
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
    /// Assemble a resolved site from its root number and declaration position, root key
    /// column kinds, branch path, and target. Kernel-internal; the store's site resolver
    /// is the sole constructor.
    fn new(
        root_number: NodeNumber,
        root_index: u16,
        key: Vec<ScalarKind>,
        branch: Vec<BranchHop>,
        target: AuthTarget,
    ) -> Self {
        Self {
            root_number,
            root_index,
            key,
            branch,
            target,
        }
    }

    /// A root-level managed-index read site: no branch path, an [`AuthTarget::Index`]
    /// target. The store's site resolver is the sole constructor.
    fn index(
        root_number: NodeNumber,
        root_index: u16,
        key: Vec<ScalarKind>,
        target: AuthTarget,
    ) -> Self {
        Self {
            root_number,
            root_index,
            key,
            branch: Vec::new(),
            target,
        }
    }

    /// The addressed root's declaration position — its index into the store's per-root
    /// schema and managed-index tables.
    pub(super) fn root_index(&self) -> u16 {
        self.root_index
    }

    /// The number of key columns the whole key-path this site addresses carries: the
    /// root's key columns plus every branch hop's key columns, to any depth. The VM pops
    /// exactly this many key operands and assembles them root-first before calling an op.
    pub fn key_arity(&self) -> usize {
        self.key.len() + self.branch.iter().map(|hop| hop.key.len()).sum::<usize>()
    }

    /// The number of ordered projection components an index-read site addresses, or
    /// `None` for a source-node site. A unique lookup pops this many key operands; a
    /// progressive scan pops one fewer (the held prefix) and yields the trailing one.
    pub fn index_projection_len(&self) -> Option<usize> {
        self.index_read().map(|(_, _, projection)| projection.len())
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
            AuthTarget::Entry { .. } | AuthTarget::Field { .. } | AuthTarget::Group { .. } => None,
        }
    }
}
