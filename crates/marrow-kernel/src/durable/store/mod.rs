//! The durable store handle and its read/transaction sessions (design §G).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::{ByteEngine, ReadView, StoreError, WriteTxn};

use super::physical;
use super::profile;
use super::{
    AuthorizedSite, BoundedKeys, BoundedLimit, CommitResult, CreateOutcome, DemandCoverage, Denied,
    EntryValue, EraseOutcome, IndexSchema, InvocationGrant, KernelFault, Presence, Reopen,
    ReplaceOutcome, SessionError, SiteSpec, StoreSchema,
};
use crate::codec::key::KeyScalar;
use crate::equality::ValueDomain;

mod address;
mod index_ops;
mod read_ops;
mod read_session;
mod resolve;
mod traverse;
mod txn_session;

pub use read_session::ReadSession;
pub use txn_session::TxnSession;

use resolve::resolve_site;

/// The durable operations the VM drives. Object-safe so the VM holds a
/// `&mut dyn Durable` without knowing the concrete engine or session kind. A
/// read-only export drives a [`ReadSession`]; a mutating export drives a
/// [`TxnSession`]. The verifier guarantees a read-only session never reaches a
/// mutation.
pub trait Durable {
    /// The authorized site at image site index `index`.
    fn site(&self, index: u16) -> AuthorizedSite;
    /// Every node-addressing op takes the addressed node's key-path: `[root_key]` for
    /// a root node and `[root_key, branch_key, …]` for a branch node, matching the
    /// site's root and branch-hop arity. A root site's key-path is one element.
    fn presence(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault>;
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<ValueDomain>, KernelFault>;
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault>;
    /// Materialize the record of one unkeyed group of the entry `keys` addresses: one
    /// slot per group field, present or vacant. A group's presence is its containing
    /// entry's presence, so this yields `None` exactly when the entry is payload-absent
    /// and otherwise the group's leaves (a group with all-vacant leaves reads present
    /// with every slot vacant). It reads only the group's own leaves — never the entry's
    /// top-level fields, a sibling group, or a branch.
    fn read_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault>;
    /// Exact replacement of one group of the entry `keys` addresses, scoped to the
    /// group's own field set: remove every one of the group's leaves, then write the
    /// leaf for each present field of `value`. Omitted sparse leaves do not survive
    /// (replace, not merge). A group has no independent existence, so a replace over a
    /// payload-absent entry is [`ReplaceOutcome::Missing`] and touches nothing; over a
    /// present entry the entry marker, its top-level fields, its sibling groups, and its
    /// branches are all left intact (the group-scoped payload-only law).
    fn replace_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault>;
    /// Erase one group of the entry `keys` addresses: remove every one of the group's own
    /// leaves and nothing else. [`EraseOutcome::Erased`] when any leaf existed, else
    /// [`EraseOutcome::Missing`]. The entry marker, its top-level fields, its sibling
    /// groups, and its branches are preserved.
    fn erase_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault>;
    /// Freeze the first `limit` immediate keys of the layer the whole-entry `site`
    /// belongs to — the root's entry family (a `WholePayload` site) or a keyed branch
    /// family beneath a fixed parent (a branch site) — starting at an inclusive `from`
    /// key when given, and report whether a further key existed. `ancestor_keys` is the
    /// key-path to the traversed layer's parent: empty for the root layer, `[root_key]`
    /// for a single-level branch layer — one fewer than the site's whole-entry key
    /// arity, since the traversed key is what iteration enumerates rather than an
    /// operand. At most `limit + 1` distinct present keys are acquired and the frozen
    /// set is bounded by `limit`. The walk costs `O(limit + 1 + d)` seeks, where `d` is
    /// the number of descendant-only siblings interspersed among the visited keys: a
    /// descendant-only child (branch children, no payload) is skipped by one
    /// prefix-successor seek past its subtree, and its own fan-out — however large — is
    /// never read.
    fn iterate_bounded(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault>;
    /// Freeze the first `limit` distinct values of a nonunique managed index's next
    /// projected component, holding the leading components `prefix` (a strict prefix of
    /// the index's ordered projection), starting at an inclusive `from` component when
    /// given, and report whether a further distinct value existed. Like
    /// [`Self::iterate_bounded`] this is a bounded progressive refinement: it acquires at
    /// most `limit + 1` distinct component values through the index cell family, costs
    /// `O(limit + 1)` seeks regardless of how many rows share each value (one
    /// prefix-successor seek passes a whole value's rows), and establishes no presence
    /// fact — an index scan observes only the derived index, never a source entry.
    fn index_scan(
        &mut self,
        site: &AuthorizedSite,
        prefix: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault>;
    /// Look up the single source key tuple a unique managed index maps the complete
    /// projection `key` to, or [`None`] when no row matches. One exact probe of the index
    /// cell family; it yields exactly the matching source key or absent, never a sibling,
    /// and observes no source entry.
    fn index_lookup(
        &mut self,
        site: &AuthorizedSite,
        key: &[KeyScalar],
    ) -> Result<Option<Vec<KeyScalar>>, KernelFault>;
    /// Whether the layer the whole-entry `site` names — the root's entry family (a root
    /// site) or a keyed branch family beneath the parent entry `ancestor_keys` locates (a
    /// branch site) — has at least one payload-bearing immediate child. One forward
    /// [`layer_step`] from the layer's start: a present child yields `Present`, an empty
    /// or purely descendant-only layer yields `Absent`. Descendant-only children (branch
    /// children with no payload marker) are skipped by one prefix-successor seek each, so
    /// the probe reads at most one payload child key and observes no values. Like the
    /// bounded traversal it establishes no per-key presence fact; it answers only the
    /// family-populated question.
    fn family_populated(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault>;
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: ValueDomain,
    ) -> Result<(), KernelFault>;
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault>;
    /// Set (present) or clear (vacant) a sparse field of an entry the caller has
    /// statically proven present. Asserts the entry marker is present — a violation
    /// is a marker/field mismatch ([`KernelFault::Corruption`]), never implicit
    /// creation — then stages the leaf exactly like [`Self::set_sparse`].
    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault>;
    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault>;
    fn replace_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault>;
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault>;
    fn erase_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault>;
    /// Commit the transaction (a no-op returning [`CommitResult::Committed`] for a
    /// read-only session, which the verifier guarantees never opens one).
    fn commit(&mut self) -> CommitResult;
}

/// The durable store handle. CLI-only caller at T01; dies at D00.
pub struct DurableStore<E: ByteEngine> {
    engine: E,
    /// The store's roots by declaration position: one [`StoreSchema`] per durable root,
    /// each with its own name-keyed physical cell family. A site's `root` indexes this
    /// table. One engine transaction spans every root, so a cross-root write commits or
    /// rolls back as one unit.
    schemas: Vec<StoreSchema>,
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
        Self {
            engine,
            schemas,
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

    fn verify_profile(&self) -> Result<(), SessionError> {
        match self
            .engine
            .read_view()
            .map_err(SessionError::Engine)?
            .get(&physical::meta_key(PROFILE))
            .map_err(SessionError::Engine)?
        {
            None => Ok(()),
            Some(stored) if stored == profile::store_descriptor(&self.schemas) => Ok(()),
            Some(_) => Err(SessionError::ProfileMismatch),
        }
    }

    fn authorized_sites(&self) -> Vec<AuthorizedSite> {
        self.sites
            .iter()
            .map(|site| resolve_site(&self.schemas[site.root as usize], site.root, &site.target))
            .collect()
    }

    /// Open a read session over a coherent read view after resolving effective
    /// authority and revalidating the store profile. The view is bound to the
    /// session's borrow of the store, so its reads observe one version for the
    /// whole call.
    pub fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, E>, SessionError> {
        resolve_authority(demand, self.ceiling, grant).map_err(|Denied| SessionError::Denied)?;
        self.verify_profile()?;
        let auth = self.authorized_sites();
        let view = self.engine.read_view().map_err(SessionError::Engine)?;
        Ok(ReadSession { view, auth })
    }

    /// Open a transaction session after resolving effective authority, revalidating
    /// the profile, and provisioning the profile cell on a fresh store.
    pub fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, E>, SessionError> {
        resolve_authority(demand, self.ceiling, grant).map_err(|Denied| SessionError::Denied)?;
        self.verify_profile()?;
        let auth = self.authorized_sites();
        let descriptor = profile::store_descriptor(&self.schemas);
        // Per-root managed indexes: a write to root R maintains exactly `indexes[R]`, so a
        // cross-root transaction keeps each root's own indexes coherent without confusing
        // one root's index cells with another's.
        let indexes: Vec<Vec<IndexSchema>> = self
            .schemas
            .iter()
            .map(|schema| schema.indexes.clone())
            .collect();
        // Split the store's fields into disjoint borrows: the transaction borrows the
        // engine mutably while the session still writes the poison flag. The schema is
        // read here (into `descriptor` and the resolved sites) before the split.
        let Self {
            engine, poisoned, ..
        } = self;
        let mut txn = engine.begin().map_err(SessionError::Engine)?;
        // First provision: record the profile inside this transaction if absent.
        let profile_key = physical::meta_key(PROFILE);
        if txn
            .get(&profile_key)
            .map_err(SessionError::Engine)?
            .is_none()
        {
            txn.put(&profile_key, descriptor)
                .map_err(SessionError::Engine)?;
        }
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

/// The meta-cell names in the `0x10` family.
pub(super) const PROFILE: &str = "profile";
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
mod tests;
