//! The cooperative cache lock: affine, non-clone, close-on-exec.

use std::fmt;

use crate::custody::{
    AdmittedDir, CustodyError, CustodyOp, FsIdentity, LockAcquisition, OpenedFile,
};
use crate::entry::EntryName;

/// An exclusively held cooperative lock on one entry of an admitted directory.
///
/// The lock is affine: it derives neither `Clone` nor `Copy`, moving it
/// transfers the sole custody, and dropping it is the only release. The
/// descriptor is close-on-exec, so no spawned process inherits the exclusion.
/// Lock entry names share the admitted directory with pending-journal names
/// and must stay disjoint from them; that discipline belongs to the consumer.
///
/// `flock` is held by an open file description, not a process or thread: a
/// child that inherits the descriptor across `fork` shares the same hold and is
/// inside the exclusion, while a fresh open in that child contends like any
/// other process would, and threads sharing one holder are not serialized by
/// it. Release therefore completes only once a concurrently forked child's
/// descriptor closes at `exec`; a holder that drops and immediately reacquires
/// inside that window may observe [`LockError::Held`].
pub struct CacheLock {
    /// Held for custody alone: closing the descriptor on drop is the one
    /// release, so the field is never read.
    #[allow(dead_code)]
    file: OpenedFile,
    identity: FsIdentity,
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
        let file = dir.open_or_create_lock_entry(name)?;
        if file.try_lock_exclusive()? == LockAcquisition::Held {
            return Err(LockError::Held);
        }
        // Restored only after the witness and the lock, so a refused node keeps
        // the mode it carried.
        file.restore_lock_mode()?;
        // The name must still map to the locked inode: without this recheck a
        // racing unlink-and-recreate would leave this holder excluding nobody
        // on an orphaned inode.
        dir.reassert(name, file.identity(), CustodyOp::Lock)?;
        Ok(Self {
            identity: file.identity(),
            file,
        })
    }

    /// The locked entry's inode identity.
    pub fn identity(&self) -> FsIdentity {
        self.identity
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
