//! The durable store handle and its read/transaction sessions (design §G).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::{ByteEngine, CommitOutcome, ReadView, StoreError, WriteTxn};

use super::physical::{self, CellKind};
use super::plan::{CellWrite, IndexOp, Planner};
use super::profile;
use super::{
    AuthTarget, AuthorizedSite, BoundedKeys, BoundedLimit, BranchHop, BranchSchema, CommitResult,
    CreateOutcome, DemandCoverage, Denied, EntryValue, EraseOutcome, FieldSchema, IndexComponent,
    IndexSchema, InvocationGrant, KernelFault, NextKey, Presence, Reopen, ReplaceOutcome,
    SessionError, SiteSpec, SiteTarget, StoreSchema,
};
use crate::codec::key::KeyScalar;
use crate::codec::value::{ScalarKind, ValueShape, decode_domain, encode_domain};
use crate::equality::ValueDomain;

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
    schema: StoreSchema,
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
    /// Build a store over an already-open engine, minting the store ceiling from the
    /// handle's write capability. The native/tracer caller; an ephemeral attachment
    /// uses [`Self::from_engine_with_ceiling`] to bound the ceiling by image demand.
    pub fn from_engine(engine: E, schema: StoreSchema, sites: Vec<SiteSpec>) -> Self {
        let ceiling = DemandCoverage {
            read: true,
            write: engine.require_write_access("open").is_ok(),
        };
        Self::from_engine_with_ceiling(engine, schema, sites, ceiling)
    }

    /// Build a store over an already-open engine with an explicit deployment
    /// ceiling. The ephemeral-attachment caller bounds the ceiling by the image's
    /// demand union, so authority never exceeds what the compiler described even
    /// when the backing engine is unconditionally writable.
    pub fn from_engine_with_ceiling(
        engine: E,
        schema: StoreSchema,
        sites: Vec<SiteSpec>,
        ceiling: DemandCoverage,
    ) -> Self {
        Self {
            engine,
            schema,
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
            Some(stored) if stored == profile::descriptor(&self.schema) => Ok(()),
            Some(_) => Err(SessionError::ProfileMismatch),
        }
    }

    fn authorized_sites(&self) -> Vec<AuthorizedSite> {
        self.sites
            .iter()
            .map(|site| resolve_site(&self.schema, &site.target))
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
        let descriptor = profile::descriptor(&self.schema);
        let indexes = self.schema.indexes.clone();
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
const PROFILE: &str = "profile";
const WITNESS: &str = "witness";

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

/// Resolve a sealed [`SiteTarget`] against the store schema into the executable
/// [`AuthorizedSite`] the kernel ops address, once at session setup: the addressed
/// node's root, its branch path, its own record fields (for whole-entry ops), and —
/// for a field target — the field's name, kind, and required flag. A branch target
/// walks its branch path through the recursive schema so the addressed node carries the
/// key kind and record of the branch the path descends to, at any depth.
fn resolve_site(schema: &StoreSchema, target: &SiteTarget) -> AuthorizedSite {
    // A managed-index read addresses no source node: it resolves to the index's cell
    // family identity, its read kind, and the scalar kind of each projected component
    // (root key columns and top-level fields, by position), and carries an empty branch
    // path since every index is root-level.
    if let SiteTarget::IndexScan(position) | SiteTarget::IndexLookup(position) = target {
        let index = &schema.indexes[*position as usize];
        let projection = index
            .projection
            .iter()
            .map(|component| index_component_kind(schema, *component))
            .collect();
        return AuthorizedSite::index(
            schema.root_name.clone(),
            schema.key.clone(),
            AuthTarget::index(index.id, index.unique, projection),
        );
    }
    // The container node the site addresses: its branch path (one hop per branch-path
    // element) and own record fields. A root target's container is the root; a branch
    // target's (whole entry or field) is the branch node the path descends to.
    let (branch, container_fields): (Vec<BranchHop>, &[FieldSchema]) = match target {
        SiteTarget::WholePayload | SiteTarget::FieldLeaf(_) => (Vec::new(), &schema.fields),
        SiteTarget::BranchEntry(path) | SiteTarget::BranchField { branch: path, .. } => {
            resolve_branch_path(schema, path)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) => {
            unreachable!("index targets resolved above")
        }
    };
    // A whole-entry site enumerates the container's footprint, so it carries the
    // container's record; a field-target site carries its field plus the container
    // record so a staged set can reconcile the node at commit.
    let target = match target {
        SiteTarget::WholePayload | SiteTarget::BranchEntry(_) => {
            AuthTarget::Entry(container_fields.to_vec())
        }
        SiteTarget::FieldLeaf(index) | SiteTarget::BranchField { field: index, .. } => {
            AuthTarget::field(&container_fields[*index as usize], container_fields)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) => {
            unreachable!("index targets resolved above")
        }
    };
    AuthorizedSite::new(schema.root_name.clone(), schema.key.clone(), branch, target)
}

/// The scalar kind of one managed-index projection component, resolved by position
/// against the root schema: an identity key column reads its column kind; a top-level
/// field reads its scalar shape. The verifier proves every projected leaf is a stored
/// orderable-key scalar, so a non-scalar field shape here is a forged image reaching an
/// index-ineligible projection — faulted as a kernel invariant breach rather than
/// silently mis-encoded.
fn index_component_kind(schema: &StoreSchema, component: IndexComponent) -> ScalarKind {
    match component {
        IndexComponent::Key(column) => schema.key[column as usize],
        IndexComponent::Field(field) => match &schema.fields[field as usize].shape {
            ValueShape::Scalar(kind) => *kind,
            _ => unreachable!("the verifier proves an index component is a stored scalar"),
        },
    }
}

/// Walk a branch path through the recursive branch schema, one hop per element: the hops
/// down to the addressed branch node — each carrying that branch's name and key kind —
/// and the node's own record fields. The single owner of branch-path-to-node resolution,
/// so a direct branch and a nested branch resolve the same way at increasing depth: hop
/// `i` indexes level `i`'s declaration-ordered branch list, and the next level's list is
/// that branch's own sub-branches.
fn resolve_branch_path<'a>(
    schema: &'a StoreSchema,
    path: &[u16],
) -> (Vec<BranchHop>, &'a [FieldSchema]) {
    let mut hops = Vec::with_capacity(path.len());
    let mut branches: &[BranchSchema] = &schema.branches;
    let mut fields: &[FieldSchema] = &schema.fields;
    for &index in path {
        let branch = &branches[index as usize];
        hops.push(BranchHop::new(branch.name.clone(), branch.key.clone()));
        fields = &branch.fields;
        branches = &branch.branches;
    }
    (hops, fields)
}

/// The physical marker stem of the node `site` addresses at key-path `keys`: the root
/// marker followed by one branch-child stem per branch hop. The single owner of
/// key-path-to-node-stem resolution, so a root and a branch node derive their stem the
/// same way. The verifier proves the key-path arity and each element's scalar kind
/// against the site's declared root and hop kinds, but this is the trust boundary the
/// independently verified image crosses into the kernel, so a mismatch faults
/// [`KernelFault::Corruption`] in release rather than dropping a hop and mis-addressing
/// the write to a shallower node.
fn node_stem(site: &AuthorizedSite, keys: &[KeyScalar]) -> Result<Vec<u8>, KernelFault> {
    let mut cols = keys;
    let root_cols = take_columns(&mut cols, &site.key)?;
    let mut stem = physical::marker_key(&site.root, root_cols);
    for hop in &site.branch {
        let hop_cols = take_columns(&mut cols, &hop.key)?;
        stem = physical::branch_child_stem(&stem, &hop.name, hop_cols);
    }
    // Every operand column must be consumed by a node in the branch path; a leftover
    // column is a key-path/schema arity disagreement (a forged image), faulted rather
    // than silently ignored.
    if cols.is_empty() {
        Ok(stem)
    } else {
        Err(KernelFault::Corruption)
    }
}

/// Take the next `kinds.len()` columns off the front of `cols`, checking each column's
/// scalar kind matches the expected column kind, and advance `cols` past them. A short
/// key-path or a per-column kind mismatch faults [`KernelFault::Corruption`] — the trust
/// boundary the verifier's arity/kind proof stands on, defended in depth here so a forged
/// image can never mis-split a composite key-path across nodes.
fn take_columns<'a>(
    cols: &mut &'a [KeyScalar],
    kinds: &[ScalarKind],
) -> Result<&'a [KeyScalar], KernelFault> {
    if cols.len() < kinds.len() {
        return Err(KernelFault::Corruption);
    }
    let (head, tail) = cols.split_at(kinds.len());
    for (column, kind) in head.iter().zip(kinds) {
        if column.scalar_kind() != *kind {
            return Err(KernelFault::Corruption);
        }
    }
    *cols = tail;
    Ok(head)
}

/// A read session: reads observe one coherent view for the whole call. Non-`Clone`;
/// the view is released when the session drops.
pub struct ReadSession<'s, E: ByteEngine>
where
    E: 's,
{
    view: E::View<'s>,
    auth: Vec<AuthorizedSite>,
}

impl<'s, E: ByteEngine + 's> Durable for ReadSession<'s, E> {
    fn site(&self, index: u16) -> AuthorizedSite {
        self.auth[index as usize].clone()
    }
    fn presence(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault> {
        op_presence(&self.view, site, keys)
    }
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<ValueDomain>, KernelFault> {
        op_read_field(&self.view, site, keys)
    }
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        // A coherent read session observes committed state with no staging, so a
        // markerless own field leaf is a persisted orphan (corruption), not pending.
        op_read_entry(&self.view, site, keys, false)
    }
    fn iterate_bounded(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault> {
        op_iterate_bounded(&self.view, site, ancestor_keys, from, limit)
    }
    fn index_scan(
        &mut self,
        site: &AuthorizedSite,
        prefix: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault> {
        op_index_scan(&self.view, site, prefix, from, limit)
    }
    fn index_lookup(
        &mut self,
        site: &AuthorizedSite,
        key: &[KeyScalar],
    ) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
        op_index_lookup(&self.view, site, key)
    }
    fn family_populated(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault> {
        op_family_populated(&self.view, site, ancestor_keys)
    }
    fn set_required(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: ValueDomain,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn set_sparse(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn set_sparse_present(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn create_entry(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn replace_entry(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn erase_field(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn erase_entry(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn commit(&mut self) -> CommitResult {
        CommitResult::Committed
    }
}

/// A transaction session: one implicit single-writer transaction the export's call
/// graph joins. Non-`Clone`, `#[must_use]`; the consuming engine transaction it
/// holds aborts on drop if it was not committed.
#[must_use = "a transaction session must be committed or it rolls back on drop"]
pub struct TxnSession<'s, E: ByteEngine>
where
    E: 's,
{
    /// The engine write transaction. `None` after commit consumes it, so a
    /// second commit is a fault and drop is a no-op.
    txn: Option<E::Txn<'s>>,
    /// The store's poison flag, set on an indeterminate commit so a reopen
    /// reclassifies.
    poisoned: &'s mut bool,
    auth: Vec<AuthorizedSite>,
    token: [u8; 16],
    /// The root's managed indexes, in stable declaration order. Every root-level write
    /// keeps them coherent as a consequence of the source write; a store with no index
    /// carries an empty list and skips maintenance entirely.
    indexes: Vec<IndexSchema>,
    /// The durable nodes whose fields were staged this transaction, keyed by the
    /// node's marker stem so several field sets on one node stage it once. Each is
    /// reconciled at commit to decide created vs required-missing — a root node or a
    /// branch node identically, since the stem and record are resolved when the field
    /// is staged rather than re-derived from the root schema.
    pending: BTreeMap<Vec<u8>, PendingNode>,
}

/// A durable node staged for commit reconcile: its own record fields and the leaf-most
/// key of its address (for a `RequiredMissing` report). The node's marker stem is the
/// map key. This is what makes reconcile node-parametric — it validates the staged
/// node's marker and required fields at its own physical stem, one level down for a
/// branch node.
struct PendingNode {
    fields: Vec<FieldSchema>,
    key: KeyScalar,
}

/// The pre-write state a root field write captures for index maintenance: the exact indexes
/// projecting the written field, their projected field values before the write, and the
/// written field's record position. The new projected state is the old with that one
/// position replaced, so a field write reads and moves only the indexes projecting it.
struct FieldMaintenance {
    indexes: Vec<IndexSchema>,
    old: Vec<Option<ValueDomain>>,
    position: usize,
}

impl<'s, E: ByteEngine + 's> TxnSession<'s, E> {
    /// The witness token this session commits, so a caller can classify a later
    /// reopen after an indeterminate commit.
    pub fn token(&self) -> [u8; 16] {
        self.token
    }

    /// The live engine transaction. Present until commit consumes it; the verifier
    /// proves no durable op runs after commit.
    fn txn(&self) -> &E::Txn<'s> {
        self.txn
            .as_ref()
            .expect("transaction is live until commit or drop")
    }

    fn txn_mut(&mut self) -> &mut E::Txn<'s> {
        self.txn
            .as_mut()
            .expect("transaction is live until commit or drop")
    }

    fn do_commit(&mut self) -> CommitResult {
        if *self.poisoned || self.txn.is_none() {
            return CommitResult::CommitFault;
        }
        match self.reconcile() {
            Ok(()) => {}
            Err(result @ CommitResult::RequiredMissing { .. }) => {
                self.txn = None; // drop aborts the engine transaction.
                return result;
            }
            Err(_) => {
                self.txn = None;
                *self.poisoned = true;
                return CommitResult::CommitFault;
            }
        }
        // The witness rides in the same engine transaction as the staged data.
        let witness = self.token.to_vec();
        if self
            .txn_mut()
            .put(&physical::meta_key(WITNESS), witness)
            .is_err()
        {
            self.txn = None;
            *self.poisoned = true;
            return CommitResult::CommitFault;
        }
        match self.txn.take().expect("checked live above").commit() {
            CommitOutcome::Confirmed => CommitResult::Committed,
            // A clean abort left the store unchanged; an indeterminate commit
            // leaves durability unknown and poisons the store for reclassification.
            CommitOutcome::Aborted => CommitResult::CommitFault,
            CommitOutcome::Indeterminate => {
                *self.poisoned = true;
                CommitResult::CommitFault
            }
        }
    }

    /// Validate every staged node: a node with any present leaf but a missing required
    /// field is a `RequiredMissing` rollback; a markerless node whose required fields
    /// are all present gets its marker (created at commit); a fully-erased staged node
    /// is a no-op. Each staged node carries its own marker stem (the map key) and its
    /// own record, so a root node and a branch node reconcile identically — the branch
    /// node at its own stem one level down, never confused with the root's marker or
    /// fields. A node reached only by whole-entry create/replace/erase writes its
    /// marker directly and never stages, so it needs no reconcile.
    fn reconcile(&mut self) -> Result<(), CommitResult> {
        let pending = std::mem::take(&mut self.pending);
        for (stem, node) in &pending {
            let marker_present = read_raw(self.txn(), stem)
                .map_err(|_| CommitResult::CommitFault)?
                .is_some();
            let mut any_leaf = false;
            let mut missing_required: Option<String> = None;
            for field in &node.fields {
                let leaf = physical::stem_field_leaf(stem, &field.name);
                let present = read_raw(self.txn(), &leaf)
                    .map_err(|_| CommitResult::CommitFault)?
                    .is_some();
                any_leaf |= present;
                if field.required && !present && missing_required.is_none() {
                    missing_required = Some(field.name.clone());
                }
            }
            if !marker_present && !any_leaf {
                continue; // fully erased; nothing to reconcile.
            }
            if let Some(field) = missing_required {
                return Err(CommitResult::RequiredMissing {
                    key: node.key.clone(),
                    field,
                });
            }
            if !marker_present {
                self.txn_mut()
                    .put(stem, physical::MARKER_VALUE.to_vec())
                    .map_err(|_| CommitResult::CommitFault)?;
            }
        }
        Ok(())
    }

    /// Stage the node a field set touches for commit reconcile, keyed by its marker
    /// stem so several sets on one node stage it once. The node's own record (root or
    /// branch) and reporting key are read from the field-target site, so reconcile
    /// validates the addressed node rather than the root — the field-exact branch tail's
    /// soundness rests here. A whole-entry op carries no field target and stages nothing
    /// (it writes its marker directly).
    fn stage_node(&mut self, site: &AuthorizedSite, keys: &[KeyScalar]) -> Result<(), KernelFault> {
        let AuthTarget::Field { record, .. } = &site.target else {
            return Ok(());
        };
        let stem = node_stem(site, keys)?;
        let key = keys
            .last()
            .cloned()
            .expect("a durable key-path is non-empty");
        self.pending.entry(stem).or_insert_with(|| PendingNode {
            fields: record.clone(),
            key,
        });
        Ok(())
    }
}

impl<'s, E: ByteEngine + 's> Durable for TxnSession<'s, E> {
    fn site(&self, index: u16) -> AuthorizedSite {
        self.auth[index as usize].clone()
    }
    fn presence(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault> {
        op_presence(self.txn(), site, keys)
    }
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<ValueDomain>, KernelFault> {
        op_read_field(self.txn(), site, keys)
    }
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        // A transaction may hold sparse fields staged for reconcile at commit, so a
        // markerless own field leaf is tolerated as payload-absent, not corruption.
        op_read_entry(self.txn(), site, keys, true)
    }
    fn iterate_bounded(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault> {
        op_iterate_bounded(self.txn(), site, ancestor_keys, from, limit)
    }
    fn index_scan(
        &mut self,
        site: &AuthorizedSite,
        prefix: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault> {
        op_index_scan(self.txn(), site, prefix, from, limit)
    }
    fn index_lookup(
        &mut self,
        site: &AuthorizedSite,
        key: &[KeyScalar],
    ) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
        op_index_lookup(self.txn(), site, key)
    }
    fn family_populated(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault> {
        op_family_populated(self.txn(), site, ancestor_keys)
    }
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: ValueDomain,
    ) -> Result<(), KernelFault> {
        let stem = node_stem(site, keys)?;
        let leaf = physical::stem_field_leaf(&stem, field_name(site, true));
        let bytes = encode_domain(&value).map_err(|_| KernelFault::ValueRange)?;
        let maintenance = self.field_maintenance_before(site, &stem)?;
        self.txn_mut()
            .put(&leaf, bytes)
            .map_err(KernelFault::Engine)?;
        self.stage_node(site, keys)?;
        self.maintain_field_write(site, keys, maintenance, Some(value))?;
        Ok(())
    }
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        let stem = node_stem(site, keys)?;
        let leaf = physical::stem_field_leaf(&stem, field_name(site, false));
        let maintenance = self.field_maintenance_before(site, &stem)?;
        match value {
            Some(value) => {
                let bytes = encode_domain(&value).map_err(|_| KernelFault::ValueRange)?;
                self.txn_mut()
                    .put(&leaf, bytes)
                    .map_err(KernelFault::Engine)?;
                self.stage_node(site, keys)?;
                self.maintain_field_write(site, keys, maintenance, Some(value))?;
            }
            None => {
                self.txn_mut().remove(&leaf).map_err(KernelFault::Engine)?;
                self.maintain_field_write(site, keys, maintenance, None)?;
            }
        }
        Ok(())
    }
    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        // The compiler's place-slot presence proof makes an absent marker
        // unreachable; assert it here as defense in depth over the trust boundary.
        // A present field leaf without a present entry marker is corruption, never
        // implicit creation (the marker law).
        let marker = node_stem(site, keys)?;
        if read_raw(self.txn(), &marker)?.is_none() {
            return Err(KernelFault::Corruption);
        }
        self.set_sparse(site, keys, value)
    }
    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault> {
        let stem = node_stem(site, keys)?;
        let fields = node_fields(site);
        let planner = Planner::new();
        // Marker-first precedence through the one bounded prefix probe: a create over
        // a present payload is a no-op, while a create over an absent or
        // descendant-only slot writes the payload. `node_write` stages only the marker
        // and the node's own present field leaves — never a branch tag — so a
        // descendant-only node gains a payload without its branch descendants being
        // touched. A markerless own field leaf staged earlier in this transaction is
        // reconcile-pending, not a create barrier, so it is written through like an
        // absent slot.
        match probe_slot(self.txn(), &stem)? {
            SlotClass::Present => Ok(CreateOutcome::AlreadyPresent),
            SlotClass::DescendantOnly | SlotClass::Absent | SlotClass::Orphan => {
                let maintains = self.maintains_root(site);
                let old = if maintains {
                    self.read_projected(
                        &stem,
                        fields,
                        &Self::projected_positions_of(&self.indexes),
                    )?
                } else {
                    Vec::new()
                };
                let ops = planner.node_write(&stem, fields, &entry)?;
                self.apply(ops)?;
                if maintains {
                    self.maintain_indexes(site, keys, &old, &entry.fields)?;
                }
                Ok(CreateOutcome::Created)
            }
        }
    }
    fn replace_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        let stem = node_stem(site, keys)?;
        let fields = node_fields(site);
        let planner = Planner::new();
        // A markerless node (absent or descendant-only) has no payload to replace, so
        // it reports Missing without touching any descendants (the compiler lowers a
        // whole assignment as exists?→replace:create, so this is the defense-in-depth
        // arm the create path complements).
        if read_raw(self.txn(), &stem)?.is_none() {
            return Ok(ReplaceOutcome::Missing);
        }
        let maintains = self.maintains_root(site);
        let old = if maintains {
            self.read_projected(&stem, fields, &Self::projected_positions_of(&self.indexes))?
        } else {
            Vec::new()
        };
        // Exact replacement through the one node-parametric planner: remove the node's
        // own cells, then write the new payload, so unlisted sparse leaves do not
        // survive and keyed branch descendants are left intact.
        let mut ops = planner.node_erase(&stem, fields);
        ops.extend(planner.node_write(&stem, fields, &entry)?);
        self.apply(ops)?;
        if maintains {
            self.maintain_indexes(site, keys, &old, &entry.fields)?;
        }
        Ok(ReplaceOutcome::Replaced)
    }
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        let stem = node_stem(site, keys)?;
        let leaf = physical::stem_field_leaf(&stem, field_name(site, false));
        let existed = read_raw(self.txn(), &leaf)?.is_some();
        let maintenance = self.field_maintenance_before(site, &stem)?;
        self.txn_mut().remove(&leaf).map_err(KernelFault::Engine)?;
        self.maintain_field_write(site, keys, maintenance, None)?;
        Ok(if existed {
            EraseOutcome::Erased
        } else {
            EraseOutcome::Missing
        })
    }
    fn erase_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        let stem = node_stem(site, keys)?;
        let fields = node_fields(site);
        let planner = Planner::new();
        let existed = read_raw(self.txn(), &stem)?.is_some();
        let maintains = self.maintains_root(site);
        let old = if maintains {
            self.read_projected(&stem, fields, &Self::projected_positions_of(&self.indexes))?
        } else {
            Vec::new()
        };
        // Whole-node removal through the node-parametric planner: marker plus every own
        // field leaf, by exact key — a branch tag is never enumerated, so a node's
        // keyed descendants survive an erase of its payload.
        let ops = planner.node_erase(&stem, fields);
        self.apply(ops)?;
        if maintains {
            let new = vec![None; fields.len()];
            self.maintain_indexes(site, keys, &old, &new)?;
        }
        Ok(if existed {
            EraseOutcome::Erased
        } else {
            EraseOutcome::Missing
        })
    }
    fn commit(&mut self) -> CommitResult {
        self.do_commit()
    }
}

