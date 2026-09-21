//! Opaque ownership of one native engine and its process owner lock.
//!
//! The store directory is the unit of ownership and the node exclusion rests on:
//! the advisory lock is taken on the canonicalized directory itself before any
//! name inside it is opened, with the `lock` marker's own lock behind it (see
//! [`OwnerLock::acquire`]). This module alone derives the `lock` and `store.redb`
//! paths, acquires the advisory lock before admission or engine open, and keeps
//! that lock inseparable from the native engine. An indeterminate commit
//! irreversibly quarantines every node it rests on until process exit.
//!
//! The bound on that guarantee: while a holder is live, no second owner of the
//! same store directory *node* can be constructed, whatever a writer inside that
//! directory does to its children. It is not exclusion over a *path*. A writer
//! that replaces the store directory node itself — moving it aside and publishing
//! another directory under the same name — leaves two owners of two different
//! directories that one path reaches in turn, the custody split the storage
//! reference records.
//!
//! Acquisition is separate from binding so nothing above this module has to read
//! a byte of the store directory to decide exclusion. [`NativeEngineOwner::acquire_existing`]
//! canonicalizes the directory, takes the lock, and returns an affine
//! [`PendingNativeEngineOwner`] having made no engine call and without being told
//! which store instance it is about to hold. Naming the instance afterwards is the
//! only ordering in which a malformed artifact cannot preempt contention.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use marrow_codes::Code;

use crate::engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn};
use crate::error::{StoreError, StoreOp};
use crate::redb::{NativeEngine, RedbTxn, RedbView};

/// The native engine file inside a Marrow store directory.
pub const NATIVE_ENGINE_FILE: &str = "store.redb";
/// The permanent owner-lock file inside a Marrow store directory.
pub const NATIVE_LOCK_FILE: &str = "lock";
/// The native engine format written and accepted by this build.
pub const NATIVE_ENGINE_FORMAT_VERSION: u32 = NativeEngine::FORMAT_VERSION;

const LOCK_MAGIC: &[u8; 4] = b"MWSL";
/// The marker layout uses a state tag: a mutable holder writes a pending marker on
/// acquisition and a bound marker once it has the store instance.
const LOCK_VERSION: u8 = 1;
const PENDING_TAG: u8 = 0x01;
const BOUND_TAG: u8 = 0x02;
const PENDING_BYTES: usize = 4 + 1 + 1 + 4 + 8;
const BOUND_BYTES: usize = PENDING_BYTES + 16;

/// The best-effort identity recorded by a mutable native-store owner.
///
/// Inspection does not publish an identity, so this record can describe an earlier
/// owner rather than the current holder. A pending marker omits the instance.
/// Exclusion follows from the directory lock, independently of this record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeLockOwner {
    /// The recorded process id, which need not identify the current holder.
    pub pid: u32,
    /// The lifecycle store instance bytes, once the holder has bound them.
    pub instance: Option<[u8; 16]>,
    /// The acquisition time in Unix-epoch seconds. This is forensic only.
    pub acquired_unix_secs: u64,
}

impl NativeLockOwner {
    fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(BOUND_BYTES);
        bytes.extend_from_slice(LOCK_MAGIC);
        bytes.push(LOCK_VERSION);
        bytes.push(match self.instance {
            Some(_) => BOUND_TAG,
            None => PENDING_TAG,
        });
        bytes.extend_from_slice(&self.pid.to_be_bytes());
        bytes.extend_from_slice(&self.acquired_unix_secs.to_be_bytes());
        if let Some(instance) = self.instance {
            bytes.extend_from_slice(&instance);
        }
        bytes
    }

    /// The owner a marker names, or `None` for any byte string that is not a whole layout
    /// this build reads. Every access is bounds-checked: the bytes are a contender's input,
    /// so a length this decoder does not expect must reach a verdict rather than an abort.
    fn decode(bytes: &[u8]) -> Option<Self> {
        let field = |from: usize, to: usize| bytes.get(from..to);
        let byte = |at: usize| bytes.get(at).copied();
        if field(0, 4)? != LOCK_MAGIC {
            return None;
        }
        match byte(4)? {
            LOCK_VERSION => {
                let instance = match (byte(5)?, bytes.len()) {
                    (PENDING_TAG, PENDING_BYTES) => None,
                    (BOUND_TAG, BOUND_BYTES) => Some(field(18, 34)?.try_into().ok()?),
                    _ => return None,
                };
                Some(Self {
                    pid: u32::from_be_bytes(field(6, 10)?.try_into().ok()?),
                    instance,
                    acquired_unix_secs: u64::from_be_bytes(field(10, 18)?.try_into().ok()?),
                })
            }
            _ => None,
        }
    }
}

