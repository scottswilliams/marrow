//! The durable store handle and its read/transaction sessions (design §G).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::{ByteEngine, CommitOutcome, ReadView, StoreError, WriteTxn};

use super::physical::{self, CellKind};
use super::plan::{CellWrite, IndexOp, Planner};
use super::profile;
use super::{
    AuthTarget, AuthorizedSite, BoundedKeys, BoundedLimit, BranchHop, BranchSchema, CommitResult,
    CreateOutcome, DemandCoverage, Denied, EntryValue, EraseOutcome, FieldSchema, GroupSchema,
    IndexComponent, IndexSchema, InvocationGrant, KernelFault, NextKey, Presence, Reopen,
    ReplaceOutcome, SessionError, SiteSpec, SiteTarget, StoreSchema,
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
fn resolve_site(schema: &StoreSchema, root_index: u16, target: &SiteTarget) -> AuthorizedSite {
    // A managed-index read addresses no source node: it resolves to the index's cell
    // family identity, its read kind, and the scalar kind of each projected component
    // (root key columns and top-level fields, by position), and carries an empty branch
    // path since every index is root-level. The index position is local to this root's
    // schema (the image-wide position was rebased per root when the site table was built).
    if let SiteTarget::IndexScan(position) | SiteTarget::IndexLookup(position) = target {
        let index = &schema.indexes[*position as usize];
        let projection = index
            .projection
            .iter()
            .map(|component| index_component_kind(schema, *component))
            .collect();
        return AuthorizedSite::index(
            schema.root_name.clone(),
            root_index,
            schema.key.clone(),
            AuthTarget::index(index.id, index.unique, projection),
        );
    }
    // A root-level group addresses the root entry (empty branch path); it carries the
    // group's own record. Group-in-branch is durable-only future work, so a group site is
    // root-level at T01.
    if let SiteTarget::GroupEntry(position) = target {
        let group = &schema.groups[*position as usize];
        return AuthorizedSite::new(
            schema.root_name.clone(),
            root_index,
            schema.key.clone(),
            Vec::new(),
            AuthTarget::Group {
                name: group.name.clone(),
                fields: group.fields.clone(),
            },
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
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) | SiteTarget::GroupEntry(_) => {
            unreachable!("index and group targets resolved above")
        }
    };
    // A whole-entry site enumerates the container's footprint, so it carries the
    // container's record and its groups; a field-target site carries its field plus the
    // container record so a staged set can reconcile the node at commit. A root entry's
    // footprint includes its groups (its own payload); a branch entry carries none, since
    // group-in-branch is not yet executable.
    let target = match target {
        SiteTarget::WholePayload => AuthTarget::Entry {
            fields: container_fields.to_vec(),
            groups: schema.groups.clone(),
        },
        SiteTarget::BranchEntry(_) => AuthTarget::Entry {
            fields: container_fields.to_vec(),
            groups: Vec::new(),
        },
        SiteTarget::FieldLeaf(index) | SiteTarget::BranchField { field: index, .. } => {
            AuthTarget::field(&container_fields[*index as usize], container_fields)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) | SiteTarget::GroupEntry(_) => {
            unreachable!("index and group targets resolved above")
        }
    };
    AuthorizedSite::new(
        schema.root_name.clone(),
        root_index,
        schema.key.clone(),
        branch,
        target,
    )
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
    fn read_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        op_read_group(&self.view, site, keys, false)
    }
    fn replace_group(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn erase_group(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
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
    /// Each root's managed indexes, in stable declaration order, indexed by the root's
    /// declaration position (aligned to the store's schema table). A root-level write to
    /// root R keeps `indexes[R]` coherent as a consequence of the source write; a root
    /// with no index carries an empty list and skips maintenance entirely.
    indexes: Vec<Vec<IndexSchema>>,
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
    fn read_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        op_read_group(self.txn(), site, keys, true)
    }
    fn replace_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        let stem = node_stem(site, keys)?;
        let (name, fields) = group_target(site);
        // A group has no independent existence: replacing a group of a payload-absent
        // entry is Missing and touches nothing (symmetric with a whole-entry replace over
        // a markerless node).
        if read_raw(self.txn(), &stem)?.is_none() {
            return Ok(ReplaceOutcome::Missing);
        }
        let group_stem = physical::group_stem(&stem, name);
        let planner = Planner::new();
        // Exact replacement scoped to the group's own leaves through the group-parametric
        // planner: remove them all, then write the present ones. The entry marker, the
        // entry's top-level fields, its sibling groups, and its branches are outside the
        // group prefix and untouched. A group leaf is not index-projected, so no managed
        // index maintenance runs.
        let mut ops = planner.group_erase(&group_stem, fields);
        ops.extend(planner.group_write(&group_stem, fields, &value)?);
        self.apply(ops)?;
        Ok(ReplaceOutcome::Replaced)
    }
    fn erase_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        let stem = node_stem(site, keys)?;
        let (name, fields) = group_target(site);
        let group_stem = physical::group_stem(&stem, name);
        let planner = Planner::new();
        // A group carries no marker, so erasing it removes only its own field leaves. It
        // existed if any leaf was present; the removal is by exact key, so the entry
        // marker, top-level fields, sibling groups, and branches are preserved.
        let mut existed = false;
        for cell in planner.group_cells(&group_stem, fields) {
            if read_raw(self.txn(), &cell)?.is_some() {
                existed = true;
            }
        }
        self.apply(planner.group_erase(&group_stem, fields))?;
        Ok(if existed {
            EraseOutcome::Erased
        } else {
            EraseOutcome::Missing
        })
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
        let (fields, groups) = node_shape(site);
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
                        &Self::projected_positions_of(self.indexes_of(site)),
                    )?
                } else {
                    Vec::new()
                };
                let ops = planner.node_write(&stem, fields, groups, &entry)?;
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
        let (fields, groups) = node_shape(site);
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
            self.read_projected(
                &stem,
                fields,
                &Self::projected_positions_of(self.indexes_of(site)),
            )?
        } else {
            Vec::new()
        };
        // Exact replacement through the one node-parametric planner: remove the node's
        // own cells, then write the new payload, so unlisted sparse leaves do not
        // survive and keyed branch descendants are left intact.
        let mut ops = planner.node_erase(&stem, fields, groups);
        ops.extend(planner.node_write(&stem, fields, groups, &entry)?);
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
        let (fields, groups) = node_shape(site);
        let planner = Planner::new();
        let existed = read_raw(self.txn(), &stem)?.is_some();
        let maintains = self.maintains_root(site);
        let old = if maintains {
            self.read_projected(
                &stem,
                fields,
                &Self::projected_positions_of(self.indexes_of(site)),
            )?
        } else {
            Vec::new()
        };
        // Whole-node removal through the node-parametric planner: marker, every own field
        // leaf, and every group leaf, by exact key — a branch tag is never enumerated, so a
        // node's keyed descendants survive an erase of its payload while its groups (its own
        // payload) are swept.
        let ops = planner.node_erase(&stem, fields, groups);
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

    /// The managed indexes of the root the `site` addresses, by its declaration position.
    /// Index maintenance reads and moves only this root's index cells, so a cross-root
    /// transaction never confuses one root's indexes with another's.
    fn indexes_of(&self, site: &AuthorizedSite) -> &[IndexSchema] {
        &self.indexes[site.root_index() as usize]
    }

    /// Whether root-level managed-index maintenance applies to a write on `site`: the
    /// site's root declares indexes and the write addresses a root entry. A branch entry
    /// carries no index (indexes project a root's own keys and top-level fields), so a
    /// branch write never maintains one.
    fn maintains_root(&self, site: &AuthorizedSite) -> bool {
        !self.indexes_of(site).is_empty() && site.branch.is_empty()
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

    /// The managed indexes of the `site`'s root that project the root field at
    /// `position` — the exact indexes a write to that field must maintain, and the only
    /// ones it reads sibling leaves for.
    fn indexes_projecting(&self, site: &AuthorizedSite, position: usize) -> Vec<IndexSchema> {
        self.indexes_of(site)
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
        let indexes = self.indexes_projecting(site, position);
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
        let ops = Planner::new().index_writes(&site.root, self.indexes_of(site), keys, old, new)?;
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
        AuthTarget::Entry { fields, .. } => fields,
        AuthTarget::Field { record, .. } => record,
        AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a node op targets a node site")
        }
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
        AuthTarget::Entry { .. } | AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
            unreachable!("verifier proved a field-target site")
        }
    }
}

