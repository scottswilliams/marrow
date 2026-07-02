//! Store admission: the second, program-aware stage above `marrow-store`'s sealed open.
//!
//! `marrow-store` mints a [`SealedStore`] whose engine open passed the store-integrity
//! ladder; this module's `open_read`/`open_write`/`open_create` are the only callers of
//! that constructor, so every durable handle in the runtime and CLI originates here.
//! Admission then binds a sealed handle to a checked program's identity and the committed
//! `marrow.lock`: [`admit_read`] and [`admit_write`] consume the sealed handle, run the
//! identity/lock ladder, and are the sole constructors of [`AdmittedStore`] — a value
//! typed to `AdmittedStore` carries proof the ladder ran, not a convention. Paths that
//! legitimately stop at stage 1 (inspection, recovery, restore, identity-establishing
//! apply, pre-admission seeding) hold the `SealedStore` itself, so the skip is typed and
//! visible in their signatures.
//!
//! The two stages are crate-aligned by necessity: `marrow-store` cannot see program
//! identity, and the program-aware ladder cannot reach into the store crate's private open.

use std::marker::PhantomData;
use std::ops::Deref;
use std::path::Path;

use marrow_check::{CheckedProgram, ProjectConfig, ProjectIoError};
use marrow_store::tree::TreeStore;
use marrow_store::{AccessMode, SealedStore, StoreError};

use crate::evolution::{FenceError, fence};

/// Read-only admission: the handle may serve reads but never commits.
pub enum Read {}

/// Write-capable admission: the handle may commit.
pub enum Write {}

/// A durable store handle that passed the admission ladder.
///
/// Constructed only by [`admit_read`], [`admit_write`], and
/// [`admit_committed_memory_read`], so holding one is proof the identity/lock ladder ran.
/// The access marker `A` records whether the open was read-only or write-capable. The
/// handle derefs to [`TreeStore`] for reads, writes, and navigation; only the crate-local
/// discharge paths may take the store back out.
pub struct AdmittedStore<A> {
    store: TreeStore,
    access: PhantomData<A>,
}

impl<A> AdmittedStore<A> {
    fn new(store: TreeStore) -> Self {
        Self {
            store,
            access: PhantomData,
        }
    }

    /// Surrender the witness for a crate-local engine handoff: the auto-apply discharge
    /// path writes through the handle and re-admits after the store advances.
    pub(crate) fn into_store(self) -> TreeStore {
        self.store
    }
}

impl<A> Deref for AdmittedStore<A> {
    type Target = TreeStore;

    fn deref(&self) -> &TreeStore {
        &self.store
    }
}

/// Open the durable store at `path` read-only.
pub fn open_read(path: &Path) -> Result<SealedStore, StoreError> {
    SealedStore::open(path, AccessMode::Read)
}

/// Open an existing durable store write-capably; an absent body is an error.
pub fn open_write(path: &Path) -> Result<SealedStore, StoreError> {
    SealedStore::open(path, AccessMode::Write)
}

/// Open the durable store at `path` write-capably, creating the body when it is absent.
pub fn open_create(path: &Path) -> Result<SealedStore, StoreError> {
    SealedStore::open(path, AccessMode::Create)
}