/// Why the native owner lock could not be acquired.
#[derive(Debug)]
pub enum NativeLockError {
    /// Another live owner holds the store.
    StoreInUse { owner: Option<NativeLockOwner> },
    /// This process is denied the access taking the lock requires, so the lock was never
    /// asked for. Nothing about the store was established: a failure to reach the store
    /// directory or its lock entry is not an observation of either.
    AccessDenied(std::io::Error),
    /// The lock file or directory could not be read or synchronized.
    Io(std::io::Error),
}

impl NativeLockError {
    /// Classify an I/O failure taking the lock. A denial is its own state, so it is
    /// decided once here rather than re-read from the error at each reporting boundary.
    fn io(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            Self::AccessDenied(error)
        } else {
            Self::Io(error)
        }
    }

    /// The stable diagnostic code for this lock failure.
    pub fn code(&self) -> Code {
        match self {
            Self::StoreInUse { .. } => Code::StoreLocked,
            Self::AccessDenied(_) => Code::StorePermissionDenied,
            Self::Io(_) => Code::StoreIo,
        }
    }
}

impl std::fmt::Display for NativeLockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreInUse {
                owner:
                    Some(NativeLockOwner {
                        pid,
                        instance: Some(instance),
                        ..
                    }),
            } => {
                write!(
                    formatter,
                    "the store is already open; its marker records process {pid} (store instance ",
                )?;
                for byte in instance {
                    write!(formatter, "{byte:02x}")?;
                }
                write!(formatter, "); close the current holder, then retry")
            }
            Self::StoreInUse { owner: Some(owner) } => write!(
                formatter,
                "the store is already open; its marker records process {}; close the current holder, then retry",
                owner.pid,
            ),
            Self::StoreInUse { owner: None } => write!(
                formatter,
                "the store is already open by another process; close it, then retry",
            ),
            Self::AccessDenied(error) => write!(
                formatter,
                "access to the store directory or its lock is denied: {error}",
            ),
            Self::Io(error) => write!(formatter, "the store lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for NativeLockError {}

/// A failure while acquiring the owner lock over an existing store directory.
#[derive(Debug)]
pub enum NativeOwnerAcquireError {
    /// The store directory could not be pinned to a canonical path.
    Io(std::io::Error),
    /// The process owner lock could not be acquired.
    Lock(NativeLockError),
}

impl NativeOwnerAcquireError {
    /// The stable diagnostic code for this acquisition failure.
    pub fn code(&self) -> Code {
        match self {
            Self::Io(_) => Code::StoreIo,
            Self::Lock(error) => error.code(),
        }
    }
}

impl std::fmt::Display for NativeOwnerAcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(
                formatter,
                "the store directory could not be pinned: {error}"
            ),
            Self::Lock(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for NativeOwnerAcquireError {}

/// A failure while binding an acquired owner and opening its existing engine.
#[derive(Debug)]
pub enum NativeOwnerOpenError<R> {
    /// The owner marker could not be bound to this store instance.
    Lock(NativeLockError),
    /// Admission or a consuming promotion precondition refused the open.
    Refused(R),
    /// The existing native engine could not be opened or audited.
    Store(StoreError),
}

/// A live owner cannot be promoted from this state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePromotionRefusal {
    NotReadOnly,
    Quarantined,
}

/// A point of the owner's open sequence an observer is shown. Production arms no observer;
/// a test arms one that asserts or mutates there, so the sequence it drives is the one the
/// product runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerStep {
    /// Service preparation released the marker descriptor; the directory lock alone excludes.
    MarkerHandoff,
    /// The engine opened; the physical audit it may owe has not run.
    EngineOpened,
}

