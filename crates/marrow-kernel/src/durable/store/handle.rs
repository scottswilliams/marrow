//! The durable store handle: session opening over a coherent read view or a write
//! transaction after resolving effective authority, plus checked witness-generation minting.

use std::collections::BTreeMap;

use marrow_store::{ByteEngine, ReadView, StoreError};

use super::super::physical;
use super::super::{
    AuthorizedSite, CommitRecovery, CommitRecoveryScope, DemandCoverage, Denied,
    DurableCommitState, IndexSchema, InvocationGrant, RootNumbering, SessionError, SiteSpec,
    StoreSchema, number_store,
};
use super::resolve::resolve_site;
use super::{ReadSession, TxnSession};

/// The durable store handle. CLI-only caller at T01; dies at D00.
pub struct DurableStore<E: ByteEngine> {
    pub(super) engine: E,
    /// The store's roots by declaration position: one [`StoreSchema`] per durable root,
    /// each with its own id-keyed physical cell family. A site's `root` indexes this
    /// table. One engine transaction spans every root, so a cross-root write commits or
    /// rolls back as one unit.
    schemas: Vec<StoreSchema>,
    /// The store-local cell-key numbering of every root's durable nodes (FR01 §3), computed
    /// once from `schemas` at construction and parallel to it. The site resolver walks a
    /// root's [`RootNumbering`] in lockstep with its schema to number every addressed node,
    /// so cell keys are keyed by number, never by source spelling.
    numbering: Vec<RootNumbering>,
    sites: Vec<SiteSpec>,
    /// The store's deployment ceiling: the read/write coverage this handle admits,
    /// intersected with each invocation's grant before the first engine call. For a
    /// native handle it is the handle's own write capability; an ephemeral
    /// attachment supplies an explicit coverage bounded by its image demand union,
    /// so a read-only union cannot open a write session even over a writable engine.
    ceiling: DemandCoverage,
    /// The lifecycle-owned identity/path scope of a persistent attachment. Direct test and
    /// ephemeral handles are unscoped; because the memory engine never reports an
    /// indeterminate commit, they never need cross-reopen classification. An unscoped fact
    /// resolves only to `Unknown` rather than being allowed to classify another handle.
    recovery_scope: Option<CommitRecoveryScope>,
    poisoned: bool,
}

impl<E: ByteEngine> DurableStore<E> {
    /// Build a single-root store over an already-open engine, minting the store ceiling
    /// from the handle's write capability. The native/tracer caller; an ephemeral
    /// attachment uses [`Self::from_schemas_with_ceiling`] to bound the ceiling by image
    /// demand and to carry every root of a multi-root image.
    pub fn from_engine(engine: E, schema: StoreSchema, sites: Vec<SiteSpec>) -> Self {
        let ceiling = DemandCoverage {
            read: true,
            write: engine.require_write_access("open").is_ok(),
        };
        Self::from_schemas_with_ceiling(engine, vec![schema], sites, ceiling)
    }

    /// Build a store over an already-open engine from the image's root-indexed schema
    /// table and an explicit deployment ceiling. The ephemeral-attachment caller bounds
    /// the ceiling by the image's demand union, so authority never exceeds what the
    /// compiler described even when the backing engine is unconditionally writable. A
    /// site's `root` indexes `schemas`; every root shares this one engine, so a
    /// transaction spanning several roots commits atomically.
    pub fn from_schemas_with_ceiling(
        engine: E,
        schemas: Vec<StoreSchema>,
        sites: Vec<SiteSpec>,
        ceiling: DemandCoverage,
    ) -> Self {
        Self::from_schemas_with_optional_recovery_scope(engine, schemas, sites, ceiling, None)
    }

    /// Build a persistent store handle bound to the lifecycle-owned recovery scope.
    pub(crate) fn from_schemas_with_ceiling_and_recovery_scope(
        engine: E,
        schemas: Vec<StoreSchema>,
        sites: Vec<SiteSpec>,
        ceiling: DemandCoverage,
        recovery_scope: CommitRecoveryScope,
    ) -> Self {
        Self::from_schemas_with_optional_recovery_scope(
            engine,
            schemas,
            sites,
            ceiling,
            Some(recovery_scope),
        )
    }

