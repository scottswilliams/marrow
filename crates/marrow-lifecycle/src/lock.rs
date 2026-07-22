//! Public diagnostic projection of the lower native-owner lock.
//!
//! Physical exclusion is owned by `marrow-store` and retained inside the
//! kernel's opaque native capsule. Lifecycle callers receive only these stable
//! diagnostic types; there is no lock acquisition, release, or re-arm API here.

use marrow_codes::Code;
use marrow_kernel::durable::{NativeLockError, NativeLockOwner};

use crate::instance::StoreInstanceId;

/// The live owner named by a store-lock contention diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockOwner {
    /// The owning process id.
    pub pid: u32,
    /// The lifecycle store instance held by that process.
    pub instance: StoreInstanceId,
    /// The acquisition time in Unix-epoch seconds. Forensic only.
    pub acquired_unix_secs: u64,
}

impl From<NativeLockOwner> for LockOwner {
    fn from(owner: NativeLockOwner) -> Self {
        Self {
            pid: owner.pid,
            instance: StoreInstanceId::from_bytes(owner.instance),
            acquired_unix_secs: owner.acquired_unix_secs,
        }
    }
}

/// Why the lower native-owner lock could not be acquired.
#[derive(Debug)]
pub enum LockError {
    /// Another live process owns the store.
    StoreInUse { owner: Option<LockOwner> },
    /// The lock file or directory could not be accessed.
    Io(std::io::Error),
}

impl From<NativeLockError> for LockError {
    fn from(error: NativeLockError) -> Self {
        match error {
            NativeLockError::StoreInUse { owner } => Self::StoreInUse {
                owner: owner.map(LockOwner::from),
            },
            NativeLockError::Io(error) => Self::Io(error),
        }
    }
}

impl LockError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            Self::StoreInUse { .. } => Code::StoreLocked.as_str(),
            Self::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreInUse { owner: Some(owner) } => write!(
                formatter,
                "the store is already open by process {} (store instance {}); close it, then retry",
                owner.pid,
                owner.instance.to_hex(),
            ),
            Self::StoreInUse { owner: None } => write!(
                formatter,
                "the store is already open by another process; close it, then retry",
            ),
            Self::Io(error) => write!(formatter, "the store lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_owner_identity_projects_without_changing_bytes() {
        let lower = NativeLockOwner {
            pid: 17,
            instance: [0xab; 16],
            acquired_unix_secs: 23,
        };
        let projected = LockOwner::from(lower);
        assert_eq!(projected.pid, 17);
        assert_eq!(projected.instance.bytes(), &[0xab; 16]);
        assert_eq!(projected.acquired_unix_secs, 23);
    }
}
