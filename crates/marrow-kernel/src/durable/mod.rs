//! The typed durable runtime the VM drives.
//!
//! The kernel sits below the language. It consumes verified sites and typed
//! scalars — never source — and turns durable operations into ordered-byte engine
//! calls through the narrow [`marrow_store::ByteEngine`] seam. It owns the durable
//! operation algebra outcomes, the authority triple, the id-keyed physical layout,
//! and the commit witness. [`NumberedProjection`] pairs the schema with fresh or
//! accepted physical addresses.
//!
//! Read and write sessions are bounded by `demand ∩ ceiling ∩ grant`. Complete
//! entries own their fields and groups; keyed descendants occupy independent
//! families, including below absent ancestors. Managed indexes project from
//! root entries. The admitted projection carries the declared key and value
//! shapes, while one numbered layout serves native and in-memory execution.

mod attach;
mod audit;
mod native_owner;
mod physical;
mod plan;
mod schema;
mod session_host;
mod site;
mod store;
mod transfer;

pub use attach::{
    AttachError, AttachmentId, CeilingIdToken, DeploymentCeiling, EphemeralAttachment,
};
pub use audit::{
    AuditFault, AuditFinding, AuditReport, AuditSite, AuditSummary, ContentDigest, ExportError,
    ExportSink, MAX_REPORTED_FINDINGS,
};
pub use native_owner::{NativeRestoreError, NativeStoreOwner, PendingNativeStoreOwner};
pub(crate) use schema::IndexComponentRef;
pub use schema::{
    BranchSchema, FieldSchema, GroupSchema, IndexComponent, IndexSchema, MAX_DURABLE_DEPTH,
    SchemaBuildError, StoreSchema, StoreSchemaBuilder,
};
pub use session_host::SessionHost;
pub(crate) use site::SiteSlot;
pub use site::{ProjectionBuildError, SiteTarget, StoreProjection, StoreProjectionBuilder};
pub use store::{Durable, DurableStore, ReadSession, TxnSession};
pub use transfer::RestoreError;

/// The engine error the store surfaces, re-exported so a downstream lifecycle owner can
/// classify a native open/audit failure without a direct dependency on the byte-engine
/// crate (the path kernel stays the engine's only consumer).
pub use marrow_store::{
    Cell, MAX_KEY_LEN, MAX_VALUE_LEN, NATIVE_ENGINE_FILE, NATIVE_LOCK_FILE, NativeLockError,
    NativeLockOwner, NativeOpenAccess, NativeOwnerAcquireError, NativeOwnerOpenError,
    NativePromotionRefusal, SCAN_MAX_AGGREGATE_BYTES, SCAN_MAX_RECORDS, StoreError, StoreLimit,
    StoreOp,
};

/// The opaque native, redb-backed durable-store owner. Named as an alias so a
/// downstream lifecycle owner can hold it without naming the byte-engine crate.
pub type NativeStore = NativeStoreOwner;

/// The native engine's on-disk format version, re-exported so a downstream lifecycle owner
/// records the engine tuple from the engine's single owner rather than a mirrored
/// literal — without a direct dependency on the byte-engine crate.
pub const NATIVE_ENGINE_FORMAT_VERSION: u32 = marrow_store::NATIVE_ENGINE_FORMAT_VERSION;

use marrow_codes::Code;

use std::num::NonZeroU32;
use std::sync::Arc;

use crate::codec::key::KeyScalar;
use crate::codec::value::{ScalarKind, ValueShape};
use crate::equality::ValueDomain;

/// A durable node's store-local cell-key number: a store-wide, never-reused
/// `u32` assigned to each root, field, group, and branch. Cell keys are prefixed by these
/// numbers rather than by source spelling, so a rename is zero-cell metadata. The width is
/// `u32` for lifetime headroom, independent of the image's `u16` table rings.
pub type NodeNumber = u32;

/// The maximum simultaneous durable-node count in one projection. The builder refuses
/// larger root tables, bounding numbering allocation. Physical numbers retain their
/// separate u32 lifetime space and may exceed this count bound.
pub const MAX_STORE_NODES: u32 = 1 << 16;

/// The store-local numbering of one root's durable nodes, mirroring its [`StoreSchema`]
/// structure: the root's own number, one number per top-level field (in order), one
/// [`GroupNumbering`] per group, and one [`BranchNumbering`] per branch. The shared
/// structural walk mints it from fresh or accepted addresses. A native constructor
/// consumes its inseparable [`NumberedProjection`]; the site resolver walks schema and
/// numbers together. Private fields prevent caller-built or unbounded number trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootNumbering {
    root: NodeNumber,
    fields: Vec<NodeNumber>,
    groups: Vec<GroupNumbering>,
    branches: Vec<BranchNumbering>,
}

