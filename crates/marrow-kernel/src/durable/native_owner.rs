//! Opaque semantic ownership of one persistent native store.
//!
//! This capsule is the only persistent constructor in the path kernel. It keeps
//! the lower engine-and-lock owner inside the semantic [`DurableStore`], retains
//! the exact recovery scope, and performs reopen, audit, and witness
//! classification as one consuming operation.

use std::path::{Path, PathBuf};

use marrow_store::{ByteEngine, NativeEngineOwner, NativeOwnerOpenError, StoreError};

use super::session_host::SessionHost;
use super::store::{DurableStore, ReadSession, TxnSession};
use super::{
    CommitRecovery, CommitRecoveryScope, DemandCoverage, DurableCommitState, InvocationGrant,
    SessionError, SiteSpec, StoreSchema,
};

/// A persistent native store whose semantic handle, engine, and process owner
/// lock cannot be separated by safe dependents.
///
/// ```compile_fail
/// use marrow_kernel::durable::NativeStoreOwner;
/// fn detach(owner: NativeStoreOwner) {
///     let _semantic_store = owner.store;
/// }
/// ```
///
/// The former path-plus-instance constructor is absent; only the opaque owner
/// composition can mint a persistent recovery scope.
///
/// ```compile_fail
/// use marrow_kernel::durable::NativeStore;
/// fn raw_scoped_open() {
///     let _ = NativeStore::open_native_with_recovery_scope(
///         std::path::Path::new("store.redb"), Vec::new(), Vec::new(), [0; 16]
///     );
/// }
/// ```
pub struct NativeStoreOwner {
    store: Option<DurableStore<NativeEngineOwner>>,
    directory: PathBuf,
    instance: [u8; 16],
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
}

impl NativeStoreOwner {
    /// Create and stamp the engine artifact in a newly prepared store directory,
    /// returning no open store capability.
    pub fn provision(store_dir: &Path) -> Result<(), StoreError> {
        NativeEngineOwner::provision(store_dir)
    }

    /// Open an existing persistent store. The zero-capability callback runs
    /// while the lower owner lock is held and before the engine is opened.
    pub fn open_existing_admitted<R>(
        store_dir: &Path,
        instance: [u8; 16],
        schemas: Vec<StoreSchema>,
        sites: Vec<SiteSpec>,
        admit: impl FnOnce() -> Result<(), R>,
    ) -> Result<Self, NativeOwnerOpenError<R>> {
        let directory = std::fs::canonicalize(store_dir).map_err(NativeOwnerOpenError::Io)?;
        let engine = NativeEngineOwner::open_existing_admitted(&directory, instance, admit)?;
        let ceiling = DemandCoverage {
            read: true,
            write: engine.require_write_access("open").is_ok(),
        };
        let scope = CommitRecoveryScope::persistent(instance, &directory);
        let store = DurableStore::from_schemas_with_ceiling_and_recovery_scope(
            engine,
            schemas.clone(),
            sites.clone(),
            ceiling,
            scope,
        );
        Ok(Self {
            store: Some(store),
            directory,
            instance,
            schemas,
            sites,
        })
    }

    /// Consume an indeterminate commit fact, irreversibly quarantine the lower
    /// owner, reopen the existing engine under the retained lock, run a full
    /// audit, and classify the exact witness. Only a known result returns a
    /// usable owner, which remains quarantined until process exit.
    pub fn resolve_recovery(
        mut self,
        recovery: CommitRecovery,
    ) -> (DurableCommitState, Option<Self>) {
        let store = self
            .store
            .take()
            .expect("a live native owner retains its semantic store");
        let engine = match store.into_engine().reopen_existing_and_audit() {
            Ok(engine) => engine,
            Err(_) => return (DurableCommitState::Unknown, None),
        };
        let ceiling = DemandCoverage {
            read: true,
            write: engine.require_write_access("open").is_ok(),
        };
        let scope = CommitRecoveryScope::persistent(self.instance, &self.directory);
        let mut reopened = DurableStore::from_schemas_with_ceiling_and_recovery_scope(
            engine,
            self.schemas.clone(),
            self.sites.clone(),
            ceiling,
            scope,
        );
        let state = reopened.classify_recovery(recovery);
        if state == DurableCommitState::Unknown {
            return (state, None);
        }
        self.store = Some(reopened);
        (state, Some(self))
    }