/// Whether the store path holds no on-disk store. Only a `NotFound` stat is absent: a
/// present file, a symlink loop, a denied lookup, or any other stat error means the path is
/// occupied by something that must route to the store open and fail closed there, never be
/// treated as an absent body the write paths seed. `Path::exists` collapses every stat error
/// to absent, so this inspects the link itself rather than following it.
pub fn store_path_is_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// Decide whether a store satisfies the roots its committed lock recorded.
///
/// The committed `marrow.lock` is the independent witness to durable identity: a PRESENT
/// store missing an active accepted root its own epoch covers has lost data to a rollback or
/// a torn baseline and is `store.corruption`. An ABSENT store body (`store` is `None`) is the
/// disposable-store case — a fresh checkout or a deleted store the write paths seed an empty
/// store from the committed identity for — and never fails closed. A first run records no
/// active root in the lock, so the witness never fires either way. Each committed root is
/// judged by its lock-recorded activation epoch, so a behind checkout legitimately lacks a
/// root activated after its own epoch (the store-behind fence's case) while a missing root
/// the store's epoch covers is a loss whatever the lock's high-water.
pub fn verify_present_store_lock_roots(
    store: Option<&TreeStore>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<(), StoreError> {
    let Some(store) = store else {
        return Ok(());
    };
    marrow_check::tooling::verify_store_roots_against_lock(store, lock)
}

/// The pre-open rung's verdict: how the store body stands against the committed lock.
#[derive(Debug)]
pub enum BodyVerdict {
    /// The body is consistent with the committed lock, or there is no committed baseline
    /// to contradict: proceed to open.
    Consistent,
    /// The store body is absent while the committed lock records active roots: the
    /// fresh-checkout (or lost-body) case a write-capable open seeds from the committed
    /// identity and a read-only open materializes in memory.
    SeedFromLock,
    /// A present store lost committed roots its lock recorded: durable identity is gone
    /// and the store fails closed as corruption, never re-baselined.
    Loss(StoreError),
}

/// An infrastructure fault while classifying the store body: the project could not be
/// read or the store could not be opened. Distinct from a [`BodyVerdict`], which
/// classifies a readable project.
#[derive(Debug)]
pub enum BodyClassifyError {
    Project(ProjectIoError),
    Store(StoreError),
}

impl From<ProjectIoError> for BodyClassifyError {
    fn from(error: ProjectIoError) -> Self {
        Self::Project(error)
    }
}

impl From<StoreError> for BodyClassifyError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Classify the store body against the committed lock before any open: the pre-open rung
/// of the admission ladder and the single owner of the committed-lock-roots guard.
///
/// A `dataDir` occupied by a non-directory is a configuration fault classified through the
/// shared guard before any open, so a stray file never leaks as a raw `store.io`. When no
/// native store path is configured, no lock is committed, or the lock records no active
/// root, there is no committed baseline to contradict and the body is consistent.
pub fn classify_committed_body(
    root: &Path,
    config: &ProjectConfig,
) -> Result<BodyVerdict, BodyClassifyError> {
    marrow_check::guard_data_dir(root, config)?;
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(BodyVerdict::Consistent);
    };
    let Some(lock) = marrow_check::read_committed_lock(root)? else {
        return Ok(BodyVerdict::Consistent);
    };
    if !lock.records_active_roots() {
        return Ok(BodyVerdict::Consistent);
    }
    if store_path_is_absent(&path) {
        return Ok(BodyVerdict::SeedFromLock);
    }
    let store = open_read(&path)?.into_store();
    match verify_present_store_lock_roots(Some(&store), Some(&lock)) {
        Ok(()) => Ok(BodyVerdict::Consistent),
        Err(error) => Ok(BodyVerdict::Loss(error)),
    }
}

/// Which identity bind an admission runs. The run bind is the activation fence alone —
/// the run path re-derives its program against the store's accepted catalog upstream, so
/// the digest agrees by construction — while the surface bind adds the catalog-digest
/// comparison because a surface program is checked against the source tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionPolicy {
    Run,
    Surface,
}

/// Why an identity bind refused the store. Callers map these to their own error spaces;
/// admission never constructs a session- or CLI-level error.
#[derive(Debug)]
pub enum AdmissionDrift {
    /// The activation fence refused: schema drift, a store evolved by a newer binary, an
    /// engine-profile mismatch, a stamped store under a program binding no accepted epoch,
    /// or a store fault while fencing.
    Fence(FenceError),
    /// The checked program carries no accepted catalog digest to bind.
    MissingAcceptedDigest,
    /// The store's accepted catalog digest does not match the checked program's.
    CatalogDigestMismatch,
}

/// The outcome of a read admission.
pub enum ReadAdmission {
    Admitted(AdmittedStore<Read>),
    /// The store lags a committed activation; `evolve apply` advances it.
    Behind(FenceError),
    /// The identity bind refused. The unadmitted handle is returned so discharge and
    /// dry-run classification paths can read the store they are repairing or reporting on.
    Drift {
        store: TreeStore,
        reason: AdmissionDrift,
    },
}

/// The outcome of a write admission.
pub enum WriteAdmission {
    Admitted(AdmittedStore<Write>),
    /// The store lags a committed activation; `evolve apply` advances it.
    Behind(FenceError),
    /// The identity bind refused. The unadmitted handle is returned so the auto-apply
    /// discharge path can advance the store it just judged.
    Drift {
        store: TreeStore,
        reason: AdmissionDrift,
    },
}

/// Admit a sealed handle for reading. A read cannot corrupt the store, so under the
/// surface policy a behind store still readable at its own epoch is admitted and the
/// behind fence is reported only when the identity bind already failed — the accurate
/// diagnosis for a checkout whose source has also moved past that epoch's shape. The run
/// policy judges the behind fence first, matching the write-capable run open it previews.
pub fn admit_read(
    sealed: SealedStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
    policy: AdmissionPolicy,
) -> Result<ReadAdmission, StoreError> {
    let store = sealed.into_store();
    match policy {
        AdmissionPolicy::Run => match behind_then_bind(&store, program, lock, policy)? {
            LadderOutcome::Pass => Ok(ReadAdmission::Admitted(AdmittedStore::new(store))),
            LadderOutcome::Behind(behind) => Ok(ReadAdmission::Behind(behind)),
            LadderOutcome::Drift(reason) => Ok(ReadAdmission::Drift { store, reason }),
        },
        AdmissionPolicy::Surface => admit_surface_read(store, program, lock),
    }
}