    fn from_schemas_with_optional_recovery_scope(
        engine: E,
        schemas: Vec<StoreSchema>,
        sites: Vec<SiteSpec>,
        ceiling: DemandCoverage,
        recovery_scope: Option<CommitRecoveryScope>,
    ) -> Self {
        let numbering = number_store(&schemas);
        Self {
            engine,
            schemas,
            numbering,
            sites,
            ceiling,
            recovery_scope,
            poisoned: false,
        }
    }

    /// Consume the sole recovery fact and classify this handle's exact witness-cell state.
    /// Equality with the proposed after-state proves `KnownNew`; equality with the captured
    /// before-state proves `KnownOld`. A scope mismatch, third state, or read failure is
    /// `Unknown`. No witness bytes escape this owner.
    pub fn resolve_recovery(&mut self, recovery: CommitRecovery) -> DurableCommitState {
        // The engine verdict was indeterminate on the poisoned handle itself. Only a newly
        // opened handle can provide an independent durable observation; consulting the old
        // engine could merely read its cached post-commit view and mistake it for persistence.
        if self.poisoned {
            return DurableCommitState::Unknown;
        }
        if self.recovery_scope.is_none() || self.recovery_scope != recovery.scope {
            self.poisoned = true;
            return DurableCommitState::Unknown;
        }
        let current = match self
            .engine
            .read_view()
            .and_then(|view| view.get(&physical::meta_key(WITNESS)))
        {
            Ok(current) => current,
            Err(_) => {
                self.poisoned = true;
                return DurableCommitState::Unknown;
            }
        };
        let state = if current.as_ref() == Some(&recovery.after) {
            DurableCommitState::KnownNew
        } else if current == recovery.before {
            DurableCommitState::KnownOld
        } else {
            DurableCommitState::Unknown
        };
        self.poisoned = state == DurableCommitState::Unknown;
        state
    }

    /// Run one full engine integrity audit — a complete structural walk verifying every
    /// stored checksum ([`ByteEngine::audit_integrity`](marrow_store::ByteEngine::audit_integrity)).
    /// The lifecycle runs this on an unclean open (the crash-recovery path). It covers
    /// crash-path corruption only: the fast open path does not re-verify page checksums, so
    /// an externally flipped bit in a cleanly-closed store stays undetected here until the
    /// FR01 data-root digest is populated at a later full-walk operation. The in-memory
    /// engine has no durable substrate and passes trivially.
    pub fn audit(&mut self) -> Result<(), StoreError> {
        self.engine.audit_integrity()
    }

    /// Whether an indeterminate commit still poisons this handle. Lifecycle owners use
    /// this read-only latch when their inseparable owner lock drops: losing the affine
    /// recovery fact must preserve the durable unclean marker rather than record a clean
    /// shutdown. It exposes no witness material and grants no recovery authority.
    pub fn has_unresolved_recovery(&self) -> bool {
        self.poisoned
    }

    /// Refuse a session open on a poisoned handle. An earlier indeterminate commit sets
    /// the latch (its durability is unknown), and the handle then refuses every further
    /// read or write until the opaque recovery fact is resolved against a freshly opened
    /// store. Consulted at open on both session paths so a poisoned handle never opens a
    /// view or transaction that would observe an indeterminate state. Reachable only on a
    /// native handle whose engine can report
    /// [`CommitOutcome::Indeterminate`](marrow_store::CommitOutcome); the ephemeral memory
    /// engine always confirms, so its handle is never poisoned.
    fn check_poison(&self) -> Result<(), SessionError> {
        if self.poisoned {
            Err(SessionError::Poisoned)
        } else {
            Ok(())
        }
    }

    fn authorized_sites(&self) -> Vec<AuthorizedSite> {
        self.sites
            .iter()
            .map(|site| {
                resolve_site(
                    &self.schemas[site.root as usize],
                    &self.numbering[site.root as usize],
                    site.root,
                    &site.target,
                )
            })
            .collect()
    }