/// A test's view of the open sequence over one store directory.
type OwnerObserver = dyn Fn(&Path, OwnerStep);

/// The one seam the open sequence exposes.
pub(crate) struct OwnerSeam(Option<Rc<OwnerObserver>>);

impl OwnerSeam {
    const NONE: Self = Self(None);

    #[cfg(test)]
    pub(crate) fn armed(observer: impl Fn(&Path, OwnerStep) + 'static) -> Self {
        Self(Some(Rc::new(observer)))
    }

    fn at(&self, dir: &Path, step: OwnerStep) {
        if let Some(observer) = &self.0 {
            observer(dir, step);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDisposition {
    PreserveUnclean,
    Clean,
    Quarantine,
}

struct OwnerLock {
    /// The store directory node's own advisory lock, taken before any name inside the
    /// directory is opened. See [`OwnerLock::acquire`].
    directory_node: Option<File>,
    file: Option<File>,
    disposition: DropDisposition,
}

impl OwnerLock {
    /// Take directory exclusion without creating or changing its marker.
    ///
    /// Exclusion is taken on the store directory node itself, before any name inside that
    /// directory is opened. A lock resting only on names inside the directory does not
    /// survive their replacement: the marker and the engine file are each replaceable by
    /// unlinking the name and creating another node under it, which hands a contender a node
    /// no holder holds. Each replacement alone is still refused by the other node's lock,
    /// but replacing both leaves neither — the state a whole-directory restore over a live
    /// store also produces. The directory node is the one node in the store that no
    /// replacement of its own children changes, and acquisition pinned it by canonicalizing
    /// before asking for this lock, so exclusion rests on it and the marker's lock stands
    /// behind it.
    fn acquire(dir: &Path) -> Result<Self, NativeLockError> {
        let directory_node = open_directory_node(dir).map_err(NativeLockError::io)?;
        match directory_node.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // A contender is owed the exclusion verdict and nothing else: a marker it
                // cannot read or that is not there costs it the holder's identity, never
                // the verdict itself.
                return Err(NativeLockError::StoreInUse {
                    owner: read_named_owner(dir),
                });
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(NativeLockError::io(error));
            }
        }
        // Every subsequent refusal releases the acquired directory lock through its owner.
        Ok(Self {
            directory_node: Some(directory_node),
            file: None,
            disposition: DropDisposition::PreserveUnclean,
        })
    }

    /// Lock an existing marker for inspection, or publish the mutable holder.
    /// A nonempty prior marker carries an unclean obligation until physical audit.
    fn prepare_existing(
        &mut self,
        dir: &Path,
        access: NativeOpenAccess,
        instance: [u8; 16],
    ) -> Result<AuditObligation, NativeLockError> {
        let obligation = self.inspect_marker(dir, access)?;
        if access != NativeOpenAccess::ReadOnly {
            self.publish_owner(dir, instance)?;
        }
        Ok(obligation)
    }

    fn inspect_marker(
        &mut self,
        dir: &Path,
        access: NativeOpenAccess,
    ) -> Result<AuditObligation, NativeLockError> {
        let Some(mut file) = open_marker(dir, access).map_err(NativeLockError::io)? else {
            return Ok(AuditObligation::Discharged);
        };

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // A contender is owed the exclusion verdict and nothing else: an
                // unreadable marker costs it the holder's identity, never the
                // verdict itself.
                return Err(NativeLockError::StoreInUse {
                    owner: read_owner(&mut file),
                });
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(NativeLockError::io(error));
            }
        }
        let file = self.file.insert(file);

