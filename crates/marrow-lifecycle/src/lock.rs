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
    /// The lifecycle store instance held by that process, once that process has
    /// bound it. A holder that has taken the lock but has not yet read the store
    /// directory it is opening has no instance to name, and the projection reports
    /// exactly that rather than inventing one.
    pub instance: Option<StoreInstanceId>,
    /// The acquisition time in Unix-epoch seconds. Forensic only.
    pub acquired_unix_secs: u64,
}

impl From<NativeLockOwner> for LockOwner {
    fn from(owner: NativeLockOwner) -> Self {
        Self {
            pid: owner.pid,
            instance: owner.instance.map(StoreInstanceId::from_bytes),
            acquired_unix_secs: owner.acquired_unix_secs,
        }
    }
}

/// Why the lower native-owner lock could not be acquired.
#[derive(Debug)]
pub enum LockError {
    /// Another live process owns the store.
    StoreInUse { owner: Option<LockOwner> },
    /// This process is denied the access taking the lock requires, so the lock was never
    /// asked for. Nothing about the store was established: a failure to reach the store
    /// directory or its lock entry is not an observation of either.
    AccessDenied(std::io::Error),
    /// The lock file or directory could not be accessed.
    Io(std::io::Error),
}

impl From<NativeLockError> for LockError {
    fn from(error: NativeLockError) -> Self {
        match error {
            NativeLockError::StoreInUse { owner } => Self::StoreInUse {
                owner: owner.map(LockOwner::from),
            },
            // The one place a lock failure is classified. A denial is its own state, so the
            // code below is a match on the state rather than a second reading of the error.
            NativeLockError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Self::AccessDenied(error)
            }
            NativeLockError::Io(error) => Self::Io(error),
        }
    }
}

impl LockError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            Self::StoreInUse { .. } => Code::StoreLocked.as_str(),
            Self::AccessDenied(_) => Code::StorePermissionDenied.as_str(),
            Self::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreInUse {
                owner:
                    Some(LockOwner {
                        pid,
                        instance: Some(instance),
                        ..
                    }),
            } => write!(
                formatter,
                "the store is already open by process {pid} (store instance {}); close it, then \
                 retry",
                instance.to_hex(),
            ),
            Self::StoreInUse { owner: Some(owner) } => write!(
                formatter,
                "the store is already open by process {}; close it, then retry",
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

impl std::error::Error for LockError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_owner_identity_projects_without_changing_bytes() {
        let lower = NativeLockOwner {
            pid: 17,
            instance: Some([0xab; 16]),
            acquired_unix_secs: 23,
        };
        let projected = LockOwner::from(lower);
        assert_eq!(projected.pid, 17);
        assert_eq!(
            projected
                .instance
                .expect("a bound owner names its store")
                .bytes(),
            &[0xab; 16],
        );
        assert_eq!(projected.acquired_unix_secs, 23);
    }

    /// A holder that has taken the lock but not yet bound its store projects with no
    /// instance, and its diagnostic names the process without claiming a store.
    #[test]
    fn an_unbound_owner_projects_and_renders_without_an_instance() {
        let projected = LockOwner::from(NativeLockOwner {
            pid: 19,
            instance: None,
            acquired_unix_secs: 5,
        });
        assert_eq!(projected.instance, None);
        let rendered = LockError::StoreInUse {
            owner: Some(projected),
        }
        .to_string();
        assert!(rendered.contains("process 19"), "{rendered}");
        assert!(
            !rendered.contains("store instance"),
            "an unbound holder must not be rendered as naming a store: {rendered}",
        );
    }
}