    /// Open a read session over a coherent read view after resolving effective
    /// authority. The view is bound to the session's borrow of the store, so its reads
    /// observe one version for the whole call.
    pub fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, E>, SessionError> {
        // Poison dominates authority: a store whose last commit was indeterminate is in an
        // unknown state, so it refuses every session with `Poisoned` until reopen
        // classification — before an authorization verdict, which cannot be meaningful over an
        // unknown state.
        self.check_poison()?;
        resolve_authority(demand, self.ceiling, grant).map_err(|Denied| SessionError::Denied)?;
        let auth = self.authorized_sites();
        let view = self.engine.read_view().map_err(SessionError::Engine)?;
        Ok(ReadSession { view, auth })
    }

    /// Open a transaction session after resolving effective authority. Schema-binding
    /// consistency is owned by the lifecycle head (F02a provision), not an in-store profile
    /// cell: the id-keyed layout makes a rename zero-cell, so no name-keyed profile
    /// descriptor is written or revalidated here.
    pub fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, E>, SessionError> {
        // Poison dominates authority: a store whose last commit was indeterminate is in an
        // unknown state, so it refuses every session with `Poisoned` until reopen
        // classification — before an authorization verdict, which cannot be meaningful over an
        // unknown state.
        self.check_poison()?;
        resolve_authority(demand, self.ceiling, grant).map_err(|Denied| SessionError::Denied)?;
        let auth = self.authorized_sites();
        // Per-root managed indexes: a write to root R maintains exactly `indexes[R]`, so a
        // cross-root transaction keeps each root's own indexes coherent without confusing
        // one root's index cells with another's.
        let indexes: Vec<Vec<IndexSchema>> = self
            .schemas
            .iter()
            .map(|schema| schema.indexes.clone())
            .collect();
        // Capture the exact before-state and derive the next tagged generation before
        // beginning the write transaction. `&mut self` plus the lifecycle's owner lock means
        // no other session can advance the witness between this read and `begin`.
        let before = self
            .engine
            .read_view()
            .and_then(|view| view.get(&physical::meta_key(WITNESS)))
            .map_err(SessionError::Engine)?;
        let after = next_witness(&before).map_err(SessionError::Engine)?;
        // Split the store's fields into disjoint borrows: the transaction borrows the
        // engine mutably while the session still writes the poison flag.
        let recovery_scope = self.recovery_scope.clone();
        let Self {
            engine, poisoned, ..
        } = self;
        let txn = engine.begin().map_err(SessionError::Engine)?;
        Ok(TxnSession {
            txn: Some(txn),
            poisoned,
            auth,
            recovery: Some(super::txn_session::RecoveryIntent {
                scope: recovery_scope,
                before,
                after,
            }),
            indexes,
            pending: BTreeMap::new(),
        })
    }
}

/// The witness meta-cell name in the `0x10` family.
pub(super) const WITNESS: &str = "witness";

/// Resolve effective authority: `demand ⊆ ceiling ∩ grant`. Demand never grants; it
/// is only checked. Each coverage atom the demand requires must be permitted by both
/// the deployment ceiling and the invocation grant.
fn resolve_authority(
    demand: DemandCoverage,
    ceiling: DemandCoverage,
    grant: InvocationGrant,
) -> Result<(), Denied> {
    let read_ok = !demand.read || (ceiling.read && grant.read);
    let write_ok = !demand.write || (ceiling.write && grant.write);
    if read_ok && write_ok {
        Ok(())
    } else {
        Err(Denied)
    }
}

/// The new witness domain is one byte longer than every legacy 16-byte token. The tag
/// selects the checked big-endian generation encoding; no legacy bytes are interpreted as
/// an integer.
const WITNESS_VERSION: u8 = 0x01;
const WITNESS_V1_BYTES: usize = 1 + std::mem::size_of::<u128>();