        // Only now that exclusion is settled. A second link to the marker leaves its
        // recorded identity rewritable under a name the owner does not
        // hold, so it is refused — but it does not divide exclusion (every opener of either
        // name locks the same node), and refusing on it ahead of the lock would hand a
        // contender an I/O verdict where the exclusion verdict applies.
        let held = file.metadata().map_err(NativeLockError::io)?;
        admit_held_marker(&held).map_err(NativeLockError::io)?;
        Ok(AuditObligation::of_marker(held.len()))
    }

    fn publish_owner(&mut self, dir: &Path, instance: [u8; 16]) -> Result<(), NativeLockError> {
        if let Some(file) = self.file.as_mut() {
            write_owner(
                file,
                NativeLockOwner {
                    pid: std::process::id(),
                    instance: Some(instance),
                    acquired_unix_secs: now_unix_secs(),
                },
            )
            .map_err(NativeLockError::io)?;
            sync_dir(dir).map_err(NativeLockError::io)?;
        }
        Ok(())
    }

    /// Hand off only the marker descriptor; directory exclusion never changes hands.
    fn prepare_service(
        &mut self,
        dir: &Path,
        instance: [u8; 16],
        seam: &OwnerSeam,
    ) -> Result<AuditObligation, NativeLockError> {
        let previous = self
            .file
            .as_ref()
            .map(File::metadata)
            .transpose()
            .map_err(NativeLockError::io)?;
        let marker = dir.join(NATIVE_LOCK_FILE);
        match &previous {
            Some(held) => verify_named_node(&marker, held).map_err(NativeLockError::io)?,
            None => match std::fs::symlink_metadata(&marker) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(NativeLockError::io(error)),
                Ok(_) => return Err(NativeLockError::Io(changed_node())),
            },
        }
        drop(self.file.take());
        seam.at(dir, OwnerStep::MarkerHandoff);
        let prior_unclean = self.inspect_marker(dir, NativeOpenAccess::ReadWrite)?;
        if let Some(previous) = previous {
            let held = self
                .file
                .as_ref()
                .expect("mutable access retains its marker")
                .metadata()
                .map_err(NativeLockError::io)?;
            if !names_same_node(&previous, &held) {
                return Err(NativeLockError::Io(changed_node()));
            }
        }
        self.publish_owner(dir, instance)?;
        Ok(prior_unclean)
    }

    fn mark_clean(&mut self) {
        debug_assert_ne!(self.disposition, DropDisposition::Quarantine);
        if self.disposition != DropDisposition::Quarantine {
            self.disposition = DropDisposition::Clean;
        }
    }

    fn quarantine(&mut self) {
        self.disposition = DropDisposition::Quarantine;
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        match self.disposition {
            DropDisposition::PreserveUnclean => {}
            DropDisposition::Clean => {
                if let Some(file) = &self.file {
                    let _ = file.set_len(0);
                    let _ = file.sync_all();
                }
            }
            // Quarantine is exclusion for the rest of this process's life, so every handle
            // the exclusion rests on is retained. Releasing the directory node while the
            // marker's lock is leaked would leave the quarantine standing on a name that a
            // writer inside the directory can replace.
            DropDisposition::Quarantine => {
                for handle in [self.directory_node.take(), self.file.take()]
                    .into_iter()
                    .flatten()
                {
                    std::mem::forget(handle);
                }
                return;
            }
        }
        // Closing this process's handles alone leaves locks alive in duplicates inherited
        // by a concurrently spawned child. Engine drop and marker cleanup have finished;
        // release the marker first and the authoritative directory lock last.
        for handle in [&self.file, &self.directory_node].into_iter().flatten() {
            let _ = handle.unlock();
        }
    }
}

/// The only public native-engine capability. The raw engine and owner lock are
/// private and cannot be detached or replaced by safe dependents.
pub struct NativeEngineOwner {
    engine: Option<NativeEngine>,
    lock: OwnerLock,
    directory: PathBuf,
    /// The file admitted by read-only opening, retained across service preparation.
    read_only_snapshot: Option<ReadOnlySnapshot>,
    seam: OwnerSeam,
}

