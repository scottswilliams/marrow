//! Opaque semantic ownership of one persistent native store.
//!
//! This capsule is the only persistent constructor in the path kernel. It keeps
//! the lower engine-and-lock owner inside the semantic [`DurableStore`], retains
//! the exact recovery scope, and performs reopen, audit, and witness
//! classification as one consuming operation.

use std::path::{Path, PathBuf};

use marrow_store::{
    ByteEngine, NativeEngineOwner, NativeOpenAccess, NativeOwnerAcquireError, NativeOwnerOpenError,
    NativePromotionRefusal, PendingNativeEngineOwner, StoreError, StoreOp,
};

use super::audit::{AuditReport, ContentDigest, ExportError, ExportSink};
use super::session_host::SessionHost;
use super::store::{DurableStore, ReadSession, TxnSession};
use super::{
    CommitRecovery, CommitRecoveryScope, DemandCoverage, DurableCommitState, InvocationGrant,
    NumberedProjection, SessionError,
};

/// A persistent native store whose semantic handle, engine, and process owner
/// lock cannot be separated by safe dependents.
///
/// Only the opaque owner composition can mint a persistent recovery scope; there is
/// no path-plus-instance constructor.
///
/// ```compile_fail
/// use marrow_kernel::durable::NativeStore;
/// fn raw_scoped_open() {
///     let _ = NativeStore::open_native_with_recovery_scope(
///         std::path::Path::new("store.redb"), Vec::new(), [0; 16]
///     );
/// }
/// ```
pub struct NativeStoreOwner {
    store: Option<DurableStore<NativeEngineOwner>>,
    directory: PathBuf,
    instance: [u8; 16],
}

/// Compose the semantic store over a freshly opened engine: the ceiling the
/// engine's own write access admits, and a recovery scope naming exactly the
/// instance and directory this owner holds.
fn bind_store(
    engine: NativeEngineOwner,
    layout: NumberedProjection,
    instance: [u8; 16],
    directory: &Path,
) -> DurableStore<NativeEngineOwner> {
    let ceiling = DemandCoverage {
        read: true,
        write: engine.require_write_access(StoreOp::Open).is_ok(),
    };
    let scope = CommitRecoveryScope::persistent(instance, directory);
    DurableStore::from_numbered_with_ceiling_and_recovery_scope(engine, layout, ceiling, scope)
}

impl NativeStoreOwner {
    /// The semantic store this owner holds.
    fn store(&self) -> &DurableStore<NativeEngineOwner> {
        self.store
            .as_ref()
            .expect("a live native owner retains its semantic store")
    }

    /// Take the semantic store out for a consuming reopen.
    fn take_store(&mut self) -> DurableStore<NativeEngineOwner> {
        self.store
            .take()
            .expect("a live native owner retains its semantic store")
    }

    /// Consume read-only access into service without replacing the accepted
    /// layout or releasing the directory owner. Unchanged admitted bytes take
    /// the saved allocator path; external same-inode mutation is not detected.
    pub fn into_service(mut self) -> Result<Self, NativeOwnerOpenError<NativePromotionRefusal>> {
        let (engine, layout) = self.take_store().into_parts();
        let engine = engine.into_service(self.instance)?;
        self.store = Some(bind_store(engine, layout, self.instance, &self.directory));
        Ok(self)
    }

    /// Create and stamp the engine artifact in a newly prepared store directory,
    /// returning no open store capability.
    pub fn provision(store_dir: &Path) -> Result<(), StoreError> {
        NativeEngineOwner::provision(store_dir)
    }

    /// Take the lower owner lock over an existing persistent store, making no
    /// engine call and naming no store instance. The caller reads the store
    /// directory's own artifacts under the returned owner and binds the instance
    /// those artifacts name.
    pub fn acquire_existing(
        store_dir: &Path,
    ) -> Result<PendingNativeStoreOwner, NativeOwnerAcquireError> {
        NativeEngineOwner::acquire_existing(store_dir)
            .map(|pending| PendingNativeStoreOwner { pending })
    }

    /// Consume an indeterminate commit fact, irreversibly quarantine the lower
    /// owner, reopen the existing engine under the retained lock, run a full
    /// audit, and classify the exact witness. Only a known result returns a
    /// usable owner, which remains quarantined until process exit.
    pub fn resolve_recovery(
        mut self,
        recovery: CommitRecovery,
    ) -> (DurableCommitState, Option<Self>) {
        let (engine, layout) = self.take_store().into_parts();
        let engine = match engine.reopen_existing_and_audit() {
            Ok(engine) => engine,
            Err(_) => return (DurableCommitState::Unknown, None),
        };
        let mut reopened = bind_store(engine, layout, self.instance, &self.directory);
        let state = reopened.classify_recovery(recovery);
        if state == DurableCommitState::Unknown {
            return (state, None);
        }
        self.store = Some(reopened);
        (state, Some(self))
    }