impl<'s, E: ByteEngine + 's> TxnSession<'s, E> {
    /// Apply an ordered cell plan the consequence planner produced. Every write and
    /// removal rides this session's engine transaction, so the whole plan commits or
    /// rolls back as one unit with the rest of the transaction.
    fn apply(&mut self, ops: Vec<CellWrite>) -> Result<(), KernelFault> {
        for op in ops {
            match op {
                CellWrite::Put(key, value) => {
                    self.txn_mut()
                        .put(&key, value)
                        .map_err(KernelFault::Engine)?;
                }
                CellWrite::Remove(key) => {
                    self.txn_mut().remove(&key).map_err(KernelFault::Engine)?;
                }
            }
        }
        Ok(())
    }

    /// Whether root-level managed-index maintenance applies to a write on `site`: the
    /// store declares indexes and the write addresses a root entry. A branch entry carries
    /// no index (indexes project a root's own keys and top-level fields), so a branch write
    /// never maintains one.
    fn maintains_root(&self, site: &AuthorizedSite) -> bool {
        !self.indexes.is_empty() && site.branch.is_empty()
    }

    /// The distinct root field positions `indexes` project, so maintenance reads exactly the
    /// projected leaves those indexes need — never the whole record, and for a field write
    /// never a leaf of an index the write does not touch.
    fn projected_positions_of(indexes: &[IndexSchema]) -> Vec<usize> {
        let mut positions: Vec<usize> = indexes
            .iter()
            .flat_map(|index| {
                index
                    .projection
                    .iter()
                    .filter_map(|component| match component {
                        IndexComponent::Field(field) => Some(*field as usize),
                        IndexComponent::Key(_) => None,
                    })
            })
            .collect();
        positions.sort_unstable();
        positions.dedup();
        positions
    }

    /// The managed indexes that project the root field at `position` — the exact indexes a
    /// write to that field must maintain, and the only ones it reads sibling leaves for.
    fn indexes_projecting(&self, position: usize) -> Vec<IndexSchema> {
        self.indexes
            .iter()
            .filter(|index| {
                index.projection.iter().any(|component| {
                    matches!(component, IndexComponent::Field(field) if *field as usize == position)
                })
            })
            .cloned()
            .collect()
    }

    /// The current stored values at `positions` of the root entry with marker `stem`, aligned
    /// to `record` (a position not read stays `None`). Reads observe this transaction's
    /// staged writes, so an in-flight change is captured; a projected leaf that will not
    /// decode is corruption.
    fn read_projected(
        &self,
        stem: &[u8],
        record: &[FieldSchema],
        positions: &[usize],
    ) -> Result<Vec<Option<ValueDomain>>, KernelFault> {
        let mut fields = vec![None; record.len()];
        for &position in positions {
            let field = &record[position];
            let leaf = physical::stem_field_leaf(stem, &field.name);
            if let Some(bytes) = read_raw(self.txn(), &leaf)? {
                fields[position] =
                    Some(decode_domain(&bytes, &field.shape).ok_or(KernelFault::Corruption)?);
            }
        }
        Ok(fields)
    }

    /// Capture the pre-write state a root field write needs for index maintenance, before
    /// the write overwrites the field leaf: the exact indexes projecting the written field,
    /// those indexes' projected field values, and the written position. `None` when the write
    /// maintains no index (an unindexed store, a branch field, or a field no index projects),
    /// so the field ops read and stage nothing on the common path.
    fn field_maintenance_before(
        &self,
        site: &AuthorizedSite,
        stem: &[u8],
    ) -> Result<Option<FieldMaintenance>, KernelFault> {
        if !self.maintains_root(site) {
            return Ok(None);
        }
        let record = site_record(site);
        let position = field_index_in_record(site, record);
        let indexes = self.indexes_projecting(position);
        if indexes.is_empty() {
            return Ok(None);
        }
        let old = self.read_projected(stem, record, &Self::projected_positions_of(&indexes))?;
        Ok(Some(FieldMaintenance {
            indexes,
            old,
            position,
        }))
    }

    /// Maintain the field write's indexes from its captured state and the field's new value
    /// (`None` for a clear/erase). The new projected state is the old with the written
    /// position replaced, so only the indexes projecting the field move.
    fn maintain_field_write(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        maintenance: Option<FieldMaintenance>,
        new_value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        let Some(FieldMaintenance {
            indexes,
            old,
            position,
        }) = maintenance
        else {
            return Ok(());
        };
        let mut new = old.clone();
        new[position] = new_value;
        let ops = Planner::new().index_writes(&site.root, &indexes, keys, &old, &new)?;
        self.apply_index_ops(ops)
    }

    /// Maintain every managed index for a whole root entry write, given the entry's projected
    /// field values before (`old`) and after (`new`). An index row exists exactly when every
    /// projected component is present, so a field absent in a state contributes no row.
    fn maintain_indexes(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        old: &[Option<ValueDomain>],
        new: &[Option<ValueDomain>],
    ) -> Result<(), KernelFault> {
        let ops = Planner::new().index_writes(&site.root, &self.indexes, keys, old, new)?;
        self.apply_index_ops(ops)
    }

    /// Apply the planner's index-cell operations on this session's transaction, in stable
    /// order. A remove clears a row that left an index; a put writes a non-unique row; a
    /// unique put faults [`KernelFault::UniqueIndexViolation`] when the cell already holds a
    /// *different* source identity — a coherent re-put of the same identity is written
    /// through. A collision rolls the whole transaction back without poisoning the store, so
    /// index and source changes commit or roll back as one unit.
    fn apply_index_ops(&mut self, ops: Vec<IndexOp>) -> Result<(), KernelFault> {
        for op in ops {
            match op {
                IndexOp::Remove(cell) => {
                    self.txn_mut().remove(&cell).map_err(KernelFault::Engine)?;
                }
                IndexOp::Put(cell, value) => {
                    self.txn_mut()
                        .put(&cell, value)
                        .map_err(KernelFault::Engine)?;
                }
                IndexOp::UniquePut(cell, value) => {
                    if read_raw(self.txn(), &cell)?.is_some_and(|existing| existing != value) {
                        return Err(KernelFault::UniqueIndexViolation);
                    }
                    self.txn_mut()
                        .put(&cell, value)
                        .map_err(KernelFault::Engine)?;
                }
            }
        }
        Ok(())
    }
}

/// The record whose fields a site addresses: the entry's own record for a whole-entry
/// site, the containing node's record for a field site. Index maintenance reads projected
/// leaves from it.
fn site_record(site: &AuthorizedSite) -> &[FieldSchema] {
    match &site.target {
        AuthTarget::Entry(fields) => fields,
        AuthTarget::Field { record, .. } => record,
        AuthTarget::Index { .. } => unreachable!("verifier proved a node op targets a node site"),
    }
}

/// The position of a field site's field within its containing record.
fn field_index_in_record(site: &AuthorizedSite, record: &[FieldSchema]) -> usize {
    let AuthTarget::Field { name, .. } = &site.target else {
        unreachable!("a field op targets a field site")
    };
    record
        .iter()
        .position(|field| &field.name == name)
        .expect("a field site names a record field")
}

/// The field name of a field-target site, checking the required flag matches the
/// operation. The verifier already restricts required vs sparse ops to the right
/// site target; this reads the token's own flag as defense-in-depth over the trust
/// boundary rather than trusting a caller assertion.
fn field_name(site: &AuthorizedSite, want_required: bool) -> &str {
    match &site.target {
        AuthTarget::Field { name, required, .. } => {
            debug_assert_eq!(
                *required, want_required,
                "site required-ness must match the operation the verifier admitted"
            );
            name
        }
        AuthTarget::Entry(_) | AuthTarget::Index { .. } => {
            unreachable!("verifier proved a field-target site")
        }
    }
}

/// The addressed node's own record fields for a whole-entry op. The verifier proves a
/// whole-entry opcode targets an entry site, so a field target here is unreachable.
fn node_fields(site: &AuthorizedSite) -> &[FieldSchema] {
    match &site.target {
        AuthTarget::Entry(fields) => fields,
        AuthTarget::Field { .. } | AuthTarget::Index { .. } => {
            unreachable!("verifier proved a whole-entry op targets an entry site")
        }
    }
}

fn read_raw<V: ReadView>(cells: &V, key: &[u8]) -> Result<Option<Vec<u8>>, KernelFault> {
    cells.get(key).map_err(KernelFault::Engine)
}

/// The four-state classification of a whole-entry slot the bounded prefix probe
/// yields.
enum SlotClass {
    /// The payload marker is present: the entry has a payload.
    Present,
    /// No marker, but a branch descendant exists — a descendant-only node (children,
    /// no payload). It reads as payload-absent; a create gives it a payload without
    /// disturbing the descendants.
    DescendantOnly,
    /// No marker, but an own field leaf exists — a marker/field mismatch. A persisted
    /// orphan is corruption; a sparse field staged earlier in the same transaction is
    /// reconcile-pending, so a mutating session tolerates it (see [`op_read_entry`]).
    Orphan,
    /// No marker and nothing beneath: the slot is absent.
    Absent,
}