impl RootNumbering {
    /// The root node's own cell-key number.
    pub fn root(&self) -> NodeNumber {
        self.root
    }

    /// One number per top-level field, in declaration order.
    pub fn fields(&self) -> &[NodeNumber] {
        &self.fields
    }

    /// One numbering per unkeyed group, in declaration order.
    pub fn groups(&self) -> &[GroupNumbering] {
        &self.groups
    }

    /// One numbering per keyed branch, in declaration order.
    pub fn branches(&self) -> &[BranchNumbering] {
        &self.branches
    }
}

/// The numbering of one unkeyed group: its own number and one number per field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupNumbering {
    number: NodeNumber,
    fields: Vec<NodeNumber>,
}

impl GroupNumbering {
    /// The group node's own cell-key number.
    pub fn number(&self) -> NodeNumber {
        self.number
    }

    /// One number per group field, in declaration order.
    pub fn fields(&self) -> &[NodeNumber] {
        &self.fields
    }
}

/// The numbering of one keyed branch, recursively: its own number, one number per field,
/// and one [`BranchNumbering`] per nested sub-branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchNumbering {
    number: NodeNumber,
    fields: Vec<NodeNumber>,
    branches: Vec<BranchNumbering>,
}

impl BranchNumbering {
    /// The branch node's own cell-key number.
    pub fn number(&self) -> NodeNumber {
        self.number
    }

    /// One number per branch field, in declaration order.
    pub fn fields(&self) -> &[NodeNumber] {
        &self.fields
    }

    /// One numbering per nested sub-branch, in declaration order.
    pub fn branches(&self) -> &[BranchNumbering] {
        &self.branches
    }
}

/// Assign fresh store-wide preorder numbers for provisioning and ephemeral execution.
/// The shared structural walk visits each node, its fields, groups and then branches.
/// Persistent handles instead retain their accepted Head's numbers through
/// [`NumberedProjection::accepted`].
pub fn number_store(projection: &StoreProjection) -> Vec<RootNumbering> {
    let mut next = 0u32;
    let mut alloc = || {
        let n = next;
        // Total by invariant: the projection's builder refused any root table holding more
        // than [`MAX_STORE_NODES`] nodes, so the checked step cannot see `u32::MAX`.
        next = next
            .checked_add(1)
            .expect("a published projection holds at most MAX_STORE_NODES nodes");
        Ok::<_, std::convert::Infallible>(n)
    };
    match number_projection(projection, &mut alloc) {
        Ok(numbering) => numbering,
        Err(never) => match never {},
    }
}

/// A projection paired with the physical numbers its paths will use. The fields are
/// private so construction, restoration and recovery cannot substitute another layout.
#[derive(Debug)]
pub struct NumberedProjection {
    projection: StoreProjection,
    numbering: Vec<RootNumbering>,
}

/// An accepted number sequence does not describe a bounded bijection for its projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberingError {
    Count,
    Duplicate { number: NodeNumber },
    HighWater { number: NodeNumber, next: u32 },
}

impl std::fmt::Display for NumberingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count => write!(f, "the number count does not match the projection"),
            Self::Duplicate { number } => write!(f, "number {number} occurs more than once"),
            Self::HighWater { number, next } => {
                write!(f, "number {number} is not below high-water {next}")
            }
        }
    }
}

impl std::error::Error for NumberingError {}

impl NumberedProjection {
    /// Mint fresh preorder addresses for a new or ephemeral store.
    pub(crate) fn fresh(projection: StoreProjection) -> Self {
        let numbering = number_store(&projection);
        Self {
            projection,
            numbering,
        }
    }

    /// Pair an accepted sequence in the kernel's structural order with its projection.
    /// The lifecycle caller owns the semantic identity join. This boundary checks exact
    /// coverage, uniqueness and the exclusive lifetime high-water, without requiring
    /// dense numbers or equating their values with the simultaneous node-count bound.
    pub fn accepted(
        projection: StoreProjection,
        numbers: &[NodeNumber],
        next_number: u32,
    ) -> Result<Self, NumberingError> {
        if numbers.len() > MAX_STORE_NODES as usize {
            return Err(NumberingError::Count);
        }
        let mut remaining = numbers.iter();
        let mut used = std::collections::HashSet::with_capacity(numbers.len());
        let numbering = number_projection(&projection, &mut || {
            let number = *remaining.next().ok_or(NumberingError::Count)?;
            if number >= next_number {
                return Err(NumberingError::HighWater {
                    number,
                    next: next_number,
                });
            }
            if !used.insert(number) {
                return Err(NumberingError::Duplicate { number });
            }
            Ok(number)
        })?;
        if remaining.next().is_some() {
            return Err(NumberingError::Count);
        }
        Ok(Self {
            projection,
            numbering,
        })
    }
}

