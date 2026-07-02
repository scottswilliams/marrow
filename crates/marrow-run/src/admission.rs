//! Store admission: the second, program-aware stage above `marrow-store`'s sealed open.
//!
//! `marrow-store` mints a [`SealedStore`] whose engine open passed the store-integrity
//! ladder. Admission binds that sealed handle to a checked program's identity and the
//! committed `marrow.lock`, deciding whether the store may serve the program. Every
//! durable open in the runtime and CLI routes through this module, so `SealedStore` — and
//! `TreeStore`'s crate-private constructors behind it — appear nowhere else: a store handle
//! cannot reach a command without passing this boundary.
//!
//! The two stages are crate-aligned by necessity: `marrow-store` cannot see program
//! identity, and the program-aware ladder cannot reach into the store crate's private open.

use std::marker::PhantomData;
use std::path::Path;

use marrow_check::{CheckedProgram, ProjectConfig, ProjectIoError};
use marrow_store::tree::TreeStore;
use marrow_store::{AccessMode, SealedStore, StoreError};

use crate::evolution::{FenceError, fence};
use crate::project_session::ProjectSessionError;

/// Read-only admission: the handle may serve reads but never commits.
pub enum Read {}

/// Write-capable admission: the handle may commit.
pub enum Write {}

/// A durable store handle that reached a command through admission.
///
/// The only public source of a runtime store handle. Its constructor is module-private, so
/// an `AdmittedStore` is proof the handle was opened through [`SealedStore`] rather than
/// around the store-integrity ladder. The access marker `A` records whether the open was
/// read-only or write-capable.
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

    /// Borrow the underlying store for reads, writes, and navigation.
    pub fn store(&self) -> &TreeStore {
        &self.store
    }

    /// Take ownership of the underlying store.
    pub fn into_store(self) -> TreeStore {
        self.store
    }
}

/// Open the durable store at `path` read-only.
pub fn open_read(path: &Path) -> Result<AdmittedStore<Read>, StoreError> {
    Ok(AdmittedStore::new(
        SealedStore::open(path, AccessMode::Read)?.into_store(),
    ))
}

/// Open an existing durable store write-capably; an absent body is an error.
pub fn open_write(path: &Path) -> Result<AdmittedStore<Write>, StoreError> {
    Ok(AdmittedStore::new(
        SealedStore::open(path, AccessMode::Write)?.into_store(),
    ))
}

/// Open the durable store at `path` write-capably, creating the body when it is absent.
pub fn open_create(path: &Path) -> Result<AdmittedStore<Write>, StoreError> {
    Ok(AdmittedStore::new(
        SealedStore::open(path, AccessMode::Create)?.into_store(),
    ))
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

/// Which admission ladder an open runs. The three production ladders differ in when the
/// store-behind fence is judged relative to the identity bind: a write-capable admission
/// must refuse a behind store before anything else, while a read tolerates a behind store
/// readable at its own epoch and reports the behind fence only when the identity bind
/// already failed — the accurate diagnosis for a checkout whose source has also moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionPolicy {
    /// The run open: behind fence first, then the activation fence. The catalog digest is
    /// bound upstream by re-deriving the program against the store's accepted catalog.
    Run,
    /// A read-only surface open: identity bind first, behind reclassification on failure.
    SurfaceRead,
    /// A write-capable surface open: behind fence first, then the identity bind.
    SurfaceWrite,
}

/// The one verdict of the store-admission ladder.
///
/// The ladder has two rungs by physical necessity: the store body is classified against the
/// committed lock before any open (an absent body cannot be opened; a lost one must fail
/// before a write-capable open could re-baseline it), and the identity ladder runs on the
/// opened handle. [`classify_committed_body`] yields `Loss`/`SeedFromLock`/`Admit`;
/// [`admit`] yields `Behind`/`Drift`/`Admit`.
#[derive(Debug)]
pub enum AdmissionVerdict {
    /// The store may serve the program.
    Admit,
    /// The store body is absent while the committed lock records active roots: the
    /// fresh-checkout (or lost-body) case a write-capable open seeds from the committed
    /// identity and a read-only open materializes in memory.
    SeedFromLock,
    /// The store lags a committed activation; `evolve apply` advances it.
    Behind(FenceError),
    /// The store is stamped under an identity this program does not bind: schema drift,
    /// a store evolved by a newer binary, an engine-profile mismatch, or a catalog-digest
    /// mismatch. The payload is the exact refusal.
    Drift(Box<ProjectSessionError>),
    /// A present store lost committed roots its lock recorded: durable identity is gone
    /// and the store fails closed as corruption, never re-baselined.
    Loss(StoreError),
}