fn next_witness(before: &Option<Vec<u8>>) -> Result<Vec<u8>, StoreError> {
    let generation = match before.as_deref() {
        None => 0,
        // Every exact 16-byte value belongs to the legacy opaque domain. Migration starts
        // the tagged domain at zero while retaining those exact bytes as the before-state.
        Some(bytes) if bytes.len() == std::mem::size_of::<u128>() => 0,
        Some(bytes) if bytes.len() == WITNESS_V1_BYTES && bytes[0] == WITNESS_VERSION => {
            let current = u128::from_be_bytes(
                bytes[1..]
                    .try_into()
                    .expect("the exact v1 witness length was checked"),
            );
            current.checked_add(1).ok_or(StoreError::LimitExceeded {
                limit: "commit witness generation",
            })?
        }
        Some(_) => {
            return Err(StoreError::Corruption {
                message: "the commit witness cell has an unknown encoding".to_string(),
            });
        }
    };
    let mut witness = Vec::with_capacity(WITNESS_V1_BYTES);
    witness.push(WITNESS_VERSION);
    witness.extend_from_slice(&generation.to_be_bytes());
    Ok(witness)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell as Flag;
    use std::rc::Rc;

    use marrow_store::{ByteEngine, CommitOutcome, MemoryEngine, StoreError, WriteTxn};

    use super::*;
    use crate::codec::value::ScalarKind;
    use crate::durable::{
        CommitResult, Durable, FieldSchema, InvocationGrant, SiteSpec, SiteTarget,
    };

    fn store_with_engine<E: ByteEngine>(engine: E) -> DurableStore<E> {
        let schema = StoreSchema {
            root_name: "counters".into(),
            key: vec![ScalarKind::Int],
            fields: vec![FieldSchema::scalar("value", ScalarKind::Int, true)],
            branches: Vec::new(),
            groups: Vec::new(),
            indexes: Vec::new(),
        };
        let sites = vec![SiteSpec {
            root: 0,
            target: SiteTarget::WholePayload,
        }];
        DurableStore::from_schemas_with_ceiling(
            engine,
            vec![schema],
            sites,
            DemandCoverage {
                read: true,
                write: true,
            },
        )
    }

    fn store() -> DurableStore<MemoryEngine> {
        store_with_engine(MemoryEngine::new())
    }

    fn scoped_store(path: &str) -> DurableStore<MemoryEngine> {
        let mut store = store();
        store.recovery_scope = Some(CommitRecoveryScope::persistent([0x41; 16], path));
        store
    }

    fn witness(generation: u128) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(WITNESS_V1_BYTES);
        bytes.push(WITNESS_VERSION);
        bytes.extend_from_slice(&generation.to_be_bytes());
        bytes
    }

    fn seed_witness<E: ByteEngine>(store: &mut DurableStore<E>, value: Vec<u8>) {
        let mut txn = store.engine.begin().expect("begin seed");
        txn.put(&physical::meta_key(WITNESS), value)
            .expect("put seed");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }

    fn current_witness<E: ByteEngine>(store: &DurableStore<E>) -> Option<Vec<u8>> {
        store
            .engine
            .read_view()
            .expect("read view")
            .get(&physical::meta_key(WITNESS))
            .expect("get witness")
    }

    fn commit_empty<E: ByteEngine>(store: &mut DurableStore<E>) -> CommitResult {
        let mut txn = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("transaction session");
        txn.commit()
    }

    /// The E02 poison-latch consult at session open: a poisoned handle refuses every
    /// further read and write session with [`SessionError::Poisoned`], before any view or
    /// transaction opens, until the store is reopened and the interrupted commit
    /// reclassified. The latch is set here directly because the ephemeral memory engine
    /// never reports an indeterminate commit (the state is reachable only on the native
    /// path), so this owner-local unit test drives the consult the persistent lifecycle
    /// relies on.
    #[test]
    fn a_poisoned_handle_refuses_every_session_open() {
        let mut store = store();
        let grant = InvocationGrant::full_store();
        let demand = DemandCoverage {
            read: true,
            write: true,
        };

        // A healthy handle opens both sessions.
        assert!(store.read_session(grant, demand).is_ok());
        assert!(store.txn_session(grant, demand).is_ok());

        // Poison the latch as an indeterminate commit would.
        store.poisoned = true;

        assert!(
            matches!(
                store.read_session(grant, demand),
                Err(SessionError::Poisoned)
            ),
            "a read session must refuse on a poisoned handle",
        );
        assert!(
            matches!(
                store.txn_session(grant, demand),
                Err(SessionError::Poisoned)
            ),
            "a transaction session must refuse on a poisoned handle",
        );
    }

    /// The poison consult runs before the engine is touched: a poisoned handle refuses the
    /// transaction open before `begin`, so no cell — not even the witness — is written.
    #[test]
    fn the_poison_consult_precedes_any_engine_write() {
        let mut store = store();
        store.poisoned = true;
        let grant = InvocationGrant::full_store();
        let demand = DemandCoverage {
            read: true,
            write: true,
        };
        assert!(matches!(
            store.txn_session(grant, demand),
            Err(SessionError::Poisoned)
        ));
        // Nothing was written: the refusal short-circuited before `begin`.
        assert_eq!(
            store
                .engine
                .read_view()
                .expect("read view")
                .get(&physical::meta_key(WITNESS))
                .expect("get witness"),
            None,
        );
    }

    #[test]
    fn every_legacy_token_migrates_to_the_disjoint_zero_generation() {
        for legacy in [[0x00; 16], [u8::MAX; 16], [0x01; 16]] {
            let mut store = scoped_store("/test/legacy");
            seed_witness(&mut store, legacy.to_vec());

            assert!(matches!(commit_empty(&mut store), CommitResult::Committed));
            assert_eq!(current_witness(&store), Some(witness(0)));
            assert_eq!(current_witness(&store).expect("witness").len(), 17);
        }
    }

    #[test]
    fn generations_advance_exactly_and_exhaust_before_opening_a_transaction() {
        let mut store = scoped_store("/test/exhaustion");
        seed_witness(&mut store, witness(u128::MAX - 1));

        assert!(matches!(commit_empty(&mut store), CommitResult::Committed));
        assert_eq!(current_witness(&store), Some(witness(u128::MAX)));

        let error = match store.txn_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: true,
            },
        ) {
            Err(error) => error,
            Ok(_) => panic!("the exhausted generation must refuse before begin"),
        };
        assert!(matches!(
            error,
            SessionError::Engine(StoreError::LimitExceeded {
                limit: "commit witness generation"
            })
        ));
        assert_eq!(current_witness(&store), Some(witness(u128::MAX)));
    }

    #[test]
    fn malformed_or_unknown_witness_encodings_refuse_without_rewriting_them() {
        let malformed = [vec![0x01; 15], vec![0x01; 18], {
            let mut bytes = witness(7);
            bytes[0] = 0x02;
            bytes
        }];
        for bytes in malformed {
            let mut store = scoped_store("/test/malformed");
            seed_witness(&mut store, bytes.clone());
            let error = match store.txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            ) {
                Err(error) => error,
                Ok(_) => panic!("a malformed witness must refuse"),
            };
            assert!(matches!(
                error,
                SessionError::Engine(StoreError::Corruption { .. })
            ));
            assert_eq!(current_witness(&store), Some(bytes));
        }
    }

    #[test]
    fn exact_before_after_and_third_states_classify_without_approximation() {
        let path = "/test/exact-classification";
        let scope = CommitRecoveryScope::persistent([0x41; 16], path);
        let before = witness(20);
        let after = witness(21);

        for (current, expected) in [
            (before.clone(), DurableCommitState::KnownOld),
            (after.clone(), DurableCommitState::KnownNew),
            (witness(22), DurableCommitState::Unknown),
        ] {
            let mut store = scoped_store(path);
            seed_witness(&mut store, current);
            let fact = CommitRecovery {
                scope: Some(scope.clone()),
                before: Some(before.clone()),
                after: after.clone(),
            };
            assert_eq!(store.resolve_recovery(fact), expected);
            assert_eq!(store.poisoned, expected == DurableCommitState::Unknown);
        }
    }

    #[test]
    fn a_dropped_intent_generation_is_reused_only_after_its_fact_is_consumed() {
        let path = "/test/dropped-intent";
        let scope = CommitRecoveryScope::persistent([0x41; 16], path);
        let mut reopened = scoped_store(path);
        let dropped = CommitRecovery {
            scope: Some(scope),
            before: None,
            after: witness(0),
        };

        assert_eq!(
            reopened.resolve_recovery(dropped),
            DurableCommitState::KnownOld,
        );
        assert!(matches!(
            commit_empty(&mut reopened),
            CommitResult::Committed
        ));
        assert_eq!(current_witness(&reopened), Some(witness(0)));
    }

    #[test]
    fn dropping_an_uncommitted_intent_allows_the_same_next_generation() {
        let mut store = scoped_store("/test/dropped-transaction");
        let intent = store
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("derive first recovery intent");
        drop(intent);
        assert_eq!(
            current_witness(&store),
            None,
            "dropping before commit must not publish the proposed generation",
        );

        assert!(matches!(commit_empty(&mut store), CommitResult::Committed));
        assert_eq!(
            current_witness(&store),
            Some(witness(0)),
            "the next transaction may reuse the uncommitted generation",
        );
    }

    struct ReadFailureEngine {
        inner: MemoryEngine,
        fail_reads: Rc<Flag<bool>>,
    }

    impl ByteEngine for ReadFailureEngine {
        type View<'a> = <MemoryEngine as ByteEngine>::View<'a>;
        type Txn<'a> = <MemoryEngine as ByteEngine>::Txn<'a>;

        fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
            if self.fail_reads.get() {
                return Err(StoreError::Io {
                    op: "recovery_read",
                    message: "injected recovery-state read failure".into(),
                });
            }
            self.inner.read_view()
        }

        fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
            self.inner.begin()
        }

        fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
            self.inner.require_write_access(op)
        }

        fn audit_integrity(&mut self) -> Result<(), StoreError> {
            self.inner.audit_integrity()
        }
    }

    #[test]
    fn recovery_state_read_failure_is_unknown_and_poisons_the_reopened_handle() {
        let fail_reads = Rc::new(Flag::new(false));
        let mut store = store_with_engine(ReadFailureEngine {
            inner: MemoryEngine::new(),
            fail_reads: Rc::clone(&fail_reads),
        });
        let scope = CommitRecoveryScope::persistent([0x41; 16], "/test/read-failure");
        store.recovery_scope = Some(scope.clone());
        seed_witness(&mut store, witness(31));
        fail_reads.set(true);

        let fact = CommitRecovery {
            scope: Some(scope),
            before: Some(witness(30)),
            after: witness(31),
        };
        assert_eq!(store.resolve_recovery(fact), DurableCommitState::Unknown,);
        assert!(store.poisoned);
    }

    #[test]
    fn a_stale_or_wrong_scope_fact_is_unknown_never_old_or_new() {
        let scope = CommitRecoveryScope::persistent([0x41; 16], "/test/stale");
        let mut stale_store = store();
        stale_store.recovery_scope = Some(scope.clone());
        seed_witness(&mut stale_store, witness(12));
        let stale = CommitRecovery {
            scope: Some(scope),
            before: Some(witness(10)),
            after: witness(11),
        };
        assert_eq!(
            stale_store.resolve_recovery(stale),
            DurableCommitState::Unknown
        );
        assert!(matches!(
            stale_store.read_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: false,
                },
            ),
            Err(SessionError::Poisoned),
        ));

        let mut wrong_store = scoped_store("/test/wrong-store");
        seed_witness(&mut wrong_store, witness(1));
        let wrong_scope = CommitRecovery {
            scope: Some(CommitRecoveryScope::persistent(
                [0x41; 16],
                "/test/right-store",
            )),
            before: Some(witness(0)),
            after: witness(1),
        };
        assert_eq!(
            wrong_store.resolve_recovery(wrong_scope),
            DurableCommitState::Unknown
        );
        assert!(matches!(
            wrong_store.read_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: false,
                },
            ),
            Err(SessionError::Poisoned),
        ));
    }
}