/// The addressed node's own record fields and groups for a whole-entry op — the whole
/// payload footprint the consequence planner enumerates. The verifier proves a whole-entry
/// opcode targets an entry site, so a field target here is unreachable. A branch node
/// carries no group, so its group slice is empty.
fn node_shape(site: &AuthorizedSite) -> (&[FieldSchema], &[GroupSchema]) {
    match &site.target {
        AuthTarget::Entry { fields, groups } => (fields, groups),
        AuthTarget::Field { .. } | AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
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
            // An own field leaf or a group leaf (both the node's own payload) below a
            // markerless stem is a marker/payload mismatch — the orphan case.
            physical::BelowMarker::OwnField | physical::BelowMarker::OwnGroup => SlotClass::Orphan,
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
        AuthTarget::Entry { .. } => stem,
        AuthTarget::Field { name, .. } => physical::stem_field_leaf(&stem, name),
        AuthTarget::Index { .. } | AuthTarget::Group { .. } => {
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
    let (fields, groups) = node_shape(site);
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
    let values = read_record_leaves(cells, &stem, fields)?;
    // A present entry materializes each of its groups (its own payload) under the group
    // prefix, in schema order — a group's presence is the entry's, so a present entry
    // always yields every group sub-record. A present entry missing a required group leaf
    // is the same marker/payload mismatch as a missing required top-level field.
    let mut group_values = Vec::with_capacity(groups.len());
    for group in groups {
        let group_stem = physical::group_stem(&stem, &group.name);
        group_values.push(EntryValue {
            fields: read_record_leaves(cells, &group_stem, &group.fields)?,
            groups: Vec::new(),
        });
    }
    Ok(Some(EntryValue {
        fields: values,
        groups: group_values,
    }))
}

/// Read the ordered field-leaf values of the record whose leaves namespace under `stem`
/// (an entry marker stem or a group stem): one slot per field, decoded to its shape. A
/// present required leaf is mandatory — its absence under a present container is a
/// marker/payload mismatch ([`KernelFault::Corruption`]) — while an absent sparse leaf
/// reads vacant. The shared owner of top-level-field and group-leaf materialization, so
/// both decode and enforce required-completeness identically.
///
/// Engine work is proportional to the *present* leaf count, not the declared field
/// width: a structural-tag-bounded range scan over the node's own contiguous field-leaf
/// cells ([`physical::field_leaf_range`]) visits only present leaves (`O(populated + 1)`
/// reads), where a per-declared-field probe would read a vacant cell per absent field. A
/// declared-field-to-position map resolves each scanned leaf in constant time and carries
/// the required-completeness check; building it touches the declared fields but performs
/// no engine work.
fn read_record_leaves<V: ReadView>(
    cells: &V,
    stem: &[u8],
    fields: &[FieldSchema],
) -> Result<Vec<Option<ValueDomain>>, KernelFault> {
    let mut position: HashMap<Vec<u8>, usize> = HashMap::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        position.insert(physical::stem_field_leaf(stem, &field.name), index);
    }
    let mut values: Vec<Option<ValueDomain>> = (0..fields.len()).map(|_| None).collect();

    let range = physical::field_leaf_range(stem);
    let mut cursor = range.clone();
    loop {
        let page = cells
            .scan_after(&range, &cursor)
            .map_err(KernelFault::Engine)?;
        let Some(last_key) = page.last().map(|(key, _)| key.clone()) else {
            break;
        };
        for (key, bytes) in &page {
            // A present own-field leaf under this node that resolves to no declared
            // field is a forged or orphaned cell: fail closed as corruption rather than
            // silently dropping it. Evolution constraint: a field drop or rename must
            // migrate or delete the retired field's stored leaves before a whole-entry
            // read; a stale leaf under a shrunk schema is indistinguishable from
            // corruption at this fail-closed check.
            let index = *position.get(key).ok_or(KernelFault::Corruption)?;
            values[index] =
                Some(decode_domain(bytes, &fields[index].shape).ok_or(KernelFault::Corruption)?);
        }
        cursor = last_key;
    }

    // A present container missing a required field leaf is the same marker/payload
    // mismatch a missing required top-level field is.
    for (index, field) in fields.iter().enumerate() {
        if field.required && values[index].is_none() {
            return Err(KernelFault::Corruption);
        }
    }
    Ok(values)
}

/// The group name and its own record fields a group site addresses. The verifier proves
/// a whole-group op targets a group site, so any other target here is a forged image.
fn group_target(site: &AuthorizedSite) -> (&str, &[FieldSchema]) {
    match &site.target {
        AuthTarget::Group { name, fields } => (name, fields),
        AuthTarget::Entry { .. } | AuthTarget::Field { .. } | AuthTarget::Index { .. } => {
            unreachable!("verifier proved a whole-group op targets a group site")
        }
    }
}

/// Materialize one group's record from the entry `keys` addresses: one slot per group
/// field, present or vacant. A group's presence is its containing entry's presence, so
/// this probes the entry marker exactly as [`op_read_entry`] does — a markerless slot is
/// payload-absent (or, for a persisted own-payload leaf with no marker, corruption in a
/// committed read and pending inside a transaction). A present entry then reads the
/// group's own leaves under the group prefix; a present entry missing a `required` group
/// leaf is a marker/payload mismatch (corruption), and an absent sparse leaf reads vacant.
fn op_read_group<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    keys: &[KeyScalar],
    tolerate_pending: bool,
) -> Result<Option<EntryValue>, KernelFault> {
    let stem = node_stem(site, keys)?;
    let (name, fields) = group_target(site);
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
    let group_stem = physical::group_stem(&stem, name);
    Ok(Some(EntryValue {
        fields: read_record_leaves(cells, &group_stem, fields)?,
        groups: Vec::new(),
    }))
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
mod tests;