/// One bounded prefix probe over an entry's marker `stem`: a point read of the
/// marker plus, when the marker is absent, one bounded scan for the first cell
/// beneath it. This is the single owner of whole-entry slot classification —
/// separating an absent slot from a descendant-only node and from a marker/field
/// mismatch — so create/read/replace/erase share one marker-first precedence rather
/// than each re-deriving presence. The scan reads the node's own cells in key order,
/// and own field leaves sort ahead of branch descendants, so the first cell decides
/// (an orphan own-leaf takes precedence over a descendant, surfacing corruption).
fn probe_slot<V: ReadView>(cells: &V, stem: &[u8]) -> Result<SlotClass, KernelFault> {
    if read_raw(cells, stem)?.is_some() {
        return Ok(SlotClass::Present);
    }
    let page = cells.scan_after(stem, stem).map_err(KernelFault::Engine)?;
    Ok(match page.first() {
        None => SlotClass::Absent,
        Some((cell_key, _)) => match physical::below_marker(stem, cell_key) {
            physical::BelowMarker::OwnField => SlotClass::Orphan,
            physical::BelowMarker::BranchDescendant => SlotClass::DescendantOnly,
            // An unrecognized structural tag is a shape the layout never writes: fail
            // closed with corruption in every session, never tolerated as staging.
            physical::BelowMarker::Corrupt => return Err(KernelFault::Corruption),
            physical::BelowMarker::Foreign => SlotClass::Absent,
        },
    })
}

fn op_presence<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
) -> Result<Presence, KernelFault> {
    let stem = node_stem(site, keys)?;
    let physical_key = match &site.target {
        AuthTarget::Entry(_) => stem,
        AuthTarget::Field { name, .. } => physical::stem_field_leaf(&stem, name),
        AuthTarget::Index { .. } => {
            unreachable!("verifier proved a presence op targets a node site")
        }
    };
    Ok(match read_raw(cells, &physical_key)? {
        Some(_) => Presence::Present,
        None => Presence::Absent,
    })
}

fn op_read_field<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
) -> Result<Option<ValueDomain>, KernelFault> {
    let AuthTarget::Field { name, shape, .. } = &site.target else {
        unreachable!("verifier proved a field read targets a field site")
    };
    let leaf = physical::stem_field_leaf(&node_stem(site, keys)?, name);
    match read_raw(cells, &leaf)? {
        None => Ok(None),
        Some(bytes) => decode_domain(&bytes, shape)
            .map(Some)
            .ok_or(KernelFault::Corruption),
    }
}

fn op_read_entry<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
    tolerate_pending: bool,
) -> Result<Option<EntryValue>, KernelFault> {
    let stem = node_stem(site, keys)?;
    let fields = node_fields(site);
    // Marker-first precedence through the one bounded prefix probe. A node with no
    // payload marker reads as payload-absent whether it is empty or a descendant-only
    // node (branch children, no payload). A markerless slot carrying an own field
    // leaf is a marker/field mismatch: in a committed read session it is a persisted
    // orphan (corruption); inside a transaction it may be a sparse field staged for
    // reconcile at commit, so a mutating session tolerates it as payload-absent.
    match probe_slot(cells, &stem)? {
        SlotClass::DescendantOnly | SlotClass::Absent => return Ok(None),
        SlotClass::Orphan => {
            return if tolerate_pending {
                Ok(None)
            } else {
                Err(KernelFault::Corruption)
            };
        }
        SlotClass::Present => {}
    }
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let leaf = physical::stem_field_leaf(&stem, &field.name);
        match read_raw(cells, &leaf)? {
            None => {
                // A present marker with a missing required field is a marker/field
                // mismatch: corruption, never implicit absence.
                if field.required {
                    return Err(KernelFault::Corruption);
                }
                values.push(None);
            }
            Some(bytes) => {
                values.push(Some(
                    decode_domain(&bytes, &field.shape).ok_or(KernelFault::Corruption)?,
                ));
            }
        }
    }
    Ok(Some(EntryValue { fields: values }))
}

/// Where a forward layer walk begins.
enum LayerSeek {
    /// At the layer's first child.
    Start,
    /// Strictly after `key`'s whole subtree (an exclusive resume).
    After(KeyScalar),
    /// At the first child whose key is `>= from` (an inclusive lower bound).
    From(KeyScalar),
}

/// One forward step over a durable `layer`: the first present (payload-bearing) child
/// at or after `seek`, or [`NextKey::End`]. A descendant-only child — branch children
/// but no payload marker — is skipped with one prefix-successor seek past its subtree,
/// which passes its whole subtree regardless of branch fan-out. The single owner of the
/// forward marker walk: the bounded layer acquisition steps through it for both the
/// root and branch layers, so they walk identically.
fn layer_step<V: ReadView>(
    cells: &V,
    layer: &physical::Layer,
    seek: LayerSeek,
) -> Result<NextKey, KernelFault> {
    let mut cursor = match seek {
        LayerSeek::Start => layer.prefix().to_vec(),
        LayerSeek::After(key) => layer.child_cursor(&key),
        LayerSeek::From(from) => layer.seek_from(&from),
    };
    loop {
        let page = cells
            .scan_after(layer.prefix(), &cursor)
            .map_err(KernelFault::Engine)?;
        let Some((cell_key, _)) = page.into_iter().next() else {
            return Ok(NextKey::End);
        };
        match layer.classify(&cell_key) {
            CellKind::Marker(key) => return Ok(NextKey::Next(key)),
            CellKind::Descendant(key) => cursor = layer.child_cursor(&key),
            CellKind::Orphan => return Err(KernelFault::Corruption),
            CellKind::Foreign => return Ok(NextKey::End),
        }
    }
}

/// The durable layer the whole-entry `site` traverses, resolving its parent entry from
/// `ancestor_keys`: a root (`WholePayload`) site traverses the root's entry family with
/// no ancestor key; a branch site traverses its branch family beneath the parent entry
/// the ancestor key-path names, one ancestor key per parent hop above the traversed
/// branch. The single owner of the site-to-traversed-layer mapping. The verifier proves
/// the ancestor arity and each key's scalar kind against the site's declared root and hop
/// kinds, but this is the trust boundary the independently verified image crosses into
/// the kernel, so a mismatch faults [`KernelFault::Corruption`] — matching [`node_stem`]'s
/// hard backstop — rather than mis-layering the traversal to a shallower or wrong parent
/// node.
fn layer_of(
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
) -> Result<physical::Layer, KernelFault> {
    match site.branch.split_last() {
        None => {
            if !ancestor_keys.is_empty() {
                return Err(KernelFault::Corruption);
            }
            // A traversable layer is single-column; a composite-keyed root layer is not
            // traversed (the verifier parks it), so a multi-column root here is a forged
            // image reaching an untraversable shape.
            if site.key.len() != 1 {
                return Err(KernelFault::Corruption);
            }
            Ok(physical::Layer::root(&site.root))
        }
        Some((traversed, parent_hops)) => {
            // The traversed branch layer must be single-column (composite-keyed layers are
            // parked before traversal); its ancestor key-path locates its parent entry —
            // the root's key columns then each parent hop's key columns.
            if traversed.key.len() != 1 {
                return Err(KernelFault::Corruption);
            }
            let mut cols = ancestor_keys;
            let root_cols = take_columns(&mut cols, &site.key)?;
            let mut stem = physical::marker_key(&site.root, root_cols);
            for hop in parent_hops {
                let hop_cols = take_columns(&mut cols, &hop.key)?;
                stem = physical::branch_child_stem(&stem, &hop.name, hop_cols);
            }
            if !cols.is_empty() {
                return Err(KernelFault::Corruption);
            }
            Ok(physical::Layer::branch(&stem, &traversed.name))
        }
    }
}

/// Freeze the first `limit` immediate keys of the layer `site` traverses and report
/// whether a further key existed. Acquires at most `limit + 1` distinct present keys —
/// the frozen set plus one existence probe — through the bounded [`layer_step`] walk,
/// costing `O(limit + 1 + d)` seeks, where `d` is the count of descendant-only siblings
/// interspersed among them (each skipped by one prefix-successor seek without its
/// fan-out being read). The frozen keys are captured before any caller runs a loop
/// body, so writes a body performs cannot change the set.
fn op_iterate_bounded<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
    from: Option<KeyScalar>,
    limit: BoundedLimit,
) -> Result<BoundedKeys, KernelFault> {
    let layer = layer_of(site, ancestor_keys)?;
    // Reserve a bounded spine rather than the full `limit`: a sparse layer freezes far
    // fewer keys than a large `at most N` permits, so the eager reservation is capped
    // and the Vec grows on demand within `limit`. Peak freeze memory is the frozen key
    // count times the maximum key size; the exact aggregate ceiling is enforced by the
    // VM's one collection owner (`MAX_AGGREGATE_BYTES`) once the keys materialize as a
    // `List[K]`.
    let mut keys: Vec<KeyScalar> = Vec::with_capacity(limit.get().min(1024));
    // The first step honors an inclusive `from`; each later step resumes strictly after
    // the last frozen key.
    let mut seek = match from {
        Some(from) => LayerSeek::From(from),
        None => LayerSeek::Start,
    };
    loop {
        match layer_step(cells, &layer, seek)? {
            NextKey::End => return Ok(BoundedKeys { keys, more: false }),
            NextKey::Next(key) => {
                if keys.len() == limit.get() {
                    // A present key exists beyond the frozen `limit`: the `on more` bit.
                    // Its existence is recorded but the key itself is not frozen or run.
                    return Ok(BoundedKeys { keys, more: true });
                }
                seek = LayerSeek::After(key.clone());
                keys.push(key);
            }
        }
    }
}

/// Whether the layer the whole-entry `site` names has at least one payload-bearing
/// immediate child: one forward [`layer_step`] from the layer's start. A present child
/// yields `Present`; an empty or purely descendant-only layer yields `Absent`. Reads at
/// most one payload child key (descendant-only children are skipped by one seek each) and
/// establishes no per-key presence fact — the bounded family-populated probe.
fn op_family_populated<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    ancestor_keys: &[KeyScalar],
) -> Result<Presence, KernelFault> {
    let layer = layer_of(site, ancestor_keys)?;
    Ok(match layer_step(cells, &layer, LayerSeek::Start)? {
        NextKey::Next(_) => Presence::Present,
        NextKey::End => Presence::Absent,
    })
}

/// The resolved index-read target a site addresses: its cell-family identity, unique
/// flag, and ordered projection component kinds. A non-index site reaching an index op
/// is a forged image routing a read to the wrong site kind — faulted rather than trusted.
fn index_target(site: &AuthorizedSite) -> Result<(&[u8; 16], bool, &[ScalarKind]), KernelFault> {
    match site.index_read() {
        Some(parts) => Ok(parts),
        None => Err(KernelFault::Corruption),
    }
}

/// Freeze the first `limit` distinct values of a nonunique index's next projected
/// component, holding the leading `prefix`, and report whether a further distinct value
/// existed. Acquires at most `limit + 1` distinct component values through the index cell
/// family, costing `O(limit + 1)` seeks: one prefix-successor seek past each yielded
/// value passes its whole run of rows regardless of fan-out (the index traversal-skip
/// law). An index scan reads only the derived index and establishes no source presence.
fn op_index_scan<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    prefix: &[KeyScalar],
    from: Option<KeyScalar>,
    limit: BoundedLimit,
) -> Result<BoundedKeys, KernelFault> {
    let (id, unique, projection) = index_target(site)?;
    // A scan is the nonunique read; a unique index admits only the complete-key lookup.
    // The verifier proves the read kind matches the index, so a mismatch here is a forged
    // image.
    if unique {
        return Err(KernelFault::Corruption);
    }
    // The held prefix must be a strict prefix of the projection — at least one component
    // remains to enumerate — and each held column must match its projected kind.
    let held = prefix.len();
    if held >= projection.len() {
        return Err(KernelFault::Corruption);
    }
    check_kinds(prefix, &projection[..held])?;
    let next_kind = projection[held];
    if let Some(from) = &from
        && from.scalar_kind() != next_kind
    {
        return Err(KernelFault::Corruption);
    }

    let layer = physical::IndexLayer::new(&site.root, id, prefix);
    let mut keys: Vec<KeyScalar> = Vec::with_capacity(limit.get().min(1024));
    // An inclusive `from` seeks to `prefix ++ enc(from)`; a bare forward scan then
    // excludes an equal cursor, which misses the `from` row only when `from` completes
    // the projection (its cell equals that key exactly). One probe of that exact key
    // resolves the boundary without a second seek per value.
    let mut cursor = match &from {
        Some(from) => {
            let seek = layer.seek_from(from);
            if cells.get(&seek).map_err(KernelFault::Engine)?.is_some() {
                keys.push(from.clone());
            }
            seek
        }
        None => layer.prefix().to_vec(),
    };
    loop {
        let page = cells
            .scan_after(layer.prefix(), &cursor)
            .map_err(KernelFault::Engine)?;
        let Some((cell_key, _)) = page.into_iter().next() else {
            return Ok(BoundedKeys { keys, more: false });
        };
        let Some(component) = layer.next_component(&cell_key) else {
            return Err(KernelFault::Corruption);
        };
        // The decoded component's kind must match the projection: an index cell whose
        // next column decodes as a different scalar kind is a corrupt or forged cell.
        if component.scalar_kind() != next_kind {
            return Err(KernelFault::Corruption);
        }
        if keys.len() == limit.get() {
            // A further distinct value exists beyond the frozen `limit`: the `on more`
            // bit. Its existence is recorded but the value itself is not frozen.
            return Ok(BoundedKeys { keys, more: true });
        }
        cursor = layer.skip_cursor(&component);
        keys.push(component);
    }
}

/// Look up the single source key tuple a unique index maps the complete projection `key`
/// to, or [`None`]. One exact probe of the index cell family; the stored value decodes as
/// the root's key tuple (`site.key.len()` columns). An index cell whose value does not
/// decode as exactly that many key columns is corruption.
fn op_index_lookup<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    key: &[KeyScalar],
) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
    let (id, unique, projection) = index_target(site)?;
    // A complete-key lookup is the unique read; a nonunique index admits only the
    // progressive scan. The verifier proves the match, so a mismatch is a forged image.
    if !unique {
        return Err(KernelFault::Corruption);
    }
    if key.len() != projection.len() {
        return Err(KernelFault::Corruption);
    }
    check_kinds(key, projection)?;
    let cell_key = physical::index_cell_key(&site.root, id, key);
    match cells.get(&cell_key).map_err(KernelFault::Engine)? {
        None => Ok(None),
        Some(bytes) => match physical::decode_index_source_key(&bytes, site.key.len()) {
            Some(source) => Ok(Some(source)),
            None => Err(KernelFault::Corruption),
        },
    }
}

