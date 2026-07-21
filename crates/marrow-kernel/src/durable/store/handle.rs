//! The durable store handle: session opening over a coherent read view or a write
//! transaction after resolving effective authority, plus witness-token minting.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::{ByteEngine, ReadView, StoreError};

use super::super::physical;
use super::super::{
    AuthorizedSite, DemandCoverage, Denied, IndexSchema, InvocationGrant, Reopen, RootNumbering,
    SessionError, SiteSpec, StoreSchema, number_store,
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
        let numbering = number_store(&schemas);
        Self {
            engine,
            schemas,
            numbering,
            sites,
            ceiling,
            poisoned: false,
        }
    }

    /// The witness classification after reopening: whether the recorded witness cell
    /// holds `token` (the commit completed) or not (it did not).
    pub fn classify(&self, token: [u8; 16]) -> Result<Reopen, StoreError> {
        match self.engine.read_view()?.get(&physical::meta_key(WITNESS))? {
            Some(w) if w == token => Ok(Reopen::CompleteNew),
            _ => Ok(Reopen::CompleteOld),
        }
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

    /// Visit every logical cell of the store in bounded scan pages, calling `per_page` for
    /// each. This is a closed lifecycle-maintenance seam (the backup/restore slice): the
    /// `ByteEngine` stays crate-private, and the visited cells are the kernel's own id-keyed
    /// logical cells — never engine pages — so a copy of them is a logical backup, not an
    /// engine-page copy. Memory is bounded by one scan page (the engine's `scan_after`
    /// contract), so a whole-store visit never materializes the store. The lifecycle owns the
    /// only caller; nothing below the kernel can reach it.
    pub fn visit_cells(
        &self,
        mut per_page: impl FnMut(&[marrow_store::Cell]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let view = self.engine.read_view()?;
        let mut cursor: Vec<u8> = Vec::new();
        loop {
            let page = view.scan_after(&[], &cursor)?;
            let Some((last_key, _)) = page.last() else {
                break;
            };
            let next_cursor = last_key.clone();
            per_page(&page)?;
            cursor = next_cursor;
        }
        Ok(())
    }

    /// Insert a batch of logical cells into the store within one transaction, committing them
    /// durably. The restore half of the backup/restore slice; the same closed
    /// lifecycle-maintenance seam as [`Self::visit_cells`]. An indeterminate or aborted commit
    /// surfaces so the caller can fail the restore rather than report a partial store.
    pub fn insert_cells(&mut self, cells: &[marrow_store::Cell]) -> Result<(), StoreError> {
        use marrow_store::{CommitOutcome, WriteTxn};
        let mut txn = self.engine.begin()?;
        for (key, value) in cells {
            txn.put(key, value.clone())?;
        }
        match txn.commit() {
            CommitOutcome::Confirmed => Ok(()),
            CommitOutcome::Aborted => Err(StoreError::Io {
                op: "restore_slice.commit",
                message: "the restore commit aborted".to_string(),
            }),
            CommitOutcome::Indeterminate => Err(StoreError::Io {
                op: "restore_slice.commit",
                message: "the restore commit was indeterminate".to_string(),
            }),
        }
    }

    /// Refuse a session open on a poisoned handle. An earlier indeterminate commit sets
    /// the latch (its durability is unknown), and the handle then refuses every further
    /// read or write until the store is reopened and the interrupted commit reclassified
    /// (complete-old vs complete-new via [`Self::classify`]). Consulted at open on both
    /// session paths so a poisoned handle never opens a view or a transaction that would
    /// observe an indeterminate state. Reachable only on a native handle whose engine can
    /// report [`CommitOutcome::Indeterminate`](marrow_store::CommitOutcome); the ephemeral
    /// memory engine always confirms, so its handle is never poisoned.
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
        // Split the store's fields into disjoint borrows: the transaction borrows the
        // engine mutably while the session still writes the poison flag.
        let Self {
            engine, poisoned, ..
        } = self;
        let txn = engine.begin().map_err(SessionError::Engine)?;
        Ok(TxnSession {
            txn: Some(txn),
            poisoned,
            auth,
            token: mint_token(),
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

/// Mint a 16-byte witness token distinct within and across processes: the wall
/// clock mixed with a process id and a monotonic counter. Not cryptographic — its
/// only contract is distinctness so a reopen can classify complete-old vs
/// complete-new.
fn mint_token() -> [u8; 16] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u128, |elapsed| elapsed.as_nanos());
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let pid = u128::from(std::process::id());
    (nanos ^ counter.rotate_left(64) ^ pid.rotate_left(32)).to_be_bytes()
}

#[cfg(test)]
mod tests {
    use marrow_store::MemoryEngine;

    use super::*;
    use crate::codec::value::ScalarKind;
    use crate::durable::{FieldSchema, InvocationGrant, SiteSpec, SiteTarget};

    fn store() -> DurableStore<MemoryEngine> {
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
            MemoryEngine::new(),
            vec![schema],
            sites,
            DemandCoverage {
                read: true,
                write: true,
            },
        )
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
}
