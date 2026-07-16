//! The durable store handle and its read/transaction sessions (design §G).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::{ByteEngine, CommitOutcome, ReadView, StoreError, WriteTxn};

use super::physical::{self, CellKind};
use super::plan::{CellWrite, Planner};
use super::profile;
use super::{
    AuthTarget, AuthorizedSite, BoundedKeys, BoundedLimit, BranchHop, CommitResult, CreateOutcome,
    DemandCoverage, Denied, EntryValue, EraseOutcome, FieldSchema, InvocationGrant, KernelFault,
    NextKey, Presence, Reopen, ReplaceOutcome, SessionError, SiteSpec, SiteTarget, StoreSchema,
};
use crate::codec::key::{KeyScalar, decode_key_value, encode_key_value};
use crate::codec::value::{RuntimeScalar, decode_value, encode_value};

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
    ) -> Result<Option<RuntimeScalar>, KernelFault>;
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault>;
    fn next_key(
        &mut self,
        site: &AuthorizedSite,
        after: Option<KeyScalar>,
    ) -> Result<NextKey, KernelFault>;
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
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: RuntimeScalar,
    ) -> Result<(), KernelFault>;
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault>;
    /// Set (present) or clear (vacant) a sparse field of an entry the caller has
    /// statically proven present. Asserts the entry marker is present — a violation
    /// is a marker/field mismatch ([`KernelFault::Corruption`]), never implicit
    /// creation — then stages the leaf exactly like [`Self::set_sparse`].
    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<RuntimeScalar>,
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
            .map(|site| resolve_site(&self.schema, site.target))
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
        // Split the store's fields into disjoint borrows: the transaction borrows
        // the engine mutably while the session still reads the schema and writes
        // the poison flag.
        let Self {
            engine,
            schema,
            poisoned,
            ..
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
            schema,
            poisoned,
            auth,
            token: mint_token(),
            pending: BTreeSet::new(),
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
/// resolves the branch from the schema so the addressed node carries its own key kind
/// and record.
fn resolve_site(schema: &StoreSchema, target: SiteTarget) -> AuthorizedSite {
    // The container node the site addresses: its branch path and own record fields.
    // A root target's container is the root; a branch target's is the branch node.
    let (branch, container_fields): (Vec<BranchHop>, &[FieldSchema]) = match target {
        SiteTarget::WholePayload | SiteTarget::FieldLeaf(_) => (Vec::new(), &schema.fields),
        SiteTarget::BranchEntry(branch) => {
            let branch = &schema.branches[branch as usize];
            (
                vec![BranchHop::new(branch.name.clone(), branch.key)],
                &branch.fields,
            )
        }
    };
    // A whole-entry site enumerates the container's footprint, so it carries the
    // container's record; a field-target site touches one leaf and carries no fields.
    let target = match target {
        SiteTarget::WholePayload | SiteTarget::BranchEntry(_) => {
            AuthTarget::Entry(container_fields.to_vec())
        }
        SiteTarget::FieldLeaf(index) => AuthTarget::field(&container_fields[index as usize]),
    };
    AuthorizedSite::new(schema.root_name.clone(), schema.key, branch, target)
}

/// The physical marker stem of the node `site` addresses at key-path `keys`: the root
/// marker followed by one branch-child stem per branch hop. The single owner of
/// key-path-to-node-stem resolution, so a root and a branch node derive their stem the
/// same way. The key-path arity and each element's scalar kind are asserted against
/// the site's declared root and hop kinds as defense in depth over the verifier's
/// proof.
fn node_stem(site: &AuthorizedSite, keys: &[KeyScalar]) -> Vec<u8> {
    debug_assert_eq!(
        keys.len(),
        1 + site.branch.len(),
        "the key-path arity matches the site's root plus branch hops",
    );
    debug_assert_eq!(
        keys[0].scalar_kind(),
        site.key,
        "the root key kind matches the site",
    );
    let mut stem = physical::marker_key(&site.root, &keys[0]);
    for (hop, key) in site.branch.iter().zip(&keys[1..]) {
        debug_assert_eq!(
            key.scalar_kind(),
            hop.key,
            "the branch key kind matches the hop",
        );
        stem = physical::branch_child_stem(&stem, &hop.name, key);
    }
    stem
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
    ) -> Result<Option<RuntimeScalar>, KernelFault> {
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
    fn next_key(
        &mut self,
        site: &AuthorizedSite,
        after: Option<KeyScalar>,
    ) -> Result<NextKey, KernelFault> {
        op_next_key(&self.view, site, after)
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
    fn set_required(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: RuntimeScalar,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn set_sparse(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn set_sparse_present(
        &mut self,
        _site: &AuthorizedSite,
        _keys: &[KeyScalar],
        _value: Option<RuntimeScalar>,
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
    schema: &'s StoreSchema,
    /// The store's poison flag, set on an indeterminate commit so a reopen
    /// reclassifies.
    poisoned: &'s mut bool,
    auth: Vec<AuthorizedSite>,
    token: [u8; 16],
    /// Keys whose fields were staged; reconciled at commit to decide created vs
    /// required-missing.
    pending: BTreeSet<Vec<u8>>,
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

    /// Validate every staged entry: a live entry missing a required field is a
    /// `RequiredMissing`; a live markerless entry with all required fields present
    /// gets its marker (created at commit). The staged set holds root-level entry
    /// keys, so this reconciles root nodes; a branch node reached only by whole-entry
    /// create/replace/erase never stages a field and needs no reconcile. Reconciling a
    /// staged branch field (the field-exact branch tail) extends this to the branch
    /// node's marker and record.
    fn reconcile(&mut self) -> Result<(), CommitResult> {
        let root = self.schema.root_name.clone();
        let planner = Planner::new(&root);
        let staged: Vec<KeyScalar> = self
            .pending
            .iter()
            .map(|bytes| {
                decode_key_value(bytes)
                    .expect("a staged key was our own encoding")
                    .0
            })
            .collect();
        for key in staged {
            let marker_key = planner.marker(&key);
            let marker_present = read_raw(self.txn(), &marker_key)
                .map_err(|_| CommitResult::CommitFault)?
                .is_some();
            let mut any_leaf = false;
            let mut missing_required: Option<String> = None;
            for field in &self.schema.fields {
                let leaf = planner.field_leaf(&key, &field.name);
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
                return Err(CommitResult::RequiredMissing { key, field });
            }
            if !marker_present {
                self.txn_mut()
                    .put(&marker_key, physical::MARKER_VALUE.to_vec())
                    .map_err(|_| CommitResult::CommitFault)?;
            }
        }
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
    ) -> Result<Option<RuntimeScalar>, KernelFault> {
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
    fn next_key(
        &mut self,
        site: &AuthorizedSite,
        after: Option<KeyScalar>,
    ) -> Result<NextKey, KernelFault> {
        op_next_key(self.txn(), site, after)
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
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: RuntimeScalar,
    ) -> Result<(), KernelFault> {
        let leaf = physical::stem_field_leaf(&node_stem(site, keys), field_name(site, true));
        let bytes = encode_value(&value).map_err(|_| KernelFault::ValueRange)?;
        self.txn_mut()
            .put(&leaf, bytes)
            .map_err(KernelFault::Engine)?;
        self.pending.insert(encode_key_value(&keys[0]));
        Ok(())
    }
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        let leaf = physical::stem_field_leaf(&node_stem(site, keys), field_name(site, false));
        match value {
            Some(value) => {
                let bytes = encode_value(&value).map_err(|_| KernelFault::ValueRange)?;
                self.txn_mut()
                    .put(&leaf, bytes)
                    .map_err(KernelFault::Engine)?;
                self.pending.insert(encode_key_value(&keys[0]));
            }
            None => {
                self.txn_mut().remove(&leaf).map_err(KernelFault::Engine)?;
            }
        }
        Ok(())
    }
    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        // The compiler's place-slot presence proof makes an absent marker
        // unreachable; assert it here as defense in depth over the trust boundary.
        // A present field leaf without a present entry marker is corruption, never
        // implicit creation (the marker law).
        let marker = node_stem(site, keys);
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
        let stem = node_stem(site, keys);
        let fields = node_fields(site);
        let planner = Planner::new(&site.root);
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
                let ops = planner.node_write(&stem, fields, &entry)?;
                self.apply(ops)?;
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
        let stem = node_stem(site, keys);
        let fields = node_fields(site);
        let planner = Planner::new(&site.root);
        // A markerless node (absent or descendant-only) has no payload to replace, so
        // it reports Missing without touching any descendants (the compiler lowers a
        // whole assignment as exists?→replace:create, so this is the defense-in-depth
        // arm the create path complements).
        if read_raw(self.txn(), &stem)?.is_none() {
            return Ok(ReplaceOutcome::Missing);
        }
        // Exact replacement through the one node-parametric planner: remove the node's
        // own cells, then write the new payload, so unlisted sparse leaves do not
        // survive and keyed branch descendants are left intact.
        let mut ops = planner.node_erase(&stem, fields);
        ops.extend(planner.node_write(&stem, fields, &entry)?);
        self.apply(ops)?;
        Ok(ReplaceOutcome::Replaced)
    }
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        let leaf = physical::stem_field_leaf(&node_stem(site, keys), field_name(site, false));
        let existed = read_raw(self.txn(), &leaf)?.is_some();
        self.txn_mut().remove(&leaf).map_err(KernelFault::Engine)?;
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
        let stem = node_stem(site, keys);
        let fields = node_fields(site);
        let planner = Planner::new(&site.root);
        let existed = read_raw(self.txn(), &stem)?.is_some();
        // Whole-node removal through the node-parametric planner: marker plus every own
        // field leaf, by exact key — a branch tag is never enumerated, so a node's
        // keyed descendants survive an erase of its payload.
        let ops = planner.node_erase(&stem, fields);
        self.apply(ops)?;
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
        AuthTarget::Entry(_) => unreachable!("verifier proved a field-target site"),
    }
}

/// The addressed node's own record fields for a whole-entry op. The verifier proves a
/// whole-entry opcode targets an entry site, so a field target here is unreachable.
fn node_fields(site: &AuthorizedSite) -> &[FieldSchema] {
    match &site.target {
        AuthTarget::Entry(fields) => fields,
        AuthTarget::Field { .. } => {
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
    let stem = node_stem(site, keys);
    let physical_key = match &site.target {
        AuthTarget::Entry(_) => stem,
        AuthTarget::Field { name, .. } => physical::stem_field_leaf(&stem, name),
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
) -> Result<Option<RuntimeScalar>, KernelFault> {
    let AuthTarget::Field { name, kind, .. } = &site.target else {
        unreachable!("verifier proved a field read targets a field site")
    };
    let leaf = physical::stem_field_leaf(&node_stem(site, keys), name);
    match read_raw(cells, &leaf)? {
        None => Ok(None),
        Some(bytes) => decode_value(&bytes, *kind)
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
    let stem = node_stem(site, keys);
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
                    decode_value(&bytes, field.kind).ok_or(KernelFault::Corruption)?,
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
/// forward marker walk: both the root `next_key` op and the bounded layer acquisition
/// step through it, so the root and branch layers walk identically.
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

fn op_next_key<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    after: Option<KeyScalar>,
) -> Result<NextKey, KernelFault> {
    let seek = match after {
        None => LayerSeek::Start,
        Some(key) => LayerSeek::After(key),
    };
    layer_step(cells, &physical::Layer::root(&site.root), seek)
}

/// The durable layer the whole-entry `site` traverses, resolving its parent entry from
/// `ancestor_keys`: a root (`WholePayload`) site traverses the root's entry family with
/// no ancestor key; a branch site traverses its branch family beneath the root entry
/// the ancestor key-path names. The single owner of the site-to-traversed-layer
/// mapping. The ancestor arity and each key's scalar kind are asserted against the
/// site's declared root and hop kinds as defense in depth over the verifier's proof.
fn layer_of(site: &AuthorizedSite, ancestor_keys: &[KeyScalar]) -> physical::Layer {
    match site.branch.split_last() {
        None => {
            debug_assert!(
                ancestor_keys.is_empty(),
                "the root layer has no ancestor key"
            );
            physical::Layer::root(&site.root)
        }
        Some((traversed, parent_hops)) => {
            debug_assert_eq!(
                ancestor_keys.len(),
                1 + parent_hops.len(),
                "a branch layer's ancestor key-path locates its parent entry",
            );
            debug_assert_eq!(
                ancestor_keys[0].scalar_kind(),
                site.key,
                "the root key kind matches the site",
            );
            let mut stem = physical::marker_key(&site.root, &ancestor_keys[0]);
            for (hop, key) in parent_hops.iter().zip(&ancestor_keys[1..]) {
                debug_assert_eq!(
                    key.scalar_kind(),
                    hop.key,
                    "the branch key kind matches the hop"
                );
                stem = physical::branch_child_stem(&stem, &hop.name, key);
            }
            physical::Layer::branch(&stem, &traversed.name)
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
    let layer = layer_of(site, ancestor_keys);
    let mut keys: Vec<KeyScalar> = Vec::with_capacity(limit.get());
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
    use marrow_store::{ByteEngine, CommitOutcome, MemoryEngine, WriteTxn};

    use super::super::physical;
    use super::super::{
        BoundedKeys, BoundedLimit, BranchSchema, CommitResult, CreateOutcome, DemandCoverage,
        EntryValue, FieldSchema, InvocationGrant, KernelFault, NextKey, Presence, ReplaceOutcome,
        SessionError, SiteSpec, SiteTarget, StoreSchema,
    };
    use super::{Durable, DurableStore};
    use crate::codec::key::KeyScalar;
    use crate::codec::value::{RuntimeScalar, ScalarKind};

    fn schema() -> StoreSchema {
        StoreSchema {
            root_name: "counters".into(),
            key: ScalarKind::Str,
            fields: vec![
                FieldSchema {
                    name: "value".into(),
                    kind: ScalarKind::Int,
                    required: true,
                },
                FieldSchema {
                    name: "label".into(),
                    kind: ScalarKind::Str,
                    required: false,
                },
            ],
            branches: Vec::new(),
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

    fn value_entry(v: i64) -> EntryValue {
        EntryValue {
            fields: vec![Some(RuntimeScalar::Int(v)), None],
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
        let mut keys = Vec::new();
        let mut cursor = None;
        while let NextKey::Next(key) = read.next_key(&entry, cursor.clone()).expect("next") {
            keys.push(key.clone());
            cursor = Some(key);
        }
        assert_eq!(
            keys,
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
                    &physical::marker_key("counters", &KeyScalar::Str("x".into())),
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
        assert_eq!(read.next_key(&entry, None), Err(KernelFault::Corruption));
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
            Some(RuntimeScalar::Str("hi".into())),
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
                    &physical::marker_key("counters", &KeyScalar::Str("x".into())),
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
            Some(RuntimeScalar::Str("hi".into())),
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
            key: ScalarKind::Str,
            fields: vec![FieldSchema {
                name: "title".into(),
                kind: ScalarKind::Str,
                required: true,
            }],
            branches: vec![BranchSchema {
                name: "notes".into(),
                key: ScalarKind::Int,
                fields: vec![FieldSchema {
                    name: "text".into(),
                    kind: ScalarKind::Str,
                    required: true,
                }],
            }],
        };
        let sites = vec![
            SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SiteSpec {
                target: SiteTarget::BranchEntry(0),
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
                fields: vec![Some(RuntimeScalar::Str("hi".into()))],
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
                fields: vec![Some(RuntimeScalar::Str("late".into()))],
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
                fields: vec![Some(RuntimeScalar::Str("Book A".into()))],
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
                    fields: vec![Some(RuntimeScalar::Str("Book A".into()))],
                })),
                "the root create gave the descendant-only node a payload",
            );
            let branch = read.site(1);
            assert_eq!(
                read.read_entry(&branch, &note),
                Ok(Some(EntryValue {
                    fields: vec![Some(RuntimeScalar::Str("hi".into()))],
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
        physical::marker_key("books", &KeyScalar::Str(key.into()))
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
        let branch_stem = physical::branch_child_stem(&stem, "notes", &KeyScalar::Int(7));
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
                fields: vec![Some(RuntimeScalar::Str("hi".into()))],
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
        let branch_stem = physical::branch_child_stem(&stem, "notes", &KeyScalar::Int(7));
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
        let branch_stem = physical::branch_child_stem(&stem, "notes", &KeyScalar::Int(7));
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

    /// The descendant-skip law of forward iteration: `next_key` visits only
    /// payload-bearing (marker-present) entries, seeking a descendant-only entry's
    /// whole subtree in one cursor step. Present entries `k1` and `k4` bracket two
    /// descendant-only entries `k2` and `k3` — each a markerless root carrying only a
    /// keyed branch child — injected directly so the ops cannot construct the state.
    /// Iteration from `k1` jumps to `k4`, skipping both; resuming from inside the
    /// descendant-only run lands on `k4` too; and iteration terminates after `k4`.
    #[test]
    fn next_key_skips_a_run_of_descendant_only_entries_between_present_siblings() {
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
                &KeyScalar::Int(7),
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

        // From the start, the first present entry; then straight to `k4`, skipping the
        // two descendant-only entries in one run; then End.
        assert_eq!(read.next_key(&root, None), Ok(NextKey::Next(k("k1"))));
        assert_eq!(
            read.next_key(&root, Some(k("k1"))),
            Ok(NextKey::Next(k("k4"))),
            "next_key skips both descendant-only entries between the present siblings",
        );
        assert_eq!(
            read.next_key(&root, Some(k("k4"))),
            Ok(NextKey::End),
            "iteration terminates after the last present entry",
        );

        // Resuming from inside the descendant-only run — after `k2`, the first of the
        // two, or after `k3`, the second — resolves to `k4` just as resuming from the
        // present sibling before them does: the skip does not depend on the cursor
        // starting at a present entry.
        assert_eq!(
            read.next_key(&root, Some(k("k2"))),
            Ok(NextKey::Next(k("k4"))),
            "resuming inside the descendant-only run still yields the next present entry",
        );
        assert_eq!(
            read.next_key(&root, Some(k("k3"))),
            Ok(NextKey::Next(k("k4")))
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
                &KeyScalar::Int(7),
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
                    fields: vec![Some(RuntimeScalar::Str("T".into()))],
                };
                txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                    .expect("root create");
            }
            // A large branch fan-out under book "a" the root walk must skip wholesale.
            for note in 0..200i64 {
                let text = EntryValue {
                    fields: vec![Some(RuntimeScalar::Str("n".into()))],
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
                    fields: vec![Some(RuntimeScalar::Str("T".into()))],
                };
                txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                    .expect("root create");
            }
            // Notes 10,20,30 under "a"; a decoy note 5 under sibling root "b".
            for note in [10i64, 20, 30] {
                let text = EntryValue {
                    fields: vec![Some(RuntimeScalar::Str("n".into()))],
                };
                txn.create_entry(
                    &branch,
                    &[KeyScalar::Str("a".into()), KeyScalar::Int(note)],
                    text,
                )
                .expect("note create");
            }
            let decoy = EntryValue {
                fields: vec![Some(RuntimeScalar::Str("x".into()))],
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
}