fn number_projection<E>(
    projection: &StoreProjection,
    alloc: &mut impl FnMut() -> Result<NodeNumber, E>,
) -> Result<Vec<RootNumbering>, E> {
    projection
        .roots()
        .iter()
        .map(|schema| {
            Ok(RootNumbering {
                root: alloc()?,
                fields: schema
                    .fields()
                    .iter()
                    .map(|_| alloc())
                    .collect::<Result<_, _>>()?,
                groups: schema
                    .groups()
                    .iter()
                    .map(|group| {
                        Ok(GroupNumbering {
                            number: alloc()?,
                            fields: group
                                .fields()
                                .iter()
                                .map(|_| alloc())
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, _>>()?,
                branches: number_branches(schema.branches(), alloc)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod numbering_tests {
    use super::*;

    fn projection() -> StoreProjection {
        let mut root = StoreSchemaBuilder::root("books", vec![ScalarKind::Int]);
        root.scalar_field("value", ScalarKind::Int, true);
        root.open_group("details");
        root.scalar_field("pages", ScalarKind::Int, false);
        root.close_group();
        root.open_branch("notes", vec![ScalarKind::Int]);
        root.scalar_field("text", ScalarKind::Str, false);
        root.close_branch();
        let mut projection = StoreProjection::builder();
        projection.root(root.finish().expect("bounded schema"));
        projection.finish().expect("no sites")
    }

    #[test]
    fn accepted_addresses_follow_every_structural_kind_and_lifetime_space() {
        let layout =
            NumberedProjection::accepted(projection(), &[9, 12, 70_000, 100, 6, 20], 70_001)
                .expect("distinct accepted addresses");
        let root = &layout.numbering[0];
        assert_eq!(root.root(), 9);
        assert_eq!(root.fields(), &[12]);
        assert_eq!(root.groups()[0].number(), 70_000);
        assert_eq!(root.groups()[0].fields(), &[100]);
        assert_eq!(root.branches()[0].number(), 6);
        assert_eq!(root.branches()[0].fields(), &[20]);
    }

    #[test]
    fn accepted_numbers_refuse_incomplete_repeated_and_exhausted_addresses() {
        for (numbers, next, expected) in [
            (vec![0, 1, 2, 3, 4], 6, NumberingError::Count),
            (vec![0, 1, 2, 3, 4, 5, 6], 7, NumberingError::Count),
            (
                vec![0, 1, 2, 3, 4, 4],
                6,
                NumberingError::Duplicate { number: 4 },
            ),
            (
                vec![0, 1, 2, 3, 4, 6],
                6,
                NumberingError::HighWater { number: 6, next: 6 },
            ),
            (
                vec![0, 1, 2, 3, 4, u32::MAX],
                u32::MAX,
                NumberingError::HighWater {
                    number: u32::MAX,
                    next: u32::MAX,
                },
            ),
        ] {
            assert_eq!(
                NumberedProjection::accepted(projection(), &numbers, next).unwrap_err(),
                expected
            );
        }
    }
}

/// The count of durable nodes [`number_store`] would number under one root: the root
/// itself, its fields, each group and the group's fields, and each branch with its fields
/// and sub-branches. Kept beside the walk it mirrors so "node" has one definition; the
/// projection builder sums it over its roots to refuse a table past [`MAX_STORE_NODES`].
/// Per-root by design: a crate-visible signature over a raw root slice would be an intake
/// shape this crate does not offer.
pub(crate) fn root_node_count(schema: &StoreSchema) -> u64 {
    1 + schema.fields().len() as u64
        + schema
            .groups()
            .iter()
            .map(|group| 1 + group.fields().len() as u64)
            .sum::<u64>()
        + branch_node_count(schema.branches())
}

fn branch_node_count(branches: &[BranchSchema]) -> u64 {
    branches
        .iter()
        .map(|branch| 1 + branch.fields().len() as u64 + branch_node_count(branch.branches()))
        .sum()
}

/// Number a level of branches in pre-order, recursing into sub-branches, through the shared
/// counter so the whole forest shares one store-wide number space.
fn number_branches<E>(
    branches: &[BranchSchema],
    alloc: &mut impl FnMut() -> Result<NodeNumber, E>,
) -> Result<Vec<BranchNumbering>, E> {
    branches
        .iter()
        .map(|branch| {
            Ok(BranchNumbering {
                number: alloc()?,
                fields: branch
                    .fields()
                    .iter()
                    .map(|_| alloc())
                    .collect::<Result<_, _>>()?,
                branches: number_branches(branch.branches(), alloc)?,
            })
        })
        .collect()
}

/// One field of a resolved node: the cell-key [`NodeNumber`] the physical layer keys leaves
/// by (never the source spelling), and the value shape and required flag the ops need.
/// The resolver produces these from a [`FieldSchema`] and its [`NodeNumber`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedField {
    pub(super) number: NodeNumber,
    pub(super) shape: ValueShape,
    pub(super) required: bool,
}

/// One resolved unkeyed group: its cell-key [`NodeNumber`] and its resolved fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedGroup {
    pub(super) number: NodeNumber,
    pub(super) fields: Vec<ResolvedField>,
}

/// The read/write coverage of a durable demand: whether it observes or mutates the
/// store at all. The projection of the compiler-side `marrow_image::ExportDemand` atom
/// set (its `reads()`/`writes()`) that the store ceiling checks, which is read/write
/// granular rather than path granular. An input to the authority check, never a source
/// of rights.
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

/// The fourth term of the authority intersection, reserved for an authenticated
/// principal: the full order is `demand ∩ ceiling ∩ grant ∩ principal`, with the first
/// three resolved before the first engine call. Every variant may only narrow — a
/// principal can restrict the effective authority to a subset, never widen it, and never
/// admits an atom the earlier three terms did not already permit. [`Any`] is the only
/// variant, the ⊤ that narrows nothing.
///
/// [`Any`]: Self::Any
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalPredicate {
    /// The ⊤: admits exactly what `demand ∩ ceiling ∩ grant` already permits.
    Any,
}

impl PrincipalPredicate {
    /// Narrow the already-resolved effective authority by this principal predicate — the
    /// fourth and last intersection term. [`Any`](Self::Any) returns it unchanged
    /// (`⊤ ∩ X = X`); a narrowing variant may only clear bits, never set them.
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

/// A failure to open a durable session or inspect a store.
#[derive(Debug)]
pub enum SessionError {
    /// The export's demand exceeds ceiling ∩ grant (`run.authority`).
    Denied,
    /// The handle was poisoned by an earlier indeterminate commit: its durability is
    /// unknown, so no further session or audit may open on it until the opaque recovery
    /// fact is resolved against a freshly opened store. Reachable only on a native handle
    /// whose engine can report an indeterminate commit; the ephemeral memory engine always
    /// confirms. Renders `run.commit`, matching the execution-time
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
/// bounds its frozen-key allocation by it.
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

pub use marrow_codes::DurableCommitState;

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
#[must_use = "a transaction commit outcome must be handled"]
#[derive(Debug)]
pub enum CommitResult {
    /// The engine confirmed the commit.
    Committed,
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
    /// A write would leave a present entry incomplete: an entry or group value short of
    /// its record width or missing a required field, or an erase of a required field or
    /// of a group holding a required leaf. The compiler's constructors and refusals make
    /// this unreachable from a verified image; the kernel refuses it before any engine
    /// access as defense in depth, so a present entry is complete on every path.
    Incomplete,
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
    pub fn code(&self) -> Code {
        match self {
            KernelFault::Corruption | KernelFault::Incomplete => marrow_codes::Code::RunCorruption,
            KernelFault::Poisoned => marrow_codes::Code::RunCommit,
            KernelFault::ValueRange => marrow_codes::Code::ValueRange,
            KernelFault::UniqueIndexViolation => marrow_codes::Code::RunUniqueIndex,
            KernelFault::Engine(error) => error.code(),
        }
    }
}

/// An opaque authorized site: a kernel-minted token carrying a site's full shape,
/// resolved once from the sealed site table at session setup. Every kernel op takes
/// one of these plus a key-path, never a caller-asserted address or expected type.
///
/// A site addresses one durable node: the root entry (`branch` empty) or a keyed
/// branch entry beneath it (one hop per nested branch). The addressed root or final
/// branch number selects its static family; the operation supplies every ancestor and
/// own key column. Their exact arity and scalar domains must match `key` and each hop's
/// `key`, defended by the kernel at the independently verified image boundary.
#[derive(Debug, Clone)]
pub struct AuthorizedSite {
    /// The addressed root's cell-key number: the fixed-width component that keys
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
/// family) and its ordered key column kinds (checked against the operation key columns).
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
        payload: Arc<FieldPayload>,
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

/// Immutable field metadata prepared at session setup. Token copies share both the
/// selected shape and containing record, so they do not copy recursive schema data
/// in proportion to the record's width. The payload has no independent clone route.
#[derive(Debug)]
struct FieldPayload {
    number: NodeNumber,
    shape: ValueShape,
    required: bool,
    /// The addressed field's containing node record — the root's fields for a
    /// top-level field, a branch's fields for a branch field. A field write reads the
    /// sibling leaves a managed index projects from it, node-parametrically, one
    /// level down for a branch.
    record: Vec<ResolvedField>,
}

impl AuthTarget {
    /// A field target from a resolved field and its containing resolved record.
    fn field(field: &ResolvedField, record: &[ResolvedField]) -> Self {
        Self::Field {
            payload: Arc::new(FieldPayload {
                number: field.number,
                shape: field.shape.clone(),
                required: field.required,
                record: record.to_vec(),
            }),
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