/// An infrastructure fault while classifying the store body: the project could not be
/// read or the store could not be opened. Distinct from an [`AdmissionVerdict`], which
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
/// root, there is no committed baseline to contradict and the body is admitted. An absent
/// body under a roots-recording lock is `SeedFromLock`; a present body missing an active
/// root its own epoch covers is `Loss`.
pub fn classify_committed_body(
    root: &Path,
    config: &ProjectConfig,
) -> Result<AdmissionVerdict, BodyClassifyError> {
    marrow_check::guard_data_dir(root, config)?;
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(AdmissionVerdict::Admit);
    };
    let Some(lock) = marrow_check::read_committed_lock(root)? else {
        return Ok(AdmissionVerdict::Admit);
    };
    if !lock.records_active_roots() {
        return Ok(AdmissionVerdict::Admit);
    }
    if store_path_is_absent(&path) {
        return Ok(AdmissionVerdict::SeedFromLock);
    }
    let store = open_read(&path)?.into_store();
    match verify_present_store_lock_roots(Some(&store), Some(&lock)) {
        Ok(()) => Ok(AdmissionVerdict::Admit),
        Err(error) => Ok(AdmissionVerdict::Loss(error)),
    }
}

/// Run the identity/lock ladder on an opened store: the post-open rung of the admission
/// ladder. The policy fixes the rung order; the rungs themselves — the store-behind fence
/// against the lock's epoch high-water, the activation fence over
/// `(accepted_epoch, source_digest, engine_profile)`, and the catalog-digest bind — are
/// shared, so every open judges a store by the same rules.
pub fn admit(
    store: &TreeStore,
    program: &CheckedProgram,
    lock: Option<&marrow_catalog::CatalogLock>,
    policy: AdmissionPolicy,
) -> Result<AdmissionVerdict, ProjectSessionError> {
    match policy {
        AdmissionPolicy::Run | AdmissionPolicy::SurfaceWrite => {
            if let Some(behind) = store_behind_committed_lock(store, lock)? {
                return Ok(AdmissionVerdict::Behind(behind));
            }
            let bound = match policy {
                AdmissionPolicy::Run => {
                    fence_program(program, store).map_err(ProjectSessionError::Fence)
                }
                _ => bind_surface_identity(program, store),
            };
            match bound {
                Ok(()) => Ok(AdmissionVerdict::Admit),
                Err(error) => Ok(AdmissionVerdict::Drift(Box::new(error))),
            }
        }
        AdmissionPolicy::SurfaceRead => match bind_surface_identity(program, store) {
            Ok(()) => Ok(AdmissionVerdict::Admit),
            Err(error) => {
                if let Some(behind) = store_behind_committed_lock(store, lock)? {
                    return Ok(AdmissionVerdict::Behind(behind));
                }
                Ok(AdmissionVerdict::Drift(Box::new(error)))
            }
        },
    }
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

/// Bind a surface open to the checked program's accepted identity: the activation fence —
/// which owns the no-accepted-epoch decision, failing a stamped store opened by a program
/// that binds none closed with `run.durable_store_required` — then the catalog-digest bind.
/// Shared by both surface access modes so a read admits exactly the stores a write would,
/// apart from the behind-store refusal a read tolerates.
fn bind_surface_identity(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), ProjectSessionError> {
    fence_program(program, store).map_err(ProjectSessionError::Fence)?;
    let accepted_digest =
        program
            .catalog
            .accepted_digest
            .as_deref()
            .ok_or_else(|| ProjectSessionError::Catalog {
                code: marrow_catalog::CATALOG_INVALID,
                message: "accepted catalog digest is missing from the checked program".to_string(),
            })?;
    let found = store.catalog_snapshot_digest()?;
    if found.as_deref() != Some(accepted_digest) {
        return Err(ProjectSessionError::SchemaDrift {
            message: "store catalog digest does not match the checked project catalog".to_string(),
        });
    }
    Ok(())
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
) -> Result<Option<FenceError>, ProjectSessionError> {
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