struct ReadOnlySnapshot {
    engine_node: Metadata,
    obligation: AuditObligation,
}

/// Whether a physical integrity audit is still owed on this store directory.
///
/// A holder that exits without clearing its marker leaves the marker nonempty,
/// and the next acquisition inherits the obligation until an audit discharges
/// it. The state travels through admission, service preparation and the
/// read-only snapshot, so it is a named state rather than a bare flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditObligation {
    /// No prior holder left an unaudited marker.
    Discharged,
    /// A nonempty prior marker is outstanding until a physical audit runs.
    Inherited,
}

impl AuditObligation {
    /// The obligation a marker of `len` bytes carries.
    fn of_marker(len: u64) -> Self {
        if len == 0 {
            Self::Discharged
        } else {
            Self::Inherited
        }
    }

    /// The obligation owed when either of two readings is outstanding.
    fn or(self, other: Self) -> Self {
        if self == Self::Inherited || other == Self::Inherited {
            Self::Inherited
        } else {
            Self::Discharged
        }
    }

    fn is_inherited(self) -> bool {
        self == Self::Inherited
    }
}

/// One store directory's owner lock, held before anything in that directory has
/// been read and before any engine call. It is affine: the single way to reach a
/// live engine consumes it, and dropping it instead releases the lock while
/// preserving whatever unclean obligation it inherited, so a refusal taken under
/// this owner leaves the next acquisition owing the same full audit.
///
/// The lock is private and cannot be detached or re-armed by safe dependents.
pub struct PendingNativeEngineOwner {
    lock: OwnerLock,
    directory: PathBuf,
    seam: OwnerSeam,
}

/// The engine capability requested under an existing store's owner lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOpenAccess {
    /// Open for service; discharge any inherited physical-audit obligation.
    ReadWrite,
    /// Inspect without repairing the engine or discharging an inherited obligation.
    ReadOnly,
    /// Explicit recovery always verifies physical integrity, even after a clean close.
    Recovery,
}

impl PendingNativeEngineOwner {
    /// Metadata of the retained directory node whose advisory lock this owner holds.
    /// Renaming the directory does not redirect this observation to its old pathname.
    pub fn directory_metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.lock
            .directory_node
            .as_ref()
            .expect("a pending owner retains its directory lock")
            .metadata()
    }

    /// The canonical store directory this owner holds. The owner above reads the
    /// directory's own artifacts from here under the exclusion already taken.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Run admission under directory exclusion before opening the engine. Mutable
    /// access publishes `instance` in the marker before the callback; inspection
    /// leaves the marker's bytes and absence unchanged throughout. Service audits
    /// an inherited unclean engine, explicit recovery always audits, and inspection
    /// preserves the obligation.
    pub fn bind_and_open_existing<R>(
        mut self,
        access: NativeOpenAccess,
        instance: [u8; 16],
        admit: impl FnOnce() -> Result<(), R>,
    ) -> Result<NativeEngineOwner, NativeOwnerOpenError<R>> {
        let obligation = self
            .lock
            .prepare_existing(&self.directory, access, instance)
            .map_err(NativeOwnerOpenError::Lock)?;
        admit().map_err(NativeOwnerOpenError::Refused)?;

        let path = self.directory.join(NATIVE_ENGINE_FILE);
        let read_only_node = if access == NativeOpenAccess::ReadOnly {
            Some(std::fs::symlink_metadata(&path).map_err(|error| {
                NativeOwnerOpenError::Store(StoreError::Io {
                    op: StoreOp::Open,
                    message: error.to_string(),
                })
            })?)
        } else {
            None
        };
        let mut engine = match access {
            NativeOpenAccess::ReadWrite | NativeOpenAccess::Recovery => {
                NativeEngine::open_existing(&path)
            }
            NativeOpenAccess::ReadOnly => NativeEngine::open_read_only(&path),
        }
        .map_err(NativeOwnerOpenError::Store)?;
        if let Some(node) = &read_only_node {
            verify_named_node(&path, node).map_err(|error| {
                NativeOwnerOpenError::Store(StoreError::Io {
                    op: StoreOp::Open,
                    message: error.to_string(),
                })
            })?;
        }
        self.seam.at(&self.directory, OwnerStep::EngineOpened);
        if access == NativeOpenAccess::Recovery
            || (obligation.is_inherited() && access == NativeOpenAccess::ReadWrite)
        {
            engine
                .audit_integrity()
                .map_err(NativeOwnerOpenError::Store)?;
        }
        let Self {
            mut lock,
            directory,
            seam,
        } = self;
        if access != NativeOpenAccess::ReadOnly {
            lock.mark_clean();
        }
        Ok(NativeEngineOwner {
            engine: Some(engine),
            lock,
            directory,
            read_only_snapshot: read_only_node.map(|engine_node| ReadOnlySnapshot {
                engine_node,
                obligation,
            }),
            seam,
        })
    }
}