/// Admit the in-memory materialization of the empty committed identity: the read-only
/// serve over an absent store body, where the caller minted a memory store from the
/// committed lock. The same surface-read ladder runs; memory bytes have no engine open to
/// seal, so this is the one admission that does not consume a [`SealedStore`].
pub fn admit_committed_memory_read(
    store: TreeStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<ReadAdmission, StoreError> {
    admit_surface_read(store, program, lock)
}

fn admit_surface_read(
    store: TreeStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<ReadAdmission, StoreError> {
    match bind_identity(&store, program, AdmissionPolicy::Surface)? {
        None => Ok(ReadAdmission::Admitted(AdmittedStore::new(store))),
        Some(reason) => {
            if let Some(behind) = store_behind_committed_lock(&store, lock)? {
                return Ok(ReadAdmission::Behind(behind));
            }
            Ok(ReadAdmission::Drift { store, reason })
        }
    }
}

/// Admit a sealed handle for writing. A write-capable open must not seize a store behind
/// a committed activation, even a byte-clean one, so both policies judge the behind fence
/// before the identity bind.
pub fn admit_write(
    sealed: SealedStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
    policy: AdmissionPolicy,
) -> Result<WriteAdmission, StoreError> {
    let store = sealed.into_store();
    match behind_then_bind(&store, program, lock, policy)? {
        LadderOutcome::Pass => Ok(WriteAdmission::Admitted(AdmittedStore::new(store))),
        LadderOutcome::Behind(behind) => Ok(WriteAdmission::Behind(behind)),
        LadderOutcome::Drift(reason) => Ok(WriteAdmission::Drift { store, reason }),
    }
}

enum LadderOutcome {
    Pass,
    Behind(FenceError),
    Drift(AdmissionDrift),
}

fn behind_then_bind(
    store: &TreeStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
    policy: AdmissionPolicy,
) -> Result<LadderOutcome, StoreError> {
    if let Some(behind) = store_behind_committed_lock(store, lock)? {
        return Ok(LadderOutcome::Behind(behind));
    }
    match bind_identity(store, program, policy)? {
        None => Ok(LadderOutcome::Pass),
        Some(reason) => Ok(LadderOutcome::Drift(reason)),
    }
}

/// Bind the store to the checked program's accepted identity: the activation fence —
/// which owns the no-accepted-epoch decision, failing a stamped store opened by a program
/// that binds none closed with `run.durable_store_required` — then, under the surface
/// policy, the catalog-digest bind.
fn bind_identity(
    store: &TreeStore,
    program: &CheckedProgram,
    policy: AdmissionPolicy,
) -> Result<Option<AdmissionDrift>, StoreError> {
    if let Err(error) = fence_program(program, store) {
        return Ok(Some(AdmissionDrift::Fence(error)));
    }
    if policy == AdmissionPolicy::Run {
        return Ok(None);
    }
    let Some(accepted_digest) = program.catalog.accepted_digest.as_deref() else {
        return Ok(Some(AdmissionDrift::MissingAcceptedDigest));
    };
    let found = store.catalog_snapshot_digest()?;
    if found.as_deref() != Some(accepted_digest) {
        return Ok(Some(AdmissionDrift::CatalogDigestMismatch));
    }
    Ok(None)
}

/// The activation fence over `(accepted_epoch, source_digest, engine_profile)`: the epoch
/// rung of the admission ladder, also re-run after an auto-apply or recheck rebinds the
/// program against the store it just advanced.
pub(crate) fn fence_program(program: &CheckedProgram, store: &TreeStore) -> Result<(), FenceError> {
    fence(
        program.catalog.accepted_epoch,
        &program.source_digest(),
        store,
    )
}

/// Whether the store lags the committed lock. The lock records the epoch a write path last
/// activated against the shared source tree, so a stamped store whose epoch is below the
/// lock's high-water is a local checkout that has not caught up to an activation a teammate
/// already committed — the store-behind fence (`evolve apply` advances it), distinct from
/// same-epoch schema drift, which auto-applies a fresh activation. The store remains the
/// sole authority: the lock never rewinds or overrides it.
fn store_behind_committed_lock(
    store: &TreeStore,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<Option<FenceError>, StoreError> {
    let Some(commit) = store.read_commit_metadata()? else {
        return Ok(None);
    };
    let Some(lock) = lock else {
        return Ok(None);
    };
    if lock.epoch_high_water > commit.catalog_epoch {
        Ok(Some(FenceError::StoreBehind {
            stored: commit.catalog_epoch,
            accepted: lock.epoch_high_water,
        }))
    } else {
        Ok(None)
    }
}
