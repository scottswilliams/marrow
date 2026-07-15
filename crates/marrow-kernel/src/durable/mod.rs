//! The typed durable runtime the VM drives (design §G).
//!
//! The kernel sits below the language. It consumes verified sites and typed
//! scalars — never source — and turns durable operations into ordered-byte engine
//! calls through the narrow [`marrow_store::ByteEngine`] seam. It owns the durable
//! operation algebra outcomes, the authority triple, the store profile, the
//! name-keyed physical layout, and the commit witness.
//!
//! E01 landed the real flat read/write kernel and the ephemeral-memory attachment:
//! a fresh in-memory store minted from a verified image's schema, sites, and
//! deployment ceiling, driving read and single-write sessions bounded by
//! `demand ∩ ceiling ∩ grant`. The parked boundary is the physical layout: the
//! flat name-keyed root with one keyed record of scalar fields. Sparse structural
//! values over nested branches and groups (E03) and composite keys with bounded
//! traversal (E04) widen that layout in their own lanes; E01 never widens it.

mod attach;
mod physical;
mod plan;
mod profile;
mod store;

pub use attach::{
    AttachError, AttachmentId, CeilingIdToken, DeploymentCeiling, EphemeralAttachment,
};
pub use store::{Durable, DurableStore, ReadSession, TxnSession};

use marrow_store::StoreError;

use crate::codec::key::KeyScalar;
use crate::codec::value::{RuntimeScalar, ScalarKind};

/// The schema descriptor the store profile records and every session revalidates.
/// One root at T01; its fields in declaration (image) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSchema {
    pub root_name: String,
    pub key: ScalarKind,
    pub fields: Vec<FieldSchema>,
}

/// One field of the root's record: its name, scalar kind, and required flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub kind: ScalarKind,
    pub required: bool,
}

/// A verified operation site the kernel maps to physical layout, indexed by the
/// image's site index. Its root is the single T01 root; the target is the sealed
/// [`SemanticTarget`](marrow_verify::SemanticTarget) projected to the physical flat
/// root — the whole payload or one of the root's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteSpec {
    pub target: SiteTarget,
}

/// The closed operation-target set the kernel serves over the flat root: the whole
/// keyed payload, or one field leaf identified by its field index into
/// [`StoreSchema::fields`]. The kernel owns the mapping from this sealed semantic
/// target to the name-keyed physical layout (see `physical`); it is the physical
/// projection of the verifier's closed `SemanticTarget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteTarget {
    WholePayload,
    FieldLeaf(u16),
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

/// The result of a forward `next_key` step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextKey {
    Next(KeyScalar),
    End,
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
/// one of these, never a caller-asserted address or expected type.
#[derive(Debug, Clone)]
pub struct AuthorizedSite {
    root: String,
    key: ScalarKind,
    target: AuthTarget,
}

#[derive(Debug, Clone)]
enum AuthTarget {
    Entry,
    Field {
        name: String,
        kind: ScalarKind,
        required: bool,
    },
}

impl AuthorizedSite {
    /// The key scalar kind this site's root is keyed by.
    pub fn key_kind(&self) -> ScalarKind {
        self.key
    }
}