impl NativeEngineOwner {
    /// Consume read-only access into service under the same directory lock.
    /// Unchanged read-only-admitted bytes take the saved allocator path. The
    /// callback aborts full repair, not earlier header recovery or out-of-band
    /// mutation. An inherited physical audit still runs; failure preserves the
    /// marker's unclean obligation and returns no service owner.
    pub fn into_service(
        mut self,
        instance: [u8; 16],
    ) -> Result<Self, NativeOwnerOpenError<NativePromotionRefusal>> {
        if self.lock.disposition == DropDisposition::Quarantine {
            return Err(NativeOwnerOpenError::Refused(
                NativePromotionRefusal::Quarantined,
            ));
        }
        let snapshot = self
            .read_only_snapshot
            .take()
            .ok_or(NativeOwnerOpenError::Refused(
                NativePromotionRefusal::NotReadOnly,
            ))?;
        let path = self.directory.join(NATIVE_ENGINE_FILE);
        let check_node = || {
            verify_named_node(&path, &snapshot.engine_node).map_err(|error| {
                NativeOwnerOpenError::Store(StoreError::Io {
                    op: StoreOp::ServicePreparation,
                    message: error.to_string(),
                })
            })
        };
        check_node()?;
        drop(self.engine.take());
        let obligation = self
            .lock
            .prepare_service(&self.directory, instance, &self.seam)
            .map_err(NativeOwnerOpenError::Lock)?;
        check_node()?;
        let mut engine =
            NativeEngine::open_for_service(&path).map_err(NativeOwnerOpenError::Store)?;
        check_node()?;
        self.seam.at(&self.directory, OwnerStep::EngineOpened);
        if obligation.or(snapshot.obligation).is_inherited() {
            engine
                .audit_integrity()
                .map_err(NativeOwnerOpenError::Store)?;
        }
        self.engine = Some(engine);
        self.lock.mark_clean();
        Ok(self)
    }
    /// Create and stamp a new native engine in `store_dir`, returning no live
    /// engine capability. An existing engine path is refused without opening or
    /// modifying it.
    pub fn provision(store_dir: &Path) -> Result<(), StoreError> {
        let directory = std::fs::canonicalize(store_dir).map_err(|error| StoreError::Io {
            op: StoreOp::Provision,
            message: error.to_string(),
        })?;
        let engine = NativeEngine::create_new(&directory.join(NATIVE_ENGINE_FILE))?;
        drop(engine);
        Ok(())
    }

    /// Pin `store_dir` to its canonical path and take its owner lock, making no
    /// engine call and requiring no store instance. Exclusion is decided here, so
    /// no byte of the store directory can be read — or fail to read — ahead of it.
    pub fn acquire_existing(
        store_dir: &Path,
    ) -> Result<PendingNativeEngineOwner, NativeOwnerAcquireError> {
        let directory = std::fs::canonicalize(store_dir).map_err(NativeOwnerAcquireError::Io)?;
        let lock = OwnerLock::acquire(&directory).map_err(NativeOwnerAcquireError::Lock)?;
        Ok(PendingNativeEngineOwner {
            lock,
            directory,
            seam: OwnerSeam::NONE,
        })
    }