    /// The bounded read-only logical walk of [`DurableStore::logical_audit`] over this
    /// store, under the retained owner lock and without a session.
    pub fn logical_audit(
        &self,
        digest: &mut dyn ContentDigest,
    ) -> Result<AuditReport, SessionError> {
        self.store().logical_audit(digest)
    }

    fn store_mut(&mut self) -> &mut DurableStore<NativeEngineOwner> {
        self.store
            .as_mut()
            .expect("a live native owner retains its semantic store")
    }

    /// Stream provisional transfer cells while retaining this store's owner.
    /// Output is usable only after the complete report is clean.
    pub fn export_cells(
        &self,
        digest: &mut dyn ContentDigest,
        sink: &mut dyn ExportSink,
    ) -> Result<AuditReport, ExportError> {
        self.store().export_cells(digest, sink)
    }
}

/// One persistent store's owner lock, held before the store directory has been
/// read and before any engine call. It is affine: binding it consumes it, and no
/// engine, session, or recovery scope exists until it has been.
///
/// The lower pending owner is private and cannot be detached by safe dependents.
pub struct PendingNativeStoreOwner {
    pending: PendingNativeEngineOwner,
}

/// Opening a private construction body or populating it failed.
#[derive(Debug)]
pub enum NativeRestoreError<A, R> {
    Open(NativeOwnerOpenError<A>),
    Restore(super::RestoreError<R>),
}

impl PendingNativeStoreOwner {
    /// Construct an empty, privately staged body under its retained owner.
    /// The lifecycle caller admits the intended head in memory and keeps its
    /// Head artifact absent until this operation succeeds. `next` reports None
    /// only after complete transfer framing has been validated.
    ///
    /// No usable owner escapes on input, commit or audit failure. In particular,
    /// an indeterminate native commit retains the lower owner's quarantine.
    pub fn restore<A, R>(
        self,
        instance: [u8; 16],
        admit: impl FnOnce() -> Result<NumberedProjection, A>,
        next: impl FnMut() -> Result<Option<super::Cell>, R>,
        digest: &mut dyn ContentDigest,
    ) -> Result<(NativeStoreOwner, AuditReport), NativeRestoreError<A, R>> {
        let mut owner = self
            .bind_and_open_existing(NativeOpenAccess::ReadWrite, instance, admit)
            .map_err(NativeRestoreError::Open)?;
        let store = owner.store.take().expect("new owner retains its store");
        let (store, report) = store
            .restore(next, digest)
            .map_err(NativeRestoreError::Restore)?;
        owner.store = Some(store);
        Ok((owner, report))
    }