    fn store_mut(&mut self) -> &mut DurableStore<NativeEngineOwner> {
        self.store
            .as_mut()
            .expect("a live native owner retains its semantic store")
    }
}

impl SessionHost for NativeStoreOwner {
    type Engine = NativeEngineOwner;

    fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, Self::Engine>, SessionError> {
        self.store_mut().read_session(grant, demand)
    }

    fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, Self::Engine>, SessionError> {
        self.store_mut().txn_session(grant, demand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::{CommitResult, Durable};
    use marrow_store::NativeLockError;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "marrow-native-store-owner-{tag}-{}-{nonce}",
                std::process::id(),
            ));
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn witness(generation: u128) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + std::mem::size_of::<u128>());
        bytes.push(0x01);
        bytes.extend_from_slice(&generation.to_be_bytes());
        bytes
    }

    fn open_owner(scratch: &Scratch, instance: [u8; 16]) -> NativeStoreOwner {
        NativeStoreOwner::provision(&scratch.0).expect("provision");
        NativeStoreOwner::open_existing_admitted(
            &scratch.0,
            instance,
            Vec::new(),
            Vec::new(),
            || Ok::<_, std::convert::Infallible>(()),
        )
        .expect("open native semantic owner")
    }

    fn assert_excluded(path: &Path, instance: [u8; 16]) {
        assert!(matches!(
            NativeEngineOwner::open_existing_admitted(path, instance, || {
                Ok::<_, std::convert::Infallible>(())
            }),
            Err(NativeOwnerOpenError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
    }

    #[test]
    fn known_recovery_returns_a_usable_owner_but_never_disarms_quarantine() {
        for (tag, seed_new, expected) in [
            ("known-old", false, DurableCommitState::KnownOld),
            ("known-new", true, DurableCommitState::KnownNew),
        ] {
            let scratch = Scratch::new(tag);
            let instance = if seed_new { [0x42; 16] } else { [0x41; 16] };
            let mut owner = open_owner(&scratch, instance);
            if seed_new {
                let mut txn = owner
                    .txn_session(
                        InvocationGrant::full_store(),
                        DemandCoverage {
                            read: true,
                            write: true,
                        },
                    )
                    .expect("open witness-seeding transaction");
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }

            let directory = std::fs::canonicalize(&scratch.0).expect("canonical scratch");
            let fact = CommitRecovery {
                scope: Some(CommitRecoveryScope::persistent(instance, &directory)),
                before: None,
                after: witness(0),
            };
            let (state, owner) = owner.resolve_recovery(fact);
            assert_eq!(state, expected);
            let mut owner = owner.expect("a known classification returns the owner");
            {
                let _read = owner
                    .read_session(
                        InvocationGrant::full_store(),
                        DemandCoverage {
                            read: true,
                            write: false,
                        },
                    )
                    .expect("a known recovered owner remains usable");
            }
            assert_excluded(&scratch.0, [0x43; 16]);
            drop(owner);
            assert_excluded(&scratch.0, [0x44; 16]);
        }
    }

    #[test]
    fn unknown_recovery_retires_the_owner_without_releasing_quarantine() {
        let scratch = Scratch::new("unknown");
        let instance = [0x45; 16];
        let owner = open_owner(&scratch, instance);
        let fact = CommitRecovery {
            scope: Some(CommitRecoveryScope::persistent(instance, "/wrong/store")),
            before: None,
            after: witness(0),
        };
        let (state, owner) = owner.resolve_recovery(fact);
        assert_eq!(state, DurableCommitState::Unknown);
        assert!(owner.is_none());
        assert_excluded(&scratch.0, [0x46; 16]);
    }

    #[test]
    fn generic_unscoped_store_drop_cannot_disarm_a_quarantined_lower_owner() {
        let scratch = Scratch::new("generic-drop");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let owner = NativeEngineOwner::open_existing_admitted(&scratch.0, [0x47; 16], || {
            Ok::<_, std::convert::Infallible>(())
        })
        .expect("open lower owner")
        .reopen_existing_and_audit()
        .expect("enter irreversible lower quarantine");
        let store = DurableStore::from_schemas_with_ceiling(
            owner,
            Vec::new(),
            Vec::new(),
            DemandCoverage {
                read: true,
                write: true,
            },
        );
        drop(store);
        assert_excluded(&scratch.0, [0x48; 16]);
    }
}