    /// Irreversibly quarantine this owner's lock, close the old engine, reopen
    /// the existing file under the same lock, and run a full integrity audit.
    /// No successful result can restore clean-on-drop behavior.
    pub fn reopen_existing_and_audit(mut self) -> Result<Self, StoreError> {
        self.engine().require_write_access(StoreOp::Recovery)?;
        self.lock.quarantine();
        drop(self.engine.take());
        let mut engine = NativeEngine::open_existing(&self.directory.join(NATIVE_ENGINE_FILE))?;
        engine.audit_integrity()?;
        self.engine = Some(engine);
        Ok(self)
    }

    fn engine(&self) -> &NativeEngine {
        self.engine
            .as_ref()
            .expect("a live native owner retains its engine")
    }

    fn engine_mut(&mut self) -> &mut NativeEngine {
        self.engine
            .as_mut()
            .expect("a live native owner retains its engine")
    }
}

/// A coherent read view that cannot outlive its native owner.
pub struct NativeOwnerView<'a> {
    inner: RedbView<'a>,
}

impl ReadView for NativeOwnerView<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

/// A native transaction whose commit verdict controls the physical owner lock.
pub struct NativeOwnerTxn<'a> {
    inner: RedbTxn<'a>,
    lock: &'a mut OwnerLock,
}

impl ReadView for NativeOwnerTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

impl WriteTxn for NativeOwnerTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.inner.put(key, value)
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.inner.remove(key)
    }

    fn commit(self) -> CommitOutcome {
        let Self { inner, lock } = self;
        commit_and_latch(inner, lock)
    }
}

fn commit_and_latch<T: WriteTxn>(inner: T, lock: &mut OwnerLock) -> CommitOutcome {
    let outcome = inner.commit();
    if outcome == CommitOutcome::Indeterminate {
        lock.quarantine();
    }
    outcome
}

impl ByteEngine for NativeEngineOwner {
    type View<'a> = NativeOwnerView<'a>;
    type Txn<'a> = NativeOwnerTxn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        Ok(NativeOwnerView {
            inner: self.engine().read_view()?,
        })
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        let Self { engine, lock, .. } = self;
        let inner = engine
            .as_mut()
            .expect("a live native owner retains its engine")
            .begin()?;
        Ok(NativeOwnerTxn { inner, lock })
    }

    fn require_write_access(&self, op: StoreOp) -> Result<(), StoreError> {
        self.engine().require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.engine_mut().audit_integrity()
    }
}

/// Open the store directory itself as the node exclusion is taken on. Nothing is read or
/// written through this handle: it exists so that the advisory lock rests on the one node in
/// the store that a writer inside the store directory cannot replace under its own name. The
/// path was canonicalized before acquisition asked for it, so the node this reaches is the
/// directory the owner is about to hold.
#[cfg(unix)]
fn open_directory_node(dir: &Path) -> std::io::Result<File> {
    File::open(dir)
}

/// The marker's custody rests on node identity and link counts, which this crate reads
/// through the Unix metadata it has, and the directory node is opened as an ordinary handle
/// only on platforms where that is defined. A platform without them is refused rather than
/// served by a weaker check.
#[cfg(not(unix))]
fn open_directory_node(_dir: &Path) -> std::io::Result<File> {
    Err(marker_refusal(
        "the store directory is admitted on Unix platforms only",
    ))
}

/// The holder identity the store directory's marker names, read without creating the entry
/// and without locking it. A contender refused at the directory node is owed the exclusion
/// verdict; the identity is the detail attached to it, so every failure to read one is an
/// absent identity rather than a different verdict.
fn read_named_owner(dir: &Path) -> Option<NativeLockOwner> {
    let mut file = File::open(dir.join(NATIVE_LOCK_FILE)).ok()?;
    read_owner(&mut file)
}