    /// Metadata from the lower owner's retained, locked directory node.
    pub fn directory_metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.pending.directory_metadata()
    }

    /// The canonical store directory this owner holds.
    pub fn directory(&self) -> &Path {
        self.pending.directory()
    }

    /// Run the zero-capability admission callback and open with the requested access
    /// under the same lock. Mutable access publishes `instance` in the marker;
    /// read-only access preserves its bytes and absence. Only
    /// service or explicit recovery access may discharge a physical-audit obligation. The
    /// recovery scope is minted here, from the instance the caller bound and the
    /// directory the lock was taken over, so no scope can name a store this owner
    /// does not hold.
    pub fn bind_and_open_existing<R>(
        self,
        access: NativeOpenAccess,
        instance: [u8; 16],
        admit: impl FnOnce() -> Result<NumberedProjection, R>,
    ) -> Result<NativeStoreOwner, NativeOwnerOpenError<R>> {
        let directory = self.pending.directory().to_path_buf();
        let mut layout = None;
        let engine = self.pending.bind_and_open_existing(access, instance, || {
            layout = Some(admit()?);
            Ok(())
        })?;
        let store = bind_store(
            engine,
            layout.expect("successful engine open completed admission"),
            instance,
            &directory,
        );
        Ok(NativeStoreOwner {
            store: Some(store),
            directory,
            instance,
        })
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
    use crate::codec::key::KeyScalar;
    use crate::codec::value::{RuntimeScalar, ScalarKind};
    use crate::durable::{
        CommitResult, Durable, EntryValue, SiteTarget, StoreProjection, StoreSchemaBuilder,
    };
    use crate::equality::ValueDomain;
    use crate::test_common::Scratch;
    use marrow_store::NativeLockError;

    fn witness(generation: u128) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + std::mem::size_of::<u128>());
        bytes.push(0x01);
        bytes.extend_from_slice(&generation.to_be_bytes());
        bytes
    }

    fn open_owner(scratch: &Scratch, instance: [u8; 16]) -> NativeStoreOwner {
        let mut schema = StoreSchemaBuilder::root("values", vec![ScalarKind::Int]);
        schema.scalar_field("value", ScalarKind::Int, true);
        let mut projection = StoreProjection::builder();
        projection.root(schema.finish().expect("bounded root"));
        projection.site(0, SiteTarget::whole_payload());
        projection.site(0, SiteTarget::field_leaf(0));
        let layout = NumberedProjection::accepted(
            projection.finish().expect("valid sites"),
            &[7, 70_000],
            70_001,
        )
        .expect("accepted sparse addresses");
        NativeStoreOwner::provision(scratch.path()).expect("provision");
        NativeStoreOwner::acquire_existing(scratch.path())
            .expect("acquire the owner lock")
            .bind_and_open_existing(NativeOpenAccess::ReadWrite, instance, || {
                Ok::<_, std::convert::Infallible>(layout)
            })
            .expect("open native semantic owner")
    }

    fn assert_excluded(path: &Path) {
        assert!(matches!(
            NativeEngineOwner::acquire_existing(path),
            Err(NativeOwnerAcquireError::Lock(
                NativeLockError::StoreInUse { .. }
            ))
        ));
    }

    #[test]
    fn empty_restore_retains_owner_and_metadata_input_returns_no_owner() {
        struct Digest;
        impl ContentDigest for Digest {
            fn absorb(&mut self, _: &[u8], _: &[u8]) {}
        }
        let scratch = Scratch::new("native-store-owner-restore-complete");
        NativeStoreOwner::provision(scratch.path()).unwrap();
        let result = NativeStoreOwner::acquire_existing(scratch.path())
            .unwrap()
            .restore(
                [0x51; 16],
                || {
                    Ok::<_, ()>(NumberedProjection::fresh(
                        StoreProjection::builder().finish().unwrap(),
                    ))
                },
                || Ok::<_, ()>(None),
                &mut Digest,
            );
        let (owner, report) = match result {
            Ok(result) => result,
            Err(error) => panic!("restore failed: {error:?}"),
        };
        assert!(report.is_clean());
        assert_excluded(scratch.path());
        drop(owner);
        let mut cell = Some((
            super::super::physical::meta_key(super::super::store::WITNESS),
            witness(0),
        ));
        let result = NativeStoreOwner::acquire_existing(scratch.path())
            .unwrap()
            .restore(
                [0x51; 16],
                || {
                    Ok::<_, ()>(NumberedProjection::fresh(
                        StoreProjection::builder().finish().unwrap(),
                    ))
                },
                || Ok::<_, ()>(cell.take()),
                &mut Digest,
            );
        assert!(matches!(
            result,
            Err(NativeRestoreError::Restore(
                super::super::RestoreError::OutsideNamespace
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
            {
                let mut txn = owner
                    .txn_session(
                        InvocationGrant::full_store(),
                        DemandCoverage {
                            read: true,
                            write: true,
                        },
                    )
                    .expect("populate before recovery");
                txn.create_entry(
                    &txn.site(0),
                    &[KeyScalar::Int(7)],
                    EntryValue {
                        groups: Vec::new(),
                        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(42)))],
                    },
                )
                .expect("write accepted address");
                assert!(matches!(txn.commit(), CommitResult::Committed));
            }
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

            let directory = std::fs::canonicalize(scratch.path()).expect("canonical scratch");
            let fact = CommitRecovery {
                scope: Some(CommitRecoveryScope::persistent(instance, &directory)),
                before: Some(witness(0)),
                after: witness(1),
            };
            let (state, owner) = owner.resolve_recovery(fact);
            assert_eq!(state, expected);
            let mut owner = owner.expect("a known classification returns the owner");
            {
                let mut read = owner
                    .read_session(
                        InvocationGrant::full_store(),
                        DemandCoverage {
                            read: true,
                            write: false,
                        },
                    )
                    .expect("a known recovered owner remains usable");
                assert_eq!(
                    read.read_field(&read.site(1), &[KeyScalar::Int(7)]),
                    Ok(Some(ValueDomain::Scalar(RuntimeScalar::Int(42))))
                );
            }
            assert_excluded(scratch.path());
            drop(owner);
            assert_excluded(scratch.path());
        }
    }

    #[test]
    fn unknown_recovery_retires_the_owner_without_releasing_quarantine() {
        let scratch = Scratch::new("native-store-owner-unknown");
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
        assert_excluded(scratch.path());
    }

    #[test]
    fn generic_unscoped_store_drop_cannot_disarm_a_quarantined_lower_owner() {
        let scratch = Scratch::new("native-store-owner-generic-drop");
        NativeEngineOwner::provision(scratch.path()).expect("provision");
        let owner = NativeEngineOwner::acquire_existing(scratch.path())
            .expect("acquire the owner lock")
            .bind_and_open_existing(NativeOpenAccess::ReadWrite, [0x47; 16], || {
                Ok::<_, std::convert::Infallible>(())
            })
            .expect("open lower owner")
            .reopen_existing_and_audit()
            .expect("enter irreversible lower quarantine");
        let store = DurableStore::from_projection_with_ceiling(
            owner,
            StoreProjection::builder()
                .finish()
                .expect("a rootless projection has no site to resolve"),
            DemandCoverage {
                read: true,
                write: true,
            },
        );
        drop(store);
        assert_excluded(scratch.path());
    }
}