/// Check that each value's scalar kind matches the expected column kind. A per-column
/// mismatch is the trust boundary the verifier's operand-kind proof stands on, defended
/// in depth so a forged image can never read an index at the wrong component type.
fn check_kinds(values: &[KeyScalar], kinds: &[ScalarKind]) -> Result<(), KernelFault> {
    for (value, kind) in values.iter().zip(kinds) {
        if value.scalar_kind() != *kind {
            return Err(KernelFault::Corruption);
        }
    }
    Ok(())
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
    use marrow_store::{ByteEngine, CommitOutcome, MemoryEngine, NativeEngine, ReadView, WriteTxn};

    use super::super::physical;
    use super::super::{
        BoundedKeys, BoundedLimit, BranchSchema, CommitResult, CreateOutcome, DemandCoverage,
        EntryValue, EraseOutcome, FieldSchema, IndexComponent, IndexSchema, InvocationGrant,
        KernelFault, Presence, ReplaceOutcome, SessionError, SiteSpec, SiteTarget, StoreSchema,
    };
    use super::{Durable, DurableStore};
    use crate::codec::key::KeyScalar;
    use crate::codec::value::{RuntimeScalar, ScalarKind};
    use crate::equality::ValueDomain;

    fn schema() -> StoreSchema {
        StoreSchema {
            root_name: "counters".into(),
            key: vec![ScalarKind::Str],
            fields: vec![
                FieldSchema::scalar("value", ScalarKind::Int, true),
                FieldSchema::scalar("label", ScalarKind::Str, false),
            ],
            branches: Vec::new(),
            indexes: Vec::new(),
        }
    }

    fn sites() -> Vec<SiteSpec> {
        vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: SiteTarget::FieldLeaf(0),
            },
            SiteSpec {
                target: SiteTarget::FieldLeaf(1),
            },
        ]
    }

    /// A branch-entry target naming the branch node the `path` of per-level branch
    /// indices descends to (`&[0]` a direct child branch, `&[0, 1]` a nested one).
    fn branch_entry(path: &[u16]) -> SiteTarget {
        SiteTarget::BranchEntry(path.into())
    }

    /// A branch-field target: the branch node `path` descends to, field index `field`.
    fn branch_field(path: &[u16], field: u16) -> SiteTarget {
        SiteTarget::BranchField {
            branch: path.into(),
            field,
        }
    }

    fn value_entry(v: i64) -> EntryValue {
        EntryValue {
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(v))), None],
        }
    }

    fn write_demand() -> DemandCoverage {
        DemandCoverage {
            read: true,
            write: true,
        }
    }

    fn read_demand() -> DemandCoverage {
        DemandCoverage {
            read: true,
            write: false,
        }
    }

    #[test]
    fn the_authority_triple_admits_the_union_and_checks_the_named_record() {
        // The compiler-side demand reaches the triple as read/write coverage: a
        // whole-program union for admission, a named export's record for invocation.
        // Under a read-only grant, a read-only record is admitted while a writing
        // record — including the union of a program that writes — is denied. Demand
        // never grants; the grant is the intersecting term.
        let read_grant = InvocationGrant {
            read: true,
            write: false,
        };

        // Invocation of a read-only export: admitted under the read-only grant.
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        assert!(store.read_session(read_grant, read_demand()).is_ok());

        // Admission of a program whose union writes: denied under the read-only grant.
        assert!(matches!(
            store.txn_session(read_grant, write_demand()),
            Err(SessionError::Denied)
        ));

        // A full grant admits the writing union.
        assert!(
            store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .is_ok()
        );
    }

    #[test]
    fn iterates_created_keys_in_forward_order() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let entry = txn.site(0);
            // Insert out of order; iteration must still be ascending.
            for name in ["b", "a", "c"] {
                txn.create_entry(&entry, &[KeyScalar::Str(name.into())], value_entry(1))
                    .expect("create");
            }
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let entry = read.site(0);
        let frozen = read
            .iterate_bounded(&entry, &[], None, bound(16))
            .expect("iterate");
        assert!(!frozen.more);
        assert_eq!(
            frozen.keys,
            vec![
                KeyScalar::Str("a".into()),
                KeyScalar::Str("b".into()),
                KeyScalar::Str("c".into()),
            ]
        );
    }

    #[test]
    fn a_field_leaf_without_a_marker_is_corruption() {
        // Write a field leaf directly, with no entry marker: an orphan leaf.
        let mut engine = MemoryEngine::new();
        {
            let mut txn = engine.begin().expect("begin");
            txn.put(
                &physical::stem_field_leaf(
                    &physical::marker_key("counters", &[KeyScalar::Str("x".into())]),
                    "value",
                ),
                b"5".to_vec(),
            )
            .expect("seed orphan leaf");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let mut store = DurableStore::from_engine(engine, schema(), sites());
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let entry = read.site(0);
        assert_eq!(
            read.iterate_bounded(&entry, &[], None, bound(4)),
            Err(KernelFault::Corruption)
        );
    }

    #[test]
    fn a_branch_field_write_with_a_root_only_key_path_faults() {
        // A branch-field site addresses the two-element key-path [root_key, branch_key].
        // A forged image that drives the strict present set over it with a single-element
        // key path must fault at the trust boundary rather than drop the branch hop and
        // mis-address the write to the root node. This is the release backstop over
        // `node_stem`'s key-path arity that the verifier's proof stands on.
        let schema = StoreSchema {
            root_name: "counters".into(),
            key: vec![ScalarKind::Str],
            fields: vec![FieldSchema::scalar("value", ScalarKind::Int, true)],
            branches: vec![BranchSchema {
                name: "notes".into(),
                key: vec![ScalarKind::Str],
                fields: vec![FieldSchema::scalar("body", ScalarKind::Str, false)],
                branches: Vec::new(),
            }],
            indexes: Vec::new(),
        };
        let sites = vec![SiteSpec {
            target: branch_field(&[0], 0),
        }];
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let branch_field = txn.site(0);
        // One key where the branch-field node needs two ([root_key, branch_key]).
        assert_eq!(
            txn.set_sparse_present(
                &branch_field,
                &[KeyScalar::Str("root".into())],
                Some(ValueDomain::Scalar(RuntimeScalar::Str("note".into()))),
            ),
            Err(KernelFault::Corruption)
        );
    }

    #[test]
    fn a_required_field_missing_at_commit_rolls_back() {
        // Stage only the sparse label on a fresh entry: the required value is unset,
        // so commit reports RequiredMissing and rolls back.
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let label = txn.site(2);
        txn.set_sparse(
            &label,
            &[KeyScalar::Str("x".into())],
            Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into()))),
        )
        .expect("set sparse");
        assert!(matches!(txn.commit(), CommitResult::RequiredMissing { .. }));
    }

    #[test]
    fn a_committed_orphan_reads_as_corruption() {
        // A committed store with a field leaf but no entry marker is corrupt. A
        // whole-entry read through a coherent read session reports corruption via the
        // bounded prefix probe rather than silently reading the slot as absent.
        let mut engine = MemoryEngine::new();
        {
            let mut txn = engine.begin().expect("begin");
            txn.put(
                &physical::stem_field_leaf(
                    &physical::marker_key("counters", &[KeyScalar::Str("x".into())]),
                    "value",
                ),
                b"5".to_vec(),
            )
            .expect("seed orphan leaf");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let mut store = DurableStore::from_engine(engine, schema(), sites());
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let entry = read.site(0);
        assert_eq!(
            read.read_entry(&entry, &[KeyScalar::Str("x".into())]),
            Err(KernelFault::Corruption),
        );
    }

    #[test]
    fn a_transaction_tolerates_a_staged_sparse_field_as_payload_absent() {
        // Inside a transaction a sparse field staged before its entry's marker is
        // reconcile-pending, not corruption: a whole-entry read observes it as
        // payload-absent, matching the pre-probe behavior the reconcile model needs.
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let label = txn.site(2);
        let entry = txn.site(0);
        txn.set_sparse(
            &label,
            &[KeyScalar::Str("x".into())],
            Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into()))),
        )
        .expect("set sparse");
        assert_eq!(
            txn.read_entry(&entry, &[KeyScalar::Str("x".into())]),
            Ok(None),
            "a staged sparse field reads as payload-absent, not corruption",
        );
    }

    /// A schema with one keyed branch: root `books` keyed by string with a required
    /// `title`, plus a keyed branch `notes` keyed by int with a required `text`. The
    /// site table addresses the root entry (0) and the branch entry (1).
    fn branch_schema() -> (StoreSchema, Vec<SiteSpec>) {
        let schema = StoreSchema {
            root_name: "books".into(),
            key: vec![ScalarKind::Str],
            fields: vec![FieldSchema::scalar("title", ScalarKind::Str, true)],
            branches: vec![BranchSchema {
                name: "notes".into(),
                key: vec![ScalarKind::Int],
                fields: vec![FieldSchema::scalar("text", ScalarKind::Str, true)],
                branches: Vec::new(),
            }],
            indexes: Vec::new(),
        };
        let sites = vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: branch_entry(&[0]),
            },
        ];
        (schema, sites)
    }

    /// The whole-entry branch vertical end to end: creating a branch entry under an
    /// absent root leaves the root descendant-only (no payload marker, children below
    /// it), so a whole read of the root is payload-absent; a create over that
    /// descendant-only slot gives the root a payload without disturbing the branch
    /// descendant, and a replace over the branch keeps the branch's own record while a
    /// replace over the descendant-only root reports Missing.
    #[test]
    fn a_branch_entry_makes_its_root_descendant_only_and_root_create_preserves_it() {
        let (schema, sites) = branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let book = KeyScalar::Str("a".into());
        let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];

        // Create a branch entry under the absent root `a`: this writes the branch
        // child's marker and its `text` leaf, and never the root `a` marker.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let branch = txn.site(1);
            let entry = EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
            };
            assert_eq!(
                txn.create_entry(&branch, &note, entry)
                    .expect("branch create"),
                CreateOutcome::Created,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        // The root `a` is descendant-only: no payload marker, so a whole read is
        // payload-absent and presence is absent, while a replace reports Missing
        // without touching the descendant. The branch entry itself is present.
        {
            let mut read = store
                .read_session(InvocationGrant::full_store(), read_demand())
                .expect("read session");
            let root = read.site(0);
            assert_eq!(
                read.read_entry(&root, std::slice::from_ref(&book)),
                Ok(None),
                "a descendant-only root reads payload-absent",
            );
            assert_eq!(
                read.presence(&root, std::slice::from_ref(&book)),
                Ok(Presence::Absent),
                "a descendant-only root has no payload marker",
            );
            let branch = read.site(1);
            assert_eq!(
                read.presence(&branch, &note),
                Ok(Presence::Present),
                "the branch entry is present",
            );
        }

        // A replace over the descendant-only root reports Missing (no payload to
        // replace) and leaves the branch untouched.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let root = txn.site(0);
            let entry = EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("late".into())))],
            };
            assert_eq!(
                txn.replace_entry(&root, std::slice::from_ref(&book), entry)
                    .expect("root replace"),
                ReplaceOutcome::Missing,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        // Create the root `a` payload over the descendant-only slot: this writes the
        // root marker and `title` without touching the branch descendant.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let root = txn.site(0);
            let entry = EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str(
                    "Book A".into(),
                )))],
            };
            assert_eq!(
                txn.create_entry(&root, std::slice::from_ref(&book), entry)
                    .expect("root create"),
                CreateOutcome::Created,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        // The root now has a payload and the branch descendant survived the create.
        {
            let mut read = store
                .read_session(InvocationGrant::full_store(), read_demand())
                .expect("read session");
            let root = read.site(0);
            assert_eq!(
                read.read_entry(&root, std::slice::from_ref(&book)),
                Ok(Some(EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str(
                        "Book A".into()
                    )))],
                })),
                "the root create gave the descendant-only node a payload",
            );
            let branch = read.site(1);
            assert_eq!(
                read.read_entry(&branch, &note),
                Ok(Some(EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
                })),
                "the branch descendant survived the root create",
            );
        }
    }

    // --- Corrupt / valid / orphan corpus (store-level byte injection). ---
    //
    // These seed cells directly through the engine seam — not through the session
    // ops — to place the store in states the ops alone cannot construct, then read
    // through a coherent session. They pin the corrupt/valid boundary the bounded
    // prefix probe draws once a branch subtree can nest below a node: a marker-absent
    // node with a legitimate keyed descendant is *valid* (descendant-only,
    // payload-absent), while a marker-absent node with one of its *own* field leaves
    // is *corrupt* — and the own-leaf corruption is surfaced ahead of the legitimate
    // descendant (the `0x10 < 0x30` precedence).

    /// The byte prefix (marker stem) of a `books` root entry.
    fn book_stem(key: &str) -> Vec<u8> {
        physical::marker_key("books", &[KeyScalar::Str(key.into())])
    }

    /// Seed `cells` (key, value pairs) into a fresh engine and wrap it in a branch-schema
    /// store, so a read session observes exactly the injected bytes.
    fn injected_branch_store(cells: &[(Vec<u8>, Vec<u8>)]) -> DurableStore<MemoryEngine> {
        let mut engine = MemoryEngine::new();
        {
            let mut txn = engine.begin().expect("begin");
            for (key, value) in cells {
                txn.put(key, value.clone()).expect("seed cell");
            }
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let (schema, sites) = branch_schema();
        DurableStore::from_engine(engine, schema, sites)
    }

    /// VALID: a branch child (marker plus its own `text` leaf) under an absent root is a
    /// legitimate descendant-only node. A whole read of the root is payload-absent, not
    /// corruption, and the branch entry reads back — the byte-injected counterpart of the
    /// ops-built descendant-only case.
    #[test]
    fn an_injected_descendant_only_node_reads_payload_absent_not_corruption() {
        let stem = book_stem("a");
        let branch_stem = physical::branch_child_stem(&stem, "notes", &[KeyScalar::Int(7)]);
        let mut store = injected_branch_store(&[
            (branch_stem.clone(), physical::MARKER_VALUE.to_vec()),
            (
                physical::stem_field_leaf(&branch_stem, "text"),
                b"hi".to_vec(),
            ),
        ]);
        let book = KeyScalar::Str("a".into());
        let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        assert_eq!(
            read.read_entry(&root, std::slice::from_ref(&book)),
            Ok(None),
            "a marker-absent node with only a keyed descendant is a valid descendant-only node",
        );
        assert_eq!(
            read.presence(&root, std::slice::from_ref(&book)),
            Ok(Presence::Absent),
        );
        let branch = read.site(1);
        assert_eq!(
            read.read_entry(&branch, &note),
            Ok(Some(EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
            })),
        );
    }

    /// CORRUPT (own-leaf precedence over a descendant): a root that has one of its own
    /// field leaves (`title`) but no marker is corrupt even when a legitimate branch
    /// descendant also exists below it. The bounded prefix probe meets the orphan own
    /// leaf (`0x10`) before the branch descendant (`0x30`), so it surfaces corruption
    /// rather than reading the node as a valid descendant-only slot.
    #[test]
    fn an_injected_root_own_leaf_without_a_marker_is_corruption_even_with_a_descendant() {
        let stem = book_stem("a");
        let branch_stem = physical::branch_child_stem(&stem, "notes", &[KeyScalar::Int(7)]);
        let mut store = injected_branch_store(&[
            // The root's own `title` leaf, with no root marker: an orphan.
            (
                physical::stem_field_leaf(&stem, "title"),
                b"Book A".to_vec(),
            ),
            // A legitimate branch descendant below the same (markerless) root.
            (branch_stem.clone(), physical::MARKER_VALUE.to_vec()),
            (
                physical::stem_field_leaf(&branch_stem, "text"),
                b"hi".to_vec(),
            ),
        ]);
        let book = KeyScalar::Str("a".into());
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        assert_eq!(
            read.read_entry(&root, std::slice::from_ref(&book)),
            Err(KernelFault::Corruption),
            "an orphan own leaf is surfaced ahead of a legitimate descendant",
        );
    }

    /// ORPHAN (branch level): a branch child that has its own `text` leaf but no branch
    /// marker is corrupt, exactly as a root orphan is — the marker/field law holds one
    /// level down.
    #[test]
    fn an_injected_branch_own_leaf_without_a_branch_marker_is_corruption() {
        let stem = book_stem("a");
        let branch_stem = physical::branch_child_stem(&stem, "notes", &[KeyScalar::Int(7)]);
        let mut store = injected_branch_store(&[
            // The root has a real payload, so the root itself is well-formed.
            (stem.clone(), physical::MARKER_VALUE.to_vec()),
            (
                physical::stem_field_leaf(&stem, "title"),
                b"Book A".to_vec(),
            ),
            // The branch child's own leaf with no branch marker: an orphan.
            (
                physical::stem_field_leaf(&branch_stem, "text"),
                b"hi".to_vec(),
            ),
        ]);
        let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let branch = read.site(1);
        assert_eq!(
            read.read_entry(&branch, &note),
            Err(KernelFault::Corruption),
            "a branch own leaf without its branch marker is corruption",
        );
    }

    /// The descendant-skip law of the bounded acquisition over a run of descendant-only
    /// entries: it freezes only payload-bearing (marker-present) entries, seeking a
    /// descendant-only entry's whole subtree in one cursor step. Present entries `k1`
    /// and `k4` bracket two descendant-only entries `k2` and `k3` — each a markerless
    /// root carrying only a keyed branch child — injected directly so the ops cannot
    /// construct the state. The acquisition from the start freezes `[k1, k4]`, skipping
    /// both; and an inclusive `from` inside the descendant-only run still resolves to
    /// `k4`, so the skip does not depend on starting at a present entry.
    #[test]
    fn a_bounded_acquisition_skips_a_run_of_descendant_only_entries_between_siblings() {
        let mut cells = Vec::new();
        // Present entries: a root marker plus its `title` leaf.
        for present in ["k1", "k4"] {
            let stem = book_stem(present);
            cells.push((stem.clone(), physical::MARKER_VALUE.to_vec()));
            cells.push((physical::stem_field_leaf(&stem, "title"), b"T".to_vec()));
        }
        // Descendant-only entries: a branch child (marker plus `text` leaf) with no
        // root marker, so the root has children but no visitable payload.
        for descendant_only in ["k2", "k3"] {
            let branch_stem = physical::branch_child_stem(
                &book_stem(descendant_only),
                "notes",
                &[KeyScalar::Int(7)],
            );
            cells.push((branch_stem.clone(), physical::MARKER_VALUE.to_vec()));
            cells.push((
                physical::stem_field_leaf(&branch_stem, "text"),
                b"hi".to_vec(),
            ));
        }
        let mut store = injected_branch_store(&cells);
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        let k = |s: &str| KeyScalar::Str(s.into());

        // From the start: the two present siblings, the two descendant-only entries
        // skipped in one seek run.
        assert_eq!(
            read.iterate_bounded(&root, &[], None, bound(8))
                .expect("iterate"),
            BoundedKeys {
                keys: vec![k("k1"), k("k4")],
                more: false,
            },
        );

        // An inclusive `from` inside the descendant-only run — at `k2`, the first of
        // the two, or `k3`, the second — resolves to `k4` just as a `from` at the
        // present sibling before them does.
        for start in ["k2", "k3"] {
            assert_eq!(
                read.iterate_bounded(&root, &[], Some(k(start)), bound(8))
                    .expect("iterate")
                    .keys,
                vec![k("k4")],
                "an inclusive from inside the descendant-only run still yields the next present key",
            );
        }
    }

    // --- Field-exact branch operations and the branch commit reconcile (E03w slice A). ---
    //
    // A field-exact set on a branch entry addresses one leaf of a branch node directly
    // (`BranchField`). Its engine write is one cell regardless of the branch record's
    // width (constant records), and the commit reconcile validates the *branch* node's
    // marker and required fields at its own stem — never the root's.

    /// A wide-record branch schema: root `books` keyed by string with a required
    /// `title`, and a branch `notes` keyed by int with a required `text` plus six sparse
    /// `f0..f5` fields. The site table addresses the root (0), the branch entry (1), the
    /// middle sparse branch field `f2` (2, branch field index 3), and the required branch
    /// field `text` (3, branch field index 0).
    fn wide_branch_schema() -> (StoreSchema, Vec<SiteSpec>) {
        let mut branch_fields = vec![FieldSchema::scalar("text", ScalarKind::Str, true)];
        for i in 0..6 {
            branch_fields.push(FieldSchema::scalar(format!("f{i}"), ScalarKind::Int, false));
        }
        let schema = StoreSchema {
            root_name: "books".into(),
            key: vec![ScalarKind::Str],
            fields: vec![FieldSchema::scalar("title", ScalarKind::Str, true)],
            branches: vec![BranchSchema {
                name: "notes".into(),
                key: vec![ScalarKind::Int],
                fields: branch_fields,
                branches: Vec::new(),
            }],
            indexes: Vec::new(),
        };
        let sites = vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: branch_entry(&[0]),
            },
            SiteSpec {
                target: branch_field(&[0], 3),
            },
            SiteSpec {
                target: branch_field(&[0], 0),
            },
        ];
        (schema, sites)
    }

    /// Every raw cell of a store, as an owned key→value map (the test stores are small
    /// enough that one page holds them all).
    fn all_cells(
        store: &DurableStore<MemoryEngine>,
    ) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
        let view = store.engine.read_view().expect("read view");
        view.scan_after(&[], &[])
            .expect("scan")
            .into_iter()
            .map(|(key, value)| (key.to_vec(), value.to_vec()))
            .collect()
    }

    /// The physical marker stem of note `note` under book `book`.
    fn note_stem(book: &str, note: i64) -> Vec<u8> {
        physical::branch_child_stem(
            &physical::marker_key("books", &[KeyScalar::Str(book.into())]),
            "notes",
            &[KeyScalar::Int(note)],
        )
    }

    /// A field-exact set on a present wide-record branch entry writes exactly one new
    /// leaf cell, independent of the branch record's width, and leaves every other cell
    /// (the marker, the required `text`, and the untouched sparse fields) byte-identical.
    /// This is the branch wide-resource evidence: field-exact write work is O(1) plus the
    /// node's own incident cells, not proportional to the record width.
    #[test]
    fn a_field_exact_branch_set_writes_one_leaf_regardless_of_branch_width() {
        let (schema, sites) = wide_branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];

        // Create the branch entry with only its required `text` present.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let branch = txn.site(1);
            let mut fields = vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))];
            fields.extend(std::iter::repeat_n(None, 6));
            txn.create_entry(&branch, &note, EntryValue { fields })
                .expect("branch create");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let before = all_cells(&store);

        // A field-exact set of one middle sparse field on the present wide branch entry.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let f2 = txn.site(2);
            txn.set_sparse(
                &f2,
                &note,
                Some(ValueDomain::Scalar(RuntimeScalar::Int(42))),
            )
            .expect("field-exact set");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let after = all_cells(&store);

        assert_eq!(
            after.len(),
            before.len() + 1,
            "a field-exact set on a 7-field branch record writes exactly one new leaf",
        );
        // Every pre-existing cell is byte-identical, except the per-commit witness token
        // (commit metadata, not application data): the write touched only the one leaf.
        let witness = physical::meta_key(super::WITNESS);
        for (key, value) in &before {
            if key == &witness {
                continue;
            }
            assert_eq!(
                after.get(key),
                Some(value),
                "a field-exact set left every prior cell untouched",
            );
        }
    }

    /// The branch commit reconcile creates the *branch* node's marker (never the root's)
    /// when a field-exact required set stages the branch node with all required fields
    /// present. Site 3 is the required `text` branch field; setting it on an absent
    /// branch entry reconcile-creates the branch marker, and the root gains no marker.
    #[test]
    fn a_field_exact_required_branch_set_reconcile_creates_the_branch_marker() {
        let (schema, sites) = wide_branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];

        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let text = txn.site(3);
            txn.set_required(
                &text,
                &note,
                ValueDomain::Scalar(RuntimeScalar::Str("made".into())),
            )
            .expect("required branch set");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        let cells = all_cells(&store);
        assert!(
            cells.contains_key(&note_stem("a", 7)),
            "the reconcile created the branch node's marker",
        );
        assert!(
            !cells.contains_key(&physical::marker_key(
                "books",
                &[KeyScalar::Str("a".into())]
            )),
            "a field-exact branch set does not create the root marker",
        );
    }

    /// A staged sparse branch-field set whose branch node's required field is missing
    /// rolls the transaction back with `RequiredMissing` — validated at the branch node's
    /// own stem and record, not the root's. Nothing persists, proving the reconcile
    /// checked the branch node's required `text` rather than the root's `title`.
    #[test]
    fn a_sparse_branch_set_missing_the_branch_required_field_rolls_back() {
        let (schema, sites) = wide_branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];
        let before = all_cells(&store);

        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let f2 = txn.site(2);
        txn.set_sparse(&f2, &note, Some(ValueDomain::Scalar(RuntimeScalar::Int(9))))
            .expect("field-exact sparse set");
        // The branch node's required `text` is missing, so commit rolls back.
        assert!(matches!(txn.commit(), CommitResult::RequiredMissing { .. }));
        // The whole transaction aborted, including the profile provision: nothing persists.
        assert_eq!(
            all_cells(&store),
            before,
            "the rolled-back set persisted nothing"
        );
    }

    // --- E04 bounded acquisition law (the freeze-then-run kernel primitive). ---
    //
    // `iterate_bounded` freezes the first N immediate keys of a durable layer and
    // reports whether an (N+1)th existed. It is the bounded, cursor-free acquisition
    // the `for … at most N … on more` form runs over: the keys are captured up front
    // (so loop-body writes cannot change the frozen set), a descendant-only child is
    // skipped by one prefix-successor seek, and an inclusive `from` bounds the start.

    fn bound(n: u32) -> BoundedLimit {
        BoundedLimit::new(n).expect("a positive traversal bound")
    }

    /// Create `names` as present root entries (a required `value`, no `label`) in one
    /// committed transaction over the flat `counters` schema.
    fn seed_root(store: &mut DurableStore<MemoryEngine>, names: &[&str]) {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let entry = txn.site(0);
        for name in names {
            txn.create_entry(&entry, &[KeyScalar::Str((*name).into())], value_entry(1))
                .expect("create");
        }
        assert_eq!(txn.commit(), CommitResult::Committed);
    }

    /// Freeze up to `n` root keys of the `counters` store, starting inclusively at
    /// `from` when given.
    fn freeze_root(
        store: &mut DurableStore<MemoryEngine>,
        from: Option<&str>,
        n: u32,
    ) -> BoundedKeys {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        read.iterate_bounded(&root, &[], from.map(|s| KeyScalar::Str(s.into())), bound(n))
            .expect("iterate")
    }

    fn strs(names: &[&str]) -> Vec<KeyScalar> {
        names.iter().map(|s| KeyScalar::Str((*s).into())).collect()
    }

    /// The freeze law: the frozen set is the first N present keys in ascending order,
    /// and `more` is set exactly when an (N+1)th key exists — regardless of insertion
    /// order.
    #[test]
    fn bounded_acquisition_freezes_the_first_n_and_flags_a_further_key() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        seed_root(&mut store, &["c", "a", "e", "b", "d"]); // inserted out of order

        // N below the population: the first N, ascending, with `more` set.
        assert_eq!(
            freeze_root(&mut store, None, 3),
            BoundedKeys {
                keys: strs(&["a", "b", "c"]),
                more: true,
            },
        );
        // N equal to the population: every key, `more` clear (no (N+1)th exists).
        assert_eq!(
            freeze_root(&mut store, None, 5),
            BoundedKeys {
                keys: strs(&["a", "b", "c", "d", "e"]),
                more: false,
            },
        );
        // N above the population: every key, `more` clear.
        assert_eq!(
            freeze_root(&mut store, None, 9),
            BoundedKeys {
                keys: strs(&["a", "b", "c", "d", "e"]),
                more: false,
            },
        );
    }

    /// The 0/1/N/N+1 boundary of the population against a fixed bound N=2.
    #[test]
    fn bounded_acquisition_covers_the_population_boundary() {
        // 0 present: empty frozen set, no more.
        let mut empty = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        assert_eq!(
            freeze_root(&mut empty, None, 2),
            BoundedKeys {
                keys: vec![],
                more: false,
            },
        );

        // 1 present (< N): the one key, no more.
        let mut one = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        seed_root(&mut one, &["a"]);
        assert_eq!(
            freeze_root(&mut one, None, 2),
            BoundedKeys {
                keys: strs(&["a"]),
                more: false,
            },
        );

        // Exactly N present: both keys, no more (the (N+1)th does not exist).
        let mut exact = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        seed_root(&mut exact, &["a", "b"]);
        assert_eq!(
            freeze_root(&mut exact, None, 2),
            BoundedKeys {
                keys: strs(&["a", "b"]),
                more: false,
            },
        );

        // N+1 present: the first N frozen, `more` set (the third is probed, not frozen).
        let mut over = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        seed_root(&mut over, &["a", "b", "c"]);
        assert_eq!(
            freeze_root(&mut over, None, 2),
            BoundedKeys {
                keys: strs(&["a", "b"]),
                more: true,
            },
        );
    }

    /// The inclusive `from` lower bound: the walk begins at `from` when present, else at
    /// the first present key above it, and is otherwise frozen and flagged as usual.
    #[test]
    fn bounded_acquisition_from_is_an_inclusive_lower_bound() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema(), sites());
        seed_root(&mut store, &["a", "c", "e"]);

        // `from` a present key: inclusive — the frozen set starts at it.
        assert_eq!(
            freeze_root(&mut store, Some("c"), 5),
            BoundedKeys {
                keys: strs(&["c", "e"]),
                more: false,
            },
        );
        // `from` between two keys: starts at the first present key above it.
        assert_eq!(
            freeze_root(&mut store, Some("b"), 5),
            BoundedKeys {
                keys: strs(&["c", "e"]),
                more: false,
            },
        );
        // `from` at the least key: inclusive, the whole layer.
        assert_eq!(
            freeze_root(&mut store, Some("a"), 5),
            BoundedKeys {
                keys: strs(&["a", "c", "e"]),
                more: false,
            },
        );
        // `from` above every key: empty.
        assert_eq!(
            freeze_root(&mut store, Some("z"), 5),
            BoundedKeys {
                keys: vec![],
                more: false,
            },
        );
        // `from` combines with the bound: the (N+1)th key above `from` sets `more`.
        assert_eq!(
            freeze_root(&mut store, Some("c"), 1),
            BoundedKeys {
                keys: strs(&["c"]),
                more: true,
            },
        );
    }

    /// Descendant-only entries — markerless roots carrying only a keyed branch child —
    /// are skipped by the bounded walk with one prefix-successor seek per run, so the
    /// frozen set holds only payload-bearing roots, and the (N+1) probe skips a
    /// descendant-only run to reach a real key.
    #[test]
    fn bounded_acquisition_skips_descendant_only_entries() {
        let mut cells = Vec::new();
        for present in ["k1", "k4"] {
            let stem = book_stem(present);
            cells.push((stem.clone(), physical::MARKER_VALUE.to_vec()));
            cells.push((physical::stem_field_leaf(&stem, "title"), b"T".to_vec()));
        }
        for descendant_only in ["k2", "k3"] {
            let branch_stem = physical::branch_child_stem(
                &book_stem(descendant_only),
                "notes",
                &[KeyScalar::Int(7)],
            );
            cells.push((branch_stem.clone(), physical::MARKER_VALUE.to_vec()));
            cells.push((
                physical::stem_field_leaf(&branch_stem, "text"),
                b"hi".to_vec(),
            ));
        }
        let mut store = injected_branch_store(&cells);
        let k = |s: &str| KeyScalar::Str(s.into());
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);

        // A generous bound freezes only the two present roots, skipping the two
        // descendant-only entries between them.
        assert_eq!(
            read.iterate_bounded(&root, &[], None, bound(10)),
            Ok(BoundedKeys {
                keys: vec![k("k1"), k("k4")],
                more: false,
            }),
        );
        // With N=1 the (N+1) probe skips the descendant-only run k2,k3 to reach k4, so
        // `more` is set although the two intervening entries carry no payload.
        assert_eq!(
            read.iterate_bounded(&root, &[], None, bound(1)),
            Ok(BoundedKeys {
                keys: vec![k("k1")],
                more: true,
            }),
        );
    }

    /// Bounded work over fan-out: a present root with a large branch subtree is passed
    /// by one prefix-successor seek to reach the next root, so root-layer freezing never
    /// reads the subtree — the frozen set is the roots, not their descendants.
    #[test]
    fn bounded_acquisition_skips_a_large_descendant_fan_out_in_one_seek() {
        let (schema, sites) = branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let root = txn.site(0);
            let branch = txn.site(1);
            for book in ["a", "b"] {
                let title = EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("T".into())))],
                };
                txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                    .expect("root create");
            }
            // A large branch fan-out under book "a" the root walk must skip wholesale.
            for note in 0..200i64 {
                let text = EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
                };
                txn.create_entry(
                    &branch,
                    &[KeyScalar::Str("a".into()), KeyScalar::Int(note)],
                    text,
                )
                .expect("note create");
            }
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        let k = |s: &str| KeyScalar::Str(s.into());

        // The root layer freezes only the two book roots; book "a"'s 200-note subtree
        // is skipped in one seek to reach "b".
        assert_eq!(
            read.iterate_bounded(&root, &[], None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![k("a"), k("b")],
                more: false,
            }),
        );
        // With N=1, "b" is the (N+1) probe reached past "a"'s whole fan-out.
        assert_eq!(
            read.iterate_bounded(&root, &[], None, bound(1)),
            Ok(BoundedKeys {
                keys: vec![k("a")],
                more: true,
            }),
        );
    }

    /// Branch-layer traversal: freezing the immediate keys of a keyed branch beneath a
    /// fixed root entry. The frozen set is that branch's own keys, scoped to the given
    /// root key (a sibling root's branch of the same name is not visited), with the same
    /// freeze / `more` / inclusive-`from` law as the root layer, one level down.
    #[test]
    fn bounded_acquisition_traverses_a_branch_layer_under_a_fixed_root_key() {
        let (schema, sites) = branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let root = txn.site(0);
            let branch = txn.site(1);
            for book in ["a", "b"] {
                let title = EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("T".into())))],
                };
                txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                    .expect("root create");
            }
            // Notes 10,20,30 under "a"; a decoy note 5 under sibling root "b".
            for note in [10i64, 20, 30] {
                let text = EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
                };
                txn.create_entry(
                    &branch,
                    &[KeyScalar::Str("a".into()), KeyScalar::Int(note)],
                    text,
                )
                .expect("note create");
            }
            let decoy = EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into())))],
            };
            txn.create_entry(
                &branch,
                &[KeyScalar::Str("b".into()), KeyScalar::Int(5)],
                decoy,
            )
            .expect("decoy create");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let branch = read.site(1);
        let a = [KeyScalar::Str("a".into())];
        let int = KeyScalar::Int;

        // Freeze the notes under "a": bounded and scoped, with `more` when an (N+1)th
        // note exists.
        assert_eq!(
            read.iterate_bounded(&branch, &a, None, bound(2)),
            Ok(BoundedKeys {
                keys: vec![int(10), int(20)],
                more: true,
            }),
        );
        assert_eq!(
            read.iterate_bounded(&branch, &a, None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![int(10), int(20), int(30)],
                more: false,
            }),
            "the branch layer is scoped to root a — b's note key 5 is not visited",
        );
        // Inclusive `from` within the branch layer.
        assert_eq!(
            read.iterate_bounded(&branch, &a, Some(int(20)), bound(5)),
            Ok(BoundedKeys {
                keys: vec![int(20), int(30)],
                more: false,
            }),
        );

        // A different fixed root key sees its own branch layer; an absent root key none.
        let mut read2 = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let branch2 = read2.site(1);
        assert_eq!(
            read2.iterate_bounded(&branch2, &[KeyScalar::Str("b".into())], None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![int(5)],
                more: false,
            }),
        );
        assert_eq!(
            read2.iterate_bounded(&branch2, &[KeyScalar::Str("c".into())], None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![],
                more: false,
            }),
        );
    }

    /// The family-populated probe over a keyed branch family (the `notes` layer under one
    /// book): `Present` when the book has at least one note, `Absent` when it has none or
    /// is itself absent — the E06 "does this asset have notes?" question. The probe reads
    /// the branch layer scoped to the fixed parent key, so one book's notes never make a
    /// sibling's family read populated.
    #[test]
    fn family_populated_answers_whether_a_branch_family_has_a_child() {
        let (schema, sites) = branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let root = txn.site(0);
            let branch = txn.site(1);
            for book in ["a", "b"] {
                let title = EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("T".into())))],
                };
                txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                    .expect("root create");
            }
            // Only book "a" gets a note; book "b" stays note-less.
            let text = EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
            };
            txn.create_entry(
                &branch,
                &[KeyScalar::Str("a".into()), KeyScalar::Int(1)],
                text,
            )
            .expect("note create");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        let branch = read.site(1);
        // The root family is populated (two books exist).
        assert_eq!(read.family_populated(&root, &[]), Ok(Presence::Present));
        // Book "a" has a note; book "b" and an absent book "c" have none.
        assert_eq!(
            read.family_populated(&branch, &[KeyScalar::Str("a".into())]),
            Ok(Presence::Present),
        );
        assert_eq!(
            read.family_populated(&branch, &[KeyScalar::Str("b".into())]),
            Ok(Presence::Absent),
        );
        assert_eq!(
            read.family_populated(&branch, &[KeyScalar::Str("c".into())]),
            Ok(Presence::Absent),
        );
    }

    /// A family whose only children are descendant-only (markerless — children below them
    /// but no payload of their own) reads `Absent`: the probe skips each descendant-only
    /// child by one seek exactly as the bounded traversal does, so a family with no
    /// payload-bearing child is not populated. An empty root family likewise reads
    /// `Absent`.
    #[test]
    fn family_populated_skips_descendant_only_children_and_empty_families() {
        let (schema, sites) = branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        // A fresh store: the root family is empty.
        {
            let mut read = store
                .read_session(InvocationGrant::full_store(), read_demand())
                .expect("read session");
            let root = read.site(0);
            assert_eq!(read.family_populated(&root, &[]), Ok(Presence::Absent));
        }
        // Give book "a" a note but never a payload marker of its own: "a" is a
        // descendant-only child of the root family. The root family must still read
        // `Absent` — it holds no payload-bearing book.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let branch = txn.site(1);
            let text = EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
            };
            txn.create_entry(
                &branch,
                &[KeyScalar::Str("a".into()), KeyScalar::Int(1)],
                text,
            )
            .expect("note create");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        let branch = read.site(1);
        // "a" has no own payload marker, only a note beneath it.
        assert_eq!(
            read.presence(&root, &[KeyScalar::Str("a".into())]),
            Ok(Presence::Absent)
        );
        // So the root family is not populated, but "a"'s own notes family is.
        assert_eq!(read.family_populated(&root, &[]), Ok(Presence::Absent));
        assert_eq!(
            read.family_populated(&branch, &[KeyScalar::Str("a".into())]),
            Ok(Presence::Present),
        );
    }

    /// `layer_of`'s hard backstop over the trust boundary (matching `node_stem`): a branch
    /// layer's ancestor key-path must be the root key then one key per parent hop. A wrong
    /// ancestor arity or a wrong ancestor key kind faults `Corruption` rather than
    /// mis-layering the traversal to the root entry family (which would leak the wrong
    /// layer's keys). The verifier proves the arity and kinds, so this is the release
    /// backstop a forged image cannot slip past.
    #[test]
    fn a_branch_layer_traversal_with_a_wrong_ancestor_key_path_faults() {
        let (schema, sites) = branch_schema();
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let branch = read.site(1); // a single-level branch site (needs `[root_key]`)

        // Empty ancestor path: a branch layer needs one ancestor key; zero is a wrong
        // arity that must fault rather than mis-layer to the root's own entry family.
        assert_eq!(
            read.iterate_bounded(&branch, &[], None, bound(4)),
            Err(KernelFault::Corruption),
        );
        // Two ancestor keys where the single-level branch layer needs one: wrong arity.
        assert_eq!(
            read.iterate_bounded(
                &branch,
                &[KeyScalar::Str("a".into()), KeyScalar::Str("b".into())],
                None,
                bound(4),
            ),
            Err(KernelFault::Corruption),
        );
        // Right arity, wrong ancestor kind: the root key is a string, so an int ancestor
        // key is a scalar-kind mismatch at the trust boundary.
        assert_eq!(
            read.iterate_bounded(&branch, &[KeyScalar::Int(0)], None, bound(4)),
            Err(KernelFault::Corruption),
        );
    }

    // --- Nested (multi-level) branches: multi-hop stems, four-state probe at depth, the
    //     sub-branch uniform payload-only law, node-parametric reconcile at depth, and
    //     bounded traversal over an inner layer (E03w slice B). ---
    //
    // The verifier still parks nested branches, so these tests hand-build a multi-level
    // schema and multi-hop sites and drive the public store API directly — the kernel
    // executes any well-formed `StoreSchema` + `SiteSpec`, the seam the verifier/compiler
    // admission of nested branches (checkpoint 1) will target. They pin the level-
    // independence of the durable laws: a sub-branch node's marker/field/cursor topology,
    // its slot classification, its whole-entry replace/erase confinement, and its commit
    // reconcile all behave one or two levels down exactly as they do at the root.

    /// A four-level nested-branch schema: root `books`(Str) → branch `notes`(Int) →
    /// sub-branch `tags`(Str) → sub-sub-branch `links`(Int). Each branch level carries a
    /// required and a sparse field so the payload-only law (replace erases omitted own
    /// fields; erase removes own cells; both preserve keyed descendants) and the node-
    /// parametric reconcile are exercised at depth. Sites: 0 root, 1 notes, 2 tags, 3
    /// links, 4 tags.weight (sparse), 5 tags.label (required), 6 notes.color (sparse).
    fn nested_schema() -> (StoreSchema, Vec<SiteSpec>) {
        let links = BranchSchema {
            name: "links".into(),
            key: vec![ScalarKind::Int],
            fields: vec![FieldSchema::scalar("url", ScalarKind::Str, false)],
            branches: Vec::new(),
        };
        let tags = BranchSchema {
            name: "tags".into(),
            key: vec![ScalarKind::Str],
            fields: vec![
                FieldSchema::scalar("label", ScalarKind::Str, true),
                FieldSchema::scalar("weight", ScalarKind::Int, false),
            ],
            branches: vec![links],
        };
        let notes = BranchSchema {
            name: "notes".into(),
            key: vec![ScalarKind::Int],
            fields: vec![
                FieldSchema::scalar("text", ScalarKind::Str, true),
                FieldSchema::scalar("color", ScalarKind::Str, false),
            ],
            branches: vec![tags],
        };
        let schema = StoreSchema {
            root_name: "books".into(),
            key: vec![ScalarKind::Str],
            fields: vec![FieldSchema::scalar("title", ScalarKind::Str, true)],
            branches: vec![notes],
            indexes: Vec::new(),
        };
        let sites = vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: branch_entry(&[0]),
            },
            SiteSpec {
                target: branch_entry(&[0, 0]),
            },
            SiteSpec {
                target: branch_entry(&[0, 0, 0]),
            },
            SiteSpec {
                target: branch_field(&[0, 0], 1),
            },
            SiteSpec {
                target: branch_field(&[0, 0], 0),
            },
            SiteSpec {
                target: branch_field(&[0], 1),
            },
        ];
        (schema, sites)
    }

    fn ks(s: &str) -> KeyScalar {
        KeyScalar::Str(s.into())
    }
    fn ki(n: i64) -> KeyScalar {
        KeyScalar::Int(n)
    }
    fn vs(s: &str) -> Option<ValueDomain> {
        Some(ValueDomain::Scalar(RuntimeScalar::Str(s.into())))
    }
    fn vi(n: i64) -> Option<ValueDomain> {
        Some(ValueDomain::Scalar(RuntimeScalar::Int(n)))
    }

    /// The physical marker stem of book `book` (level 0).
    fn nested_book_stem(book: &str) -> Vec<u8> {
        physical::marker_key("books", &[ks(book)])
    }
    /// The physical marker stem of note `note` under `book` (level 1).
    fn nested_note_stem(book: &str, note: i64) -> Vec<u8> {
        physical::branch_child_stem(&nested_book_stem(book), "notes", &[ki(note)])
    }
    /// The physical marker stem of tag `tag` under `book`/`note` (level 2).
    fn nested_tag_stem(book: &str, note: i64, tag: &str) -> Vec<u8> {
        physical::branch_child_stem(&nested_note_stem(book, note), "tags", &[ks(tag)])
    }

    fn nested_store() -> DurableStore<MemoryEngine> {
        let (schema, sites) = nested_schema();
        DurableStore::from_engine(MemoryEngine::new(), schema, sites)
    }

    /// Seed raw `cells` into a fresh engine wrapped in the nested schema, so a read
    /// session observes exactly the injected bytes — the multi-hop counterpart of
    /// `injected_branch_store`, for states the ops cannot construct.
    fn injected_nested_store(cells: &[(Vec<u8>, Vec<u8>)]) -> DurableStore<MemoryEngine> {
        let mut engine = MemoryEngine::new();
        {
            let mut txn = engine.begin().expect("begin");
            for (key, value) in cells {
                txn.put(key, value.clone()).expect("seed cell");
            }
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let (schema, sites) = nested_schema();
        DurableStore::from_engine(engine, schema, sites)
    }

    /// Every raw cell whose key lies under `prefix`, as an owned map. A node's marker is a
    /// byte-prefix of its whole subtree, so passing a node's marker stem captures exactly
    /// that node and every descendant.
    fn cells_under(
        store: &DurableStore<MemoryEngine>,
        prefix: &[u8],
    ) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
        all_cells(store)
            .into_iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .collect()
    }

    /// Create a whole entry at `site` with `fields`, committing the transaction.
    fn create_at(
        store: &mut DurableStore<MemoryEngine>,
        site: u16,
        keys: &[KeyScalar],
        fields: Vec<Option<ValueDomain>>,
    ) {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let target = txn.site(site);
        assert_eq!(
            txn.create_entry(&target, keys, EntryValue { fields })
                .expect("create"),
            CreateOutcome::Created,
        );
        assert_eq!(txn.commit(), CommitResult::Committed);
    }

    /// A whole-entry create at a level-2 (tags) node writes its marker and own field
    /// leaves under the multi-hop stem `books/book → notes/note → tags/tag`, and nowhere
    /// shallower; the entry reads back through its multi-hop site. Marker, field leaf, and
    /// the read path all resolve the same two-branch-hop stem.
    #[test]
    fn a_nested_branch_entry_addresses_its_multi_hop_stem_and_reads_back() {
        let mut store = nested_store();
        let tag = [ks("a"), ki(7), ks("x")];
        create_at(&mut store, 2, &tag, vec![vs("home"), None]);

        let cells = all_cells(&store);
        let tag_stem = nested_tag_stem("a", 7, "x");
        assert!(
            cells.contains_key(&tag_stem),
            "the tags marker sits at the multi-hop stem",
        );
        assert!(
            cells.contains_key(&physical::stem_field_leaf(&tag_stem, "label")),
            "the tags label leaf hangs off the multi-hop stem",
        );
        // A nested entry create writes no shallower marker: neither the parent note nor
        // the root book gains a payload marker.
        assert!(
            !cells.contains_key(&nested_note_stem("a", 7)),
            "the parent note node has no marker",
        );
        assert!(
            !cells.contains_key(&nested_book_stem("a")),
            "the root book node has no marker",
        );

        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let tags = read.site(2);
        assert_eq!(
            read.read_entry(&tags, &tag),
            Ok(Some(EntryValue {
                fields: vec![vs("home"), None],
            })),
            "the level-2 entry reads back through its multi-hop site",
        );
    }

    /// The four-state slot probe at a level-2 (tags) inner node: present (marker), absent
    /// (nothing), descendant-only (a level-3 links child, no tags marker), each read
    /// through the ops. The probe classifies the 3-hop stem exactly as it does the root.
    #[test]
    fn probe_slot_present_absent_and_descendant_only_at_a_level_two_node() {
        let mut store = nested_store();
        let present = [ks("a"), ki(1), ks("p")];
        let descendant_only = [ks("a"), ki(1), ks("d")];
        let absent = [ks("a"), ki(1), ks("z")];
        // Present: a tags entry with its required label.
        create_at(&mut store, 2, &present, vec![vs("home"), None]);
        // Descendant-only: a level-3 links child under tag "d", with no tags "d" marker.
        create_at(
            &mut store,
            3,
            &[ks("a"), ki(1), ks("d"), ki(100)],
            vec![vs("u")],
        );

        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let tags = read.site(2);
        assert!(
            matches!(read.read_entry(&tags, &present), Ok(Some(_))),
            "a present inner node reads a payload",
        );
        assert_eq!(
            read.read_entry(&tags, &absent),
            Ok(None),
            "an absent inner node reads payload-absent",
        );
        assert_eq!(read.presence(&tags, &absent), Ok(Presence::Absent));
        assert_eq!(
            read.read_entry(&tags, &descendant_only),
            Ok(None),
            "a markerless inner node with only a keyed descendant reads payload-absent",
        );
        assert_eq!(read.presence(&tags, &descendant_only), Ok(Presence::Absent));
    }

    /// The fourth probe state at a level-2 node: an injected tags own field leaf (`label`)
    /// with no tags marker is an orphan — corruption on a committed read, exactly as a
    /// root orphan is. The marker/field law holds two levels down.
    #[test]
    fn an_injected_level_two_orphan_leaf_is_corruption() {
        let tag_stem = nested_tag_stem("a", 1, "x");
        let mut store = injected_nested_store(&[(
            physical::stem_field_leaf(&tag_stem, "label"),
            b"home".to_vec(),
        )]);
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let tags = read.site(2);
        assert_eq!(
            read.read_entry(&tags, &[ks("a"), ki(1), ks("x")]),
            Err(KernelFault::Corruption),
            "a tags own leaf without its tags marker is corruption two levels down",
        );
    }

    /// The sub-branch uniform payload-only law for REPLACE: a whole replace of a level-1
    /// branch entry erases its omitted own field and preserves its sub-branch subtree
    /// byte-identically. The note's `color` is dropped and `text` replaced, while its
    /// whole `tags` subtree (a tags entry with its own fields) survives untouched.
    #[test]
    fn a_replace_of_a_branch_entry_erases_omitted_fields_and_preserves_its_sub_branch_subtree() {
        let mut store = nested_store();
        let note = [ks("a"), ki(1)];
        let tag = [ks("a"), ki(1), ks("x")];
        create_at(&mut store, 1, &note, vec![vs("hi"), vs("red")]);
        create_at(&mut store, 2, &tag, vec![vs("home"), vi(5)]);
        let subtree_before = cells_under(&store, &nested_tag_stem("a", 1, "x"));

        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let notes = txn.site(1);
            assert_eq!(
                txn.replace_entry(
                    &notes,
                    &note,
                    EntryValue {
                        fields: vec![vs("bye"), None],
                    },
                )
                .expect("replace"),
                ReplaceOutcome::Replaced,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        let after = all_cells(&store);
        assert!(
            !after.contains_key(&physical::stem_field_leaf(
                &nested_note_stem("a", 1),
                "color"
            )),
            "the replace erased the omitted color field",
        );
        assert_eq!(
            cells_under(&store, &nested_tag_stem("a", 1, "x")),
            subtree_before,
            "the sub-branch subtree survived the parent replace byte-identically",
        );
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let notes = read.site(1);
        assert_eq!(
            read.read_entry(&notes, &note),
            Ok(Some(EntryValue {
                fields: vec![vs("bye"), None],
            })),
        );
        let tags = read.site(2);
        assert_eq!(
            read.read_entry(&tags, &tag),
            Ok(Some(EntryValue {
                fields: vec![vs("home"), vi(5)],
            })),
            "the preserved sub-branch entry still reads its own record",
        );
    }

    /// The sub-branch uniform payload-only law for ERASE: a whole erase of a level-1
    /// branch entry removes its marker and own field leaves and preserves its sub-branch
    /// subtree. The note becomes descendant-only (payload-absent) while its tags subtree
    /// survives and reads back.
    #[test]
    fn an_erase_of_a_branch_entry_removes_its_own_cells_and_preserves_its_sub_branch_subtree() {
        let mut store = nested_store();
        let note = [ks("a"), ki(1)];
        let tag = [ks("a"), ki(1), ks("x")];
        create_at(&mut store, 1, &note, vec![vs("hi"), vs("red")]);
        create_at(&mut store, 2, &tag, vec![vs("home"), vi(5)]);
        let subtree_before = cells_under(&store, &nested_tag_stem("a", 1, "x"));

        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let notes = txn.site(1);
            assert_eq!(
                txn.erase_entry(&notes, &note).expect("erase"),
                EraseOutcome::Erased,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        let after = all_cells(&store);
        assert!(
            !after.contains_key(&nested_note_stem("a", 1)),
            "the erase removed the note's own marker",
        );
        assert!(
            !after.contains_key(&physical::stem_field_leaf(
                &nested_note_stem("a", 1),
                "text"
            )),
            "the erase removed the note's own field leaf",
        );
        assert_eq!(
            cells_under(&store, &nested_tag_stem("a", 1, "x")),
            subtree_before,
            "the sub-branch subtree survived the parent erase byte-identically",
        );
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let notes = read.site(1);
        assert_eq!(
            read.read_entry(&notes, &note),
            Ok(None),
            "the erased note is now descendant-only (payload-absent)",
        );
        let tags = read.site(2);
        assert_eq!(
            read.read_entry(&tags, &tag),
            Ok(Some(EntryValue {
                fields: vec![vs("home"), vi(5)],
            })),
        );
    }

    /// The same law at the ROOT over both nested levels: a whole erase of a root book
    /// entry removes only its own marker and title and preserves its whole nested subtree
    /// (its note and the note's tag). The payload-only law is level-independent, so a root
    /// erase never reaches into either descendant level.
    #[test]
    fn a_root_erase_preserves_the_whole_nested_branch_subtree() {
        let mut store = nested_store();
        let book = [ks("a")];
        let note = [ks("a"), ki(1)];
        let tag = [ks("a"), ki(1), ks("x")];
        create_at(&mut store, 0, &book, vec![vs("Book A")]);
        create_at(&mut store, 1, &note, vec![vs("hi"), None]);
        create_at(&mut store, 2, &tag, vec![vs("home"), None]);
        // The book's subtree below its own payload: the note and tag descendants.
        let note_subtree = cells_under(&store, &nested_note_stem("a", 1));

        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let root = txn.site(0);
            assert_eq!(
                txn.erase_entry(&root, &book).expect("erase"),
                EraseOutcome::Erased,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }

        let after = all_cells(&store);
        assert!(
            !after.contains_key(&nested_book_stem("a")),
            "the root erase removed the book's own marker",
        );
        assert_eq!(
            cells_under(&store, &nested_note_stem("a", 1)),
            note_subtree,
            "both nested levels survived the root erase byte-identically",
        );
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let notes = read.site(1);
        assert_eq!(
            read.read_entry(&notes, &note),
            Ok(Some(EntryValue {
                fields: vec![vs("hi"), None],
            })),
            "the level-1 descendant survived",
        );
        let tags = read.site(2);
        assert_eq!(
            read.read_entry(&tags, &tag),
            Ok(Some(EntryValue {
                fields: vec![vs("home"), None],
            })),
            "the level-2 descendant survived",
        );
    }

    /// The node-parametric commit reconcile at a level-2 (tags) node: a field-exact set
    /// stages the sub-branch node and reconcile validates its OWN required fields at its
    /// OWN stem. A sparse `weight` set with the required `label` missing rolls back
    /// (validated against the tags record, not the note's or root's); a required `label`
    /// set reconcile-creates the tags marker two levels down and no shallower marker.
    #[test]
    fn a_field_exact_set_on_a_sub_branch_node_reconciles_at_its_own_stem() {
        let mut store = nested_store();
        let tag = [ks("a"), ki(1), ks("x")];

        // A sparse weight set leaves the tags required `label` unset: reconcile validates
        // the tags node's own record and rolls back, persisting nothing.
        let before = all_cells(&store);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let weight = txn.site(4);
            txn.set_sparse(&weight, &tag, vi(9)).expect("sparse set");
            assert!(matches!(txn.commit(), CommitResult::RequiredMissing { .. }));
        }
        assert_eq!(
            all_cells(&store),
            before,
            "the rolled-back sub-branch set persisted nothing",
        );

        // A required label set stages the tags node with its required field present:
        // reconcile creates the tags marker at its own 3-hop stem, never a shallower one.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let label = txn.site(5);
            txn.set_required(
                &label,
                &tag,
                ValueDomain::Scalar(RuntimeScalar::Str("home".into())),
            )
            .expect("required set");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let cells = all_cells(&store);
        assert!(
            cells.contains_key(&nested_tag_stem("a", 1, "x")),
            "reconcile created the sub-branch marker at its own stem",
        );
        assert!(
            !cells.contains_key(&nested_note_stem("a", 1)),
            "the parent note node gained no marker",
        );
        assert!(
            !cells.contains_key(&nested_book_stem("a")),
            "the root book node gained no marker",
        );
    }

    /// Bounded traversal over an inner (level-2 tags) layer: `layer_of` resolves the layer
    /// from a two-element ancestor key-path `[book, note]`, and the freeze / `more` /
    /// descendant-skip / inclusive-`from` laws hold one more level down. A descendant-only
    /// tag (a level-3 links child, no tags marker) is skipped, and the layer is scoped to
    /// the fixed note — a sibling note's tags are not visited.
    #[test]
    fn bounded_acquisition_traverses_a_level_two_layer_and_skips_a_descendant_only_tag() {
        let mut store = nested_store();
        // Present tags t10, t20, t30 under note (a, 1); a descendant-only tag t15 (a
        // links child, no tags marker) between t10 and t20; a decoy tag z under note 2.
        for tag in ["t10", "t20", "t30"] {
            create_at(
                &mut store,
                2,
                &[ks("a"), ki(1), ks(tag)],
                vec![vs("L"), None],
            );
        }
        create_at(
            &mut store,
            3,
            &[ks("a"), ki(1), ks("t15"), ki(1)],
            vec![vs("u")],
        );
        create_at(
            &mut store,
            2,
            &[ks("a"), ki(2), ks("z")],
            vec![vs("L"), None],
        );

        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let tags = read.site(2);
        let ancestor = [ks("a"), ki(1)];

        // Freeze the present tags under (a, 1), skipping the descendant-only t15; the
        // (N+1) probe reaches t30 so `more` is set.
        assert_eq!(
            read.iterate_bounded(&tags, &ancestor, None, bound(2)),
            Ok(BoundedKeys {
                keys: vec![ks("t10"), ks("t20")],
                more: true,
            }),
        );
        // A generous bound freezes all three present tags, still skipping t15; the layer
        // is scoped to note 1 so note 2's tag z is not visited.
        assert_eq!(
            read.iterate_bounded(&tags, &ancestor, None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![ks("t10"), ks("t20"), ks("t30")],
                more: false,
            }),
        );
        // Inclusive `from` within the level-2 layer.
        assert_eq!(
            read.iterate_bounded(&tags, &ancestor, Some(ks("t20")), bound(5)),
            Ok(BoundedKeys {
                keys: vec![ks("t20"), ks("t30")],
                more: false,
            }),
        );

        // A different fixed note sees its own tags layer; an absent note sees none.
        let mut read2 = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let tags2 = read2.site(2);
        assert_eq!(
            read2.iterate_bounded(&tags2, &[ks("a"), ki(2)], None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![ks("z")],
                more: false,
            }),
        );
        assert_eq!(
            read2.iterate_bounded(&tags2, &[ks("a"), ki(9)], None, bound(5)),
            Ok(BoundedKeys {
                keys: vec![],
                more: false,
            }),
        );
    }

    /// A two-column composite-key root addresses each entry by the whole tuple in column
    /// order. The columns are the SAME type (both int), so only column *order*
    /// distinguishes `[1, 2]` from `[2, 1]`: `node_stem` must split the key-path into the
    /// root's two columns and encode them in order, not merely check kinds. A wrong key
    /// arity — too few or too many columns — faults corruption at the trust boundary.
    #[test]
    fn a_composite_key_root_addresses_entries_by_the_ordered_tuple() {
        let schema = StoreSchema {
            root_name: "cells".into(),
            key: vec![ScalarKind::Int, ScalarKind::Int],
            fields: vec![FieldSchema::scalar("v", ScalarKind::Int, true)],
            branches: Vec::new(),
            indexes: Vec::new(),
        };
        let sites = vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: SiteTarget::FieldLeaf(0),
            },
        ];
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let entry = txn.site(0);
            txn.create_entry(
                &entry,
                &[ki(1), ki(2)],
                EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(42)))],
                },
            )
            .expect("create");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let entry = read.site(0);
        let field = read.site(1);
        // Present at the written tuple; absent at the transposed one — order is load-bearing
        // even with same-typed columns.
        assert_eq!(
            read.presence(&entry, &[ki(1), ki(2)]),
            Ok(Presence::Present)
        );
        assert_eq!(read.presence(&entry, &[ki(2), ki(1)]), Ok(Presence::Absent));
        assert_eq!(
            read.read_field(&field, &[ki(1), ki(2)]),
            Ok(Some(ValueDomain::Scalar(RuntimeScalar::Int(42))))
        );
        // A short or long key-path is a forged arity: corruption, never a mis-split write.
        assert_eq!(
            read.presence(&entry, &[ki(1)]),
            Err(KernelFault::Corruption)
        );
        assert_eq!(
            read.presence(&entry, &[ki(1), ki(2), ki(3)]),
            Err(KernelFault::Corruption)
        );
    }

    /// A composite-keyed layer is not traversable — the traversal machinery decodes one
    /// key per immediate child, so a multi-column traversed layer would mis-read. The
    /// verifier parks composite-key traversal; a forged image reaching the kernel with a
    /// composite whole-payload site faults at `layer_of` rather than mis-decoding.
    #[test]
    fn iterate_over_a_composite_keyed_root_layer_faults_corruption() {
        let schema = StoreSchema {
            root_name: "cells".into(),
            key: vec![ScalarKind::Int, ScalarKind::Int],
            fields: vec![FieldSchema::scalar("v", ScalarKind::Int, true)],
            branches: Vec::new(),
            indexes: Vec::new(),
        };
        let sites = vec![SiteSpec {
            target: SiteTarget::WholePayload,
        }];
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let entry = read.site(0);
        // The traversed root layer has two key columns: layer_of's single-column guard
        // faults corruption before any scan.
        assert_eq!(
            read.iterate_bounded(&entry, &[], None, bound(4)),
            Err(KernelFault::Corruption),
        );
    }

    /// A single-column branch layer beneath a COMPOSITE-keyed root traverses normally: the
    /// ancestor key-path locating the parent entry is the root's whole two-column tuple, so
    /// `layer_of` consumes it via `take_columns` over multiple ancestor columns. This is the
    /// works-side counterpart to the composite-layer traversal park.
    #[test]
    fn iterate_a_single_column_branch_under_a_composite_root_consumes_multi_column_ancestors() {
        let schema = StoreSchema {
            root_name: "grid".into(),
            key: vec![ScalarKind::Int, ScalarKind::Int],
            fields: vec![FieldSchema::scalar("label", ScalarKind::Str, false)],
            branches: vec![BranchSchema {
                name: "cell".into(),
                key: vec![ScalarKind::Int],
                fields: vec![FieldSchema::scalar("cval", ScalarKind::Int, true)],
                branches: Vec::new(),
            }],
            indexes: Vec::new(),
        };
        let sites = vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: SiteTarget::BranchEntry(Box::from([0u16])),
            },
        ];
        let mut store = DurableStore::from_engine(MemoryEngine::new(), schema, sites);
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .expect("txn session");
            let cell = txn.site(1);
            // Three cells under the composite root entry (1, 2), inserted out of order.
            for c in [3, 1, 2] {
                txn.create_entry(
                    &cell,
                    &[ki(1), ki(2), ki(c)],
                    EntryValue {
                        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(0)))],
                    },
                )
                .expect("create cell");
            }
            // A cell under a different composite root entry (9, 9), which must not appear.
            txn.create_entry(
                &cell,
                &[ki(9), ki(9), ki(100)],
                EntryValue {
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(0)))],
                },
            )
            .expect("create sibling cell");
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let cell = read.site(1);
        // Iterate the cell layer under the composite ancestor (1, 2): the two ancestor
        // columns locate the parent entry, and the branch keys come back in order.
        assert_eq!(
            read.iterate_bounded(&cell, &[ki(1), ki(2)], None, bound(10)),
            Ok(BoundedKeys {
                keys: vec![ki(1), ki(2), ki(3)],
                more: false,
            }),
        );
        // A wrong-arity ancestor path (one column where two are needed) faults.
        assert_eq!(
            read.iterate_bounded(&cell, &[ki(1)], None, bound(10)),
            Err(KernelFault::Corruption),
        );
    }

    // --- managed-index maintenance differential (E05) ---

    const BY_LABEL: [u8; 16] = [0x70; 16];
    const BY_VALUE: [u8; 16] = [0x71; 16];

    /// The `counters` root with a non-unique `byLabel(label, name)` index and a unique
    /// `byValue(value)` index — the maintenance the write path keeps coherent.
    fn indexed_schema() -> StoreSchema {
        let mut schema = schema();
        schema.indexes = vec![
            IndexSchema {
                id: BY_LABEL,
                unique: false,
                projection: vec![IndexComponent::Field(1), IndexComponent::Key(0)],
            },
            IndexSchema {
                id: BY_VALUE,
                unique: true,
                projection: vec![IndexComponent::Field(0)],
            },
        ];
        schema
    }

    /// Every managed-index cell (family `0x02`) of a store, in ascending key order — the
    /// raw index state a maintained write leaves behind.
    fn index_cells<E: ByteEngine>(store: &DurableStore<E>) -> Vec<(Vec<u8>, Vec<u8>)> {
        let view = store.engine.read_view().expect("read view");
        let mut cells = view
            .scan_after(&[0x02], &[0x02])
            .expect("scan index family");
        cells.sort();
        cells
    }

    /// The expected `byLabel` cell for entry `name` with label `label`: keyed by the
    /// projected `[label, name]` tuple, valued by the source key `[name]`.
    fn label_cell(name: &str, label: &str) -> (Vec<u8>, Vec<u8>) {
        (
            physical::index_cell_key(
                "counters",
                &BY_LABEL,
                &[KeyScalar::Str(label.into()), KeyScalar::Str(name.into())],
            ),
            physical::index_cell_value(&[KeyScalar::Str(name.into())]),
        )
    }

    /// The expected unique `byValue` cell for entry `name` with value `value`: keyed by
    /// the projected `[value]`, valued by the source key `[name]`.
    fn value_cell(name: &str, value: i64) -> (Vec<u8>, Vec<u8>) {
        (
            physical::index_cell_key("counters", &BY_VALUE, &[KeyScalar::Int(value)]),
            physical::index_cell_value(&[KeyScalar::Str(name.into())]),
        )
    }

    fn sorted(mut cells: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<(Vec<u8>, Vec<u8>)> {
        cells.sort();
        cells
    }

    /// A fresh redb-backed store over the indexed schema, in a temp dir kept alive by the
    /// returned guard.
    fn native_indexed() -> (DurableStore<NativeEngine>, TempDir) {
        let temp = TempDir::new("index-maint");
        let engine = NativeEngine::open(&temp.store()).expect("open native");
        (
            DurableStore::from_engine(engine, indexed_schema(), sites()),
            temp,
        )
    }

    struct TempDir {
        root: std::path::PathBuf,
    }
    impl TempDir {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&root).expect("create temp dir");
            TempDir { root }
        }
        fn store(&self) -> std::path::PathBuf {
            self.root.join("store")
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn ent(value: i64, label: Option<&str>) -> EntryValue {
        EntryValue {
            fields: vec![
                Some(ValueDomain::Scalar(RuntimeScalar::Int(value))),
                label.map(|l| ValueDomain::Scalar(RuntimeScalar::Str(l.into()))),
            ],
        }
    }

    /// Managed-index read runtime: nonunique progressive-prefix scan and unique
    /// complete-key lookup over the maintained `byLabel`/`byValue` index cells, driven
    /// through the real maintenance write path, plus the forged-image hostiles the
    /// verified image is the sole trust boundary against.
    mod read {
        use super::*;
        use crate::durable::AuthorizedSite;
        use crate::durable::store::{op_index_lookup, op_index_scan, resolve_site};

        fn scan_site() -> AuthorizedSite {
            resolve_site(&indexed_schema(), &SiteTarget::IndexScan(0))
        }

        fn lookup_site() -> AuthorizedSite {
            resolve_site(&indexed_schema(), &SiteTarget::IndexLookup(1))
        }

        /// A store seeded through the real maintenance path: three entries whose `byLabel`
        /// rows share label `"x"` for `a` and `b`, giving distinct labels `{x, y}` and,
        /// under `"x"`, distinct names `{a, b}`.
        fn seeded() -> DurableStore<MemoryEngine> {
            let mut store =
                DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            txn.create_entry(&e, &[ks("b")], ent(2, Some("x"))).unwrap();
            txn.create_entry(&e, &[ks("c")], ent(3, Some("y"))).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
            store
        }

        /// A store whose engine is seeded with raw index cells, bypassing maintenance — the
        /// forged-image shape a hostile reference-valid image can carry.
        fn forged(cells: &[(Vec<u8>, Vec<u8>)]) -> DurableStore<MemoryEngine> {
            let mut engine = MemoryEngine::new();
            let mut txn = engine.begin().unwrap();
            for (key, value) in cells {
                txn.put(key, value.clone()).unwrap();
            }
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
            DurableStore::from_engine(engine, indexed_schema(), sites())
        }

        fn scan(
            store: &DurableStore<MemoryEngine>,
            prefix: &[KeyScalar],
            from: Option<KeyScalar>,
            limit: u32,
        ) -> Result<BoundedKeys, KernelFault> {
            let view = store.engine.read_view().unwrap();
            op_index_scan(&view, &scan_site(), prefix, from, bound(limit))
        }

        fn lookup(
            store: &DurableStore<MemoryEngine>,
            key: &[KeyScalar],
        ) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
            let view = store.engine.read_view().unwrap();
            op_index_lookup(&view, &lookup_site(), key)
        }

        #[test]
        fn scan_yields_distinct_next_component_bounded() {
            let store = seeded();
            // The empty prefix enumerates the first projected component: the distinct
            // labels, in ascending order, with no further value beyond them.
            assert_eq!(
                scan(&store, &[], None, 10),
                Ok(BoundedKeys {
                    keys: vec![ks("x"), ks("y")],
                    more: false,
                })
            );
            // A bound below the population freezes the first `N` and flags the rest.
            assert_eq!(
                scan(&store, &[], None, 1),
                Ok(BoundedKeys {
                    keys: vec![ks("x")],
                    more: true,
                })
            );
        }

        #[test]
        fn scan_refines_under_a_held_prefix_to_the_source_keys() {
            let store = seeded();
            // Holding label `"x"` enumerates its distinct source names — the terminal
            // (complete-projection) component, where each cell equals its component row key.
            assert_eq!(
                scan(&store, &[ks("x")], None, 10),
                Ok(BoundedKeys {
                    keys: vec![ks("a"), ks("b")],
                    more: false,
                })
            );
            // A label with a single row yields exactly that source name.
            assert_eq!(
                scan(&store, &[ks("y")], None, 10),
                Ok(BoundedKeys {
                    keys: vec![ks("c")],
                    more: false,
                })
            );
        }

        #[test]
        fn scan_from_is_an_inclusive_lower_bound_at_both_incomplete_and_complete_levels() {
            let store = seeded();
            // A non-terminal `from` (a label, which is not itself a whole cell): the walk
            // starts at or after it.
            assert_eq!(
                scan(&store, &[], Some(ks("y")), 10),
                Ok(BoundedKeys {
                    keys: vec![ks("y")],
                    more: false,
                })
            );
            // A terminal `from` (a source name whose cell equals its row key exactly): the
            // probe includes the equal row a bare forward scan would exclude.
            assert_eq!(
                scan(&store, &[ks("x")], Some(ks("b")), 10),
                Ok(BoundedKeys {
                    keys: vec![ks("b")],
                    more: false,
                })
            );
            // A `from` strictly above every source name under the prefix yields nothing.
            assert_eq!(
                scan(&store, &[ks("x")], Some(ks("z")), 10),
                Ok(BoundedKeys {
                    keys: vec![],
                    more: false,
                })
            );
        }

        #[test]
        fn lookup_yields_the_one_source_key_or_absent() {
            let store = seeded();
            assert_eq!(lookup(&store, &[ki(2)]), Ok(Some(vec![ks("b")])));
            assert_eq!(lookup(&store, &[ki(1)]), Ok(Some(vec![ks("a")])));
            assert_eq!(lookup(&store, &[ki(99)]), Ok(None));
        }

        #[test]
        fn a_scan_over_a_unique_index_is_rejected() {
            let store = seeded();
            let view = store.engine.read_view().unwrap();
            assert_eq!(
                op_index_scan(&view, &lookup_site(), &[], None, bound(10)),
                Err(KernelFault::Corruption),
            );
        }

        #[test]
        fn a_lookup_over_a_nonunique_index_is_rejected() {
            let store = seeded();
            let view = store.engine.read_view().unwrap();
            assert_eq!(
                op_index_lookup(&view, &scan_site(), &[ks("x"), ks("a")]),
                Err(KernelFault::Corruption),
            );
        }

        #[test]
        fn a_scan_operand_of_the_wrong_kind_is_rejected() {
            let store = seeded();
            // `byLabel`'s first component is the string label; an int prefix is a forged
            // operand.
            assert_eq!(
                scan(&store, &[ki(1)], None, 10),
                Err(KernelFault::Corruption)
            );
        }

        #[test]
        fn a_scan_prefix_covering_the_whole_projection_is_rejected() {
            let store = seeded();
            // No component remains to enumerate: a complete projection is a lookup shape,
            // not a scan.
            assert_eq!(
                scan(&store, &[ks("x"), ks("a")], None, 10),
                Err(KernelFault::Corruption),
            );
        }

        #[test]
        fn a_lookup_of_the_wrong_arity_is_rejected() {
            let store = seeded();
            let view = store.engine.read_view().unwrap();
            assert_eq!(
                op_index_lookup(&view, &lookup_site(), &[ki(1), ki(2)]),
                Err(KernelFault::Corruption),
            );
        }

        #[test]
        fn a_forged_cell_whose_component_decodes_at_the_wrong_kind_is_corruption() {
            // A `byLabel` cell whose first projected column is an int, not the string
            // label the projection declares: a reference-valid image the runtime must not
            // read as a label.
            let store = forged(&[(
                physical::index_cell_key("counters", &BY_LABEL, &[ki(5), ks("a")]),
                physical::index_cell_value(&[ks("a")]),
            )]);
            assert_eq!(scan(&store, &[], None, 10), Err(KernelFault::Corruption));
        }

        #[test]
        fn a_forged_cell_whose_value_is_not_a_source_key_is_corruption() {
            // A unique `byValue` cell whose value does not decode as the root's key tuple
            // (an empty value cannot yield the one expected source key column).
            let store = forged(&[(
                physical::index_cell_key("counters", &BY_VALUE, &[ki(7)]),
                Vec::new(),
            )]);
            assert_eq!(lookup(&store, &[ki(7)]), Err(KernelFault::Corruption));
        }
    }

    /// Creating an indexed entry adds exactly its row to every index whose projection is
    /// fully present: the non-unique `byLabel` and the unique `byValue`.
    #[test]
    fn create_adds_a_row_to_every_index() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            assert_eq!(
                txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap(),
                CreateOutcome::Created,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        assert_eq!(
            index_cells(&store),
            sorted(vec![label_cell("a", "x"), value_cell("a", 1)]),
        );
    }

    /// Changing a projected field moves that index's row and leaves an index the field
    /// does not project untouched. Setting `label` from `x` to `y` moves the `byLabel`
    /// row; `byValue` (over `value`) is unchanged.
    #[test]
    fn changing_a_projected_field_moves_only_its_index_row() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            let label = txn.site(2);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            txn.set_sparse(
                &label,
                &[ks("a")],
                Some(ValueDomain::Scalar(RuntimeScalar::Str("y".into()))),
            )
            .unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        assert_eq!(
            index_cells(&store),
            sorted(vec![label_cell("a", "y"), value_cell("a", 1)]),
        );
    }

    /// Erasing an indexed entry removes exactly its rows and leaves a sibling entry's rows
    /// intact — the index analogue of the descendant-preserving erase.
    #[test]
    fn erasing_one_entry_leaves_a_siblings_rows_intact() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            txn.create_entry(&e, &[ks("b")], ent(2, Some("y"))).unwrap();
            assert_eq!(
                txn.erase_entry(&e, &[ks("a")]).unwrap(),
                EraseOutcome::Erased
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        assert_eq!(
            index_cells(&store),
            sorted(vec![label_cell("b", "y"), value_cell("b", 2)]),
        );
    }

    /// A clear of a projected sparse field removes that index's row (the entry drops out
    /// of `byLabel`) without disturbing an index the field does not project.
    #[test]
    fn clearing_a_projected_field_removes_its_row() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            let label = txn.site(2);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            txn.set_sparse(&label, &[ks("a")], None).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        // Only the unique byValue row survives; byLabel has no row for an absent label.
        assert_eq!(index_cells(&store), sorted(vec![value_cell("a", 1)]));
    }

    /// A replace rewrites the rows to the new projected values, dropping the old.
    #[test]
    fn replacing_an_entry_rewrites_its_rows() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            assert_eq!(
                txn.replace_entry(&e, &[ks("a")], ent(9, Some("z")))
                    .unwrap(),
                ReplaceOutcome::Replaced,
            );
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        assert_eq!(
            index_cells(&store),
            sorted(vec![label_cell("a", "z"), value_cell("a", 9)]),
        );
    }

    /// A second entry colliding on a unique index faults `UniqueIndexViolation`, and the
    /// transaction rolls back without poisoning: the committed first entry survives and a
    /// fresh transaction still works.
    #[test]
    fn a_unique_collision_faults_and_rolls_back_without_poisoning() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            // "b" collides with "a" on the unique byValue index (both value 1).
            assert_eq!(
                txn.create_entry(&e, &[ks("b")], ent(1, Some("y"))),
                Err(KernelFault::UniqueIndexViolation),
            );
            // The transaction is dropped without commit: a rollback.
        }
        // Only "a"'s rows remain; "b" never landed.
        assert_eq!(
            index_cells(&store),
            sorted(vec![label_cell("a", "x"), value_cell("a", 1)]),
        );
        // The store is not poisoned: a fresh transaction commits.
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            txn.create_entry(&e, &[ks("c")], ent(2, Some("z"))).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        assert_eq!(
            index_cells(&store),
            sorted(vec![
                label_cell("a", "x"),
                label_cell("c", "z"),
                value_cell("a", 1),
                value_cell("c", 2),
            ]),
        );
    }

    /// Setting a projected field that was absent adds the index row without removing a
    /// non-existent old row (the missing-old case): an entry created without a `label` has
    /// no `byLabel` row until the field is set.
    #[test]
    fn setting_an_absent_projected_field_adds_a_row() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            let label = txn.site(2);
            // Created with no label: only the unique byValue row exists.
            txn.create_entry(&e, &[ks("a")], ent(1, None)).unwrap();
            txn.set_sparse(
                &label,
                &[ks("a")],
                Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into()))),
            )
            .unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        assert_eq!(
            index_cells(&store),
            sorted(vec![label_cell("a", "x"), value_cell("a", 1)]),
        );
    }

    /// An index row staged for an entry that fails commit rolls back with the entry: setting
    /// only the projected sparse `label` of an entry whose required `value` is unset stages a
    /// `byLabel` row, but the commit reconcile faults `RequiredMissing` and the whole
    /// transaction — index row included — rolls back, leaving no index cell behind.
    #[test]
    fn a_required_missing_rollback_leaves_no_index_row() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        let result = {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let label = txn.site(2);
            // Set the projected sparse label without ever setting the required value.
            txn.set_sparse(
                &label,
                &[ks("a")],
                Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into()))),
            )
            .unwrap();
            txn.commit()
        };
        assert!(
            matches!(result, CommitResult::RequiredMissing { field, .. } if field == "value"),
            "the commit rolls back on the unset required field",
        );
        assert!(
            index_cells(&store).is_empty(),
            "the staged byLabel row rolled back with the transaction",
        );
    }

    /// A projected leaf that will not decode is corruption: maintenance reading the old
    /// projected state over a tampered store faults `Corruption` rather than trusting an
    /// undecodable value into an index key.
    #[test]
    fn a_corrupt_projected_leaf_faults_corruption() {
        let mut store = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        // Tamper the `label` leaf of entry "a" with bytes no value decodes.
        let marker = physical::marker_key("counters", &[ks("a")]);
        let leaf = physical::stem_field_leaf(&marker, "label");
        {
            let mut txn = store.engine.begin().expect("begin");
            // Bytes no value codec decodes (decimal to avoid spelling a structural tag
            // literal the layout-owner gate reserves for physical.rs).
            txn.put(&leaf, vec![255, 255, 255]).expect("put garbage");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        // A field write that maintains byLabel must read the corrupt old value and fault.
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let label = txn.site(2);
        assert_eq!(
            txn.set_sparse(
                &label,
                &[ks("a")],
                Some(ValueDomain::Scalar(RuntimeScalar::Str("y".into()))),
            ),
            Err(KernelFault::Corruption),
        );
    }

    /// The same index cells result over the in-memory and redb engines: maintenance is
    /// kernel logic above the byte engine, so the two backends agree cell for cell.
    #[test]
    fn index_maintenance_agrees_across_engines() {
        fn replay<E: ByteEngine>(store: &mut DurableStore<E>) {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write_demand())
                .unwrap();
            let e = txn.site(0);
            let label = txn.site(2);
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
            txn.create_entry(&e, &[ks("b")], ent(2, Some("y"))).unwrap();
            txn.set_sparse(
                &label,
                &[ks("a")],
                Some(ValueDomain::Scalar(RuntimeScalar::Str("z".into()))),
            )
            .unwrap();
            txn.erase_entry(&e, &[ks("b")]).unwrap();
            assert_eq!(txn.commit(), CommitResult::Committed);
        }
        let mut mem = DurableStore::from_engine(MemoryEngine::new(), indexed_schema(), sites());
        replay(&mut mem);
        let (mut native, _temp) = native_indexed();
        replay(&mut native);
        assert_eq!(
            index_cells(&mem),
            index_cells(&native),
            "the two engines disagree on maintained index cells",
        );
        assert_eq!(
            index_cells(&mem),
            sorted(vec![label_cell("a", "z"), value_cell("a", 1)])
        );
    }
}