/// Open the directory's own regular marker. Inspection leaves an absent marker
/// absent; mutable access creates it.
///
/// The entry is classified before the open, so in the ordinary case a link standing in for
/// the marker is refused rather than created through; a link planted between that
/// classification and the open can still be created through, and the comparison of the
/// opened node against the entry afterwards is what refuses it. Either way the handle every
/// later read, write, and lock call uses is the node the directory names. How many names
/// reach that node is deliberately not decided here: a second link does not divide
/// exclusion, so it is admitted after the lock. This is one process's custody of its own
/// store directory, not a defence against a hostile writer inside it: that actor already
/// holds the store's bytes.
#[cfg(unix)]
fn open_marker(dir: &Path, access: NativeOpenAccess) -> std::io::Result<Option<File>> {
    let path = dir.join(NATIVE_LOCK_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(named) => admit_marker_node(&named)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if access == NativeOpenAccess::ReadOnly {
                return Ok(None);
            }
        }
        Err(error) => return Err(error),
    }
    let file = match access {
        NativeOpenAccess::ReadOnly => File::open(&path)?,
        NativeOpenAccess::ReadWrite | NativeOpenAccess::Recovery => OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?,
    };
    let opened = file.metadata()?;
    admit_marker_node(&opened)?;
    if !names_same_node(&std::fs::symlink_metadata(&path)?, &opened) {
        return Err(marker_refusal(
            "the store lock entry does not name the opened marker",
        ));
    }
    Ok(Some(file))
}

/// The marker's custody rests on link counts and node identity, which this crate reads
/// through the Unix metadata it has. A platform without them is refused rather than served
/// by a weaker check.
#[cfg(not(unix))]
fn open_marker(_dir: &Path, _access: NativeOpenAccess) -> std::io::Result<Option<File>> {
    Err(marker_refusal(
        "the store lock is admitted on Unix platforms only",
    ))
}

#[cfg(unix)]
fn admit_marker_node(entry: &Metadata) -> std::io::Result<()> {
    if entry.file_type().is_file() {
        Ok(())
    } else {
        Err(marker_refusal("the store lock is not a regular file"))
    }
}

/// The marker admission that may run only once exclusion is settled: the bytes an owner
/// publishes must be reachable under exactly the name it holds.
#[cfg(unix)]
fn admit_held_marker(entry: &Metadata) -> std::io::Result<()> {
    if std::os::unix::fs::MetadataExt::nlink(entry) == 1 {
        Ok(())
    } else {
        Err(marker_refusal("the store lock carries more than one link"))
    }
}

#[cfg(not(unix))]
fn admit_held_marker(_entry: &Metadata) -> std::io::Result<()> {
    Err(marker_refusal(
        "the store lock is admitted on Unix platforms only",
    ))
}

fn marker_refusal(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

#[cfg(unix)]
fn names_same_node(named: &Metadata, opened: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    named.dev() == opened.dev() && named.ino() == opened.ino()
}

#[cfg(not(unix))]
fn names_same_node(_named: &Metadata, _opened: &Metadata) -> bool {
    false
}

fn changed_node() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "the admitted store file changed",
    )
}

fn verify_named_node(path: &Path, held: &Metadata) -> std::io::Result<()> {
    let named = std::fs::symlink_metadata(path)?;
    if named.file_type().is_file() && names_same_node(&named, held) {
        Ok(())
    } else {
        Err(changed_node())
    }
}

/// The holder identity a marker carries, or `None` when it carries none this build
/// can read. Every failure reads as an absent identity: this is the detail attached
/// to a contention verdict, never the verdict itself.
fn read_owner(file: &mut File) -> Option<NativeLockOwner> {
    let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    if len == 0 || len > BOUND_BYTES {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes).ok()?;
    NativeLockOwner::decode(&bytes)
}

fn write_owner(file: &mut File, owner: NativeLockOwner) -> std::io::Result<()> {
    let bytes = owner.encode();
    file.set_len(bytes.len() as u64)?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(any(unix, windows))]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "native_owner_tests.rs"]
mod tests;
