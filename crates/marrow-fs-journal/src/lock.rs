//! The cooperative cache lock: affine, non-clone, close-on-exec.

use std::fmt;

use crate::custody::{self, AdmittedDir, CustodyError, FsIdentity, NodeKind};
use crate::entry::EntryName;
use crate::sys;

/// An exclusively held cooperative lock on one entry of an admitted directory.
///
/// The lock is affine: it cannot be cloned or copied, moving it transfers the
/// sole custody, and dropping it is the only release. The descriptor is
/// close-on-exec, so no spawned process inherits the exclusion.
///
/// Lock entry names share the admitted directory with pending-journal names
/// and must stay disjoint from them; that namespace discipline is cooperative
/// and belongs to the consumer.
///
/// Release is not instantaneous across a concurrent process spawn. A child
/// forked while the lock is held shares the underlying open file, so dropping
/// the holder releases the exclusion only once the child's close-on-exec
/// descriptor closes at `exec`. A holder that releases and immediately
/// reacquires during that window may observe [`LockError::Held`].
///
/// ```compile_fail
/// fn duplicate(lock: marrow_fs_journal::CacheLock) {
///     let _second = lock.clone();
/// }
/// ```
pub struct CacheLock {
    /// Held for custody alone: closing the descriptor on drop is the one
    /// release, so the field is never read.
    #[allow(dead_code)]
    pub(crate) handle: sys::FileHandle,
    pub(crate) identity: FsIdentity,
}

impl CacheLock {
    /// Acquire the lock on `name` inside `dir`, creating the lock entry if
    /// absent. A node of the wrong kind refuses with
    /// [`CustodyError::WrongNodeKind`]; a held lock refuses with
    /// [`LockError::Held`]; an entry whose owner bits deny read-write access
    /// refuses with [`CustodyError::ModeDenied`] naming the operator action;
    /// an entry whose identity drifted between locking and verification
    /// refuses with a typed custody error rather than holding an orphaned
    /// inode.
    pub fn acquire(dir: &AdmittedDir, name: &EntryName) -> Result<Self, LockError> {
        let handle = open_lock_entry(dir, name)?;
        // The node kind is classified on the opened handle before the lock is
        // attempted, because `flock` classifies no node kind: on Darwin it
        // refuses the one non-regular node this open accepts — a FIFO — with
        // the unsupported-semantics errno this crate reads as
        // `CustodyError::Unsupported`, so an acquisition that locked first
        // would report the platform's lock semantics rather than name the
        // planted node.
        let stat = sys::fstat_file(&handle)?;
        if stat.kind != NodeKind::Regular {
            return Err(LockError::Custody(CustodyError::WrongNodeKind {
                op: "lock",
                found: stat.kind,
            }));
        }
        if !sys::try_lock_exclusive(&handle)? {
            return Err(LockError::Held);
        }
        // The restore runs on a node already admitted as a regular file and
        // already locked, so a planted non-regular node and a contended entry
        // both keep the mode they carried.
        sys::restore_lock_mode(&handle)?;
        // The name must still map to the locked inode: without this recheck a
        // racing unlink-and-recreate would leave this holder excluding nobody
        // on an orphaned inode.
        match dir.stat_entry(name)? {
            Some(entry) if entry.identity() == stat.identity => Ok(Self {
                handle,
                identity: stat.identity,
            }),
            _ => Err(LockError::Custody(CustodyError::IdentityDrift {
                op: "lock",
            })),
        }
    }

    /// The locked entry's inode identity.
    pub fn identity(&self) -> FsIdentity {
        self.identity
    }
}

/// How many times a create-if-absent open may report absence before the
/// refusal is taken at face value. Each pass is one `openat`, so the bound is
/// the whole cost of the rendezvous.
const CREATION_RENDEZVOUS_PASSES: u32 = 8;

/// Open the lock entry, creating it when absent.
///
/// A create-if-absent open that reports absence is reporting a concurrent
/// creation of the same name rather than an absent entry: Darwin returns
/// `ENOENT` from `openat` while another thread or process is creating the
/// entry, and the name is already present by the time the refusal is read. The
/// first publication of a fresh clone is exactly that race — no checkout
/// carries the lock — so absence is retried a bounded number of times, and each
/// pass ends in the entry opening or in the holder being reported. Taking the
/// first refusal at face value would report a missing file for what is an
/// ordinary contended acquisition.
///
/// A lock entry left carrying a umask-stripped mode by a crash inside the
/// create-then-restore window is reopenable by no process those bits bind, so
/// its refusal is refined into the typed mode refusal that names the mode an
/// operator must restore. A process holding the mode-override capability
/// (`root`, or `CAP_DAC_OVERRIDE` on Linux) reopens it instead, and the restore
/// in [`CacheLock::acquire`] returns it to `0600`.
fn open_lock_entry(dir: &AdmittedDir, name: &EntryName) -> Result<sys::FileHandle, CustodyError> {
    let mut passes = 0;
    loop {
        match sys::open_lock_file(&dir.handle, name.as_str()) {
            Ok(handle) => return Ok(handle),
            Err(CustodyError::NotFound { .. }) if passes < CREATION_RENDEZVOUS_PASSES => {
                passes += 1;
            }
            Err(refusal) => {
                return Err(custody::refine_open_refusal(
                    refusal,
                    dir.observe(name),
                    custody::REQUIRED_RW,
                ));
            }
        }
    }
}

impl fmt::Debug for CacheLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheLock")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Why the cooperative lock could not be acquired.
#[derive(Debug)]
pub enum LockError {
    /// Another holder has the lock.
    Held,
    /// The lock entry could not be created, locked, or verified.
    Custody(CustodyError),
}

impl fmt::Display for LockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Held => formatter.write_str("the cooperative lock is already held"),
            Self::Custody(error) => write!(formatter, "the lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Held => None,
            Self::Custody(error) => Some(error),
        }
    }
}

impl From<CustodyError> for LockError {
    fn from(error: CustodyError) -> Self {
        Self::Custody(error)
    }
}
