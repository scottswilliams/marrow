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
//! name-keyed scalar-field root plus single-level, single-column-keyed
//! scalar-field branches; groups, nested or composite-keyed branches, widened
//! field values, and composite root keys stay parked until their owners land them.

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
use crate::codec::value::{RuntimeScalar, ScalarKind};

/// The schema descriptor the store profile records and every session revalidates.
/// One root; its top-level fields and its keyed branches in declaration (image)
/// order. A branch is a keyed subtree nested beneath every root entry (E03 executes
/// single-level, single-column-keyed branches; deeper nesting and composite branch
/// keys are E04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSchema {
    pub root_name: String,
    pub key: ScalarKind,
    pub fields: Vec<FieldSchema>,
    pub branches: Vec<BranchSchema>,
}

/// One field of a node's record: its name, scalar kind, and required flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub kind: ScalarKind,
    pub required: bool,
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
    pub key: ScalarKind,
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
    pub fields: Vec<Option<RuntimeScalar>>,
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
/// branch entry nested beneath it (one hop per nested branch; E03 executes
/// single-level branches, so at most one). The node's marker stem is the root marker
/// followed by one `branch_child_stem` per hop, over the operation's key-path
/// (`[root_key]` for a root node, `[root_key, branch_key, …]` for a branch node); the
/// key-path's scalar kinds must match `key` and each hop's `key`, which the kernel
/// asserts as defense in depth over the verifier's proof.
#[derive(Debug, Clone)]
pub struct AuthorizedSite {
    root: String,
    key: ScalarKind,
    /// The branch path from the root down to the addressed node, one hop per nested
    /// keyed branch. Empty for a root-level node.
    branch: Vec<BranchHop>,
    target: AuthTarget,
}

/// One hop of a site's branch path: the branch's name (which keys its physical child
/// stem) and its single key column's scalar kind (checked against the operation key).
#[derive(Debug, Clone)]
struct BranchHop {
    name: String,
    key: ScalarKind,
}

impl BranchHop {
    fn new(name: String, key: ScalarKind) -> Self {
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
        kind: ScalarKind,
        required: bool,
        /// The addressed field's containing node record — the root's fields for a
        /// top-level field, a branch's fields for a branch field. A staged sparse or
        /// required set carries this so the commit reconcile validates the *node's*
        /// marker and required fields, node-parametrically, one level down for a branch.
        record: Vec<FieldSchema>,
    },
}

impl AuthTarget {
    /// A field target from a resolved field schema and its containing node record.
    fn field(field: &FieldSchema, record: &[FieldSchema]) -> Self {
        Self::Field {
            name: field.name.clone(),
            kind: field.kind,
            required: field.required,
            record: record.to_vec(),
        }
    }
}

impl AuthorizedSite {
    /// Assemble a resolved site from its root, root key kind, branch path, and target.
    /// Kernel-internal; the store's site resolver is the sole constructor.
    fn new(root: String, key: ScalarKind, branch: Vec<BranchHop>, target: AuthTarget) -> Self {
        Self {
            root,
            key,
            branch,
            target,
        }
    }

    /// The key scalar kind this site's root is keyed by.
    pub fn key_kind(&self) -> ScalarKind {
        self.key
    }

    /// The length of the key-path this site addresses: one element for a root node,
    /// plus one per nested branch hop (E03 executes at most one). The VM pops exactly
    /// this many key operands and assembles them root-first before calling an op.
    pub fn key_arity(&self) -> usize {
        1 + self.branch.len()
    }
}
