//! The durable store handle and its read/transaction sessions (design §G).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::{ByteEngine, CommitOutcome, ReadView, StoreError, WriteTxn};

use super::physical::{self, CellKind};
use super::profile;
use super::{
    AuthTarget, AuthorizedSite, CommitResult, CreateOutcome, DemandCoverage, Denied, EntryValue,
    EraseOutcome, InvocationGrant, KernelFault, NextKey, Presence, Reopen, ReplaceOutcome,
    SessionError, SiteSpec, SiteTarget, StoreSchema,
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
    fn presence(&mut self, site: &AuthorizedSite, key: KeyScalar) -> Result<Presence, KernelFault>;
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<Option<RuntimeScalar>, KernelFault>;
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<Option<EntryValue>, KernelFault>;
    fn next_key(
        &mut self,
        site: &AuthorizedSite,
        after: Option<KeyScalar>,
    ) -> Result<NextKey, KernelFault>;
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        value: RuntimeScalar,
    ) -> Result<(), KernelFault>;
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault>;
    /// Set (present) or clear (vacant) a sparse field of an entry the caller has
    /// statically proven present. Asserts the entry marker is present — a violation
    /// is a marker/field mismatch ([`KernelFault::Corruption`]), never implicit
    /// creation — then stages the leaf exactly like [`Self::set_sparse`].
    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault>;
    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault>;
    fn replace_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault>;
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<EraseOutcome, KernelFault>;
    fn erase_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
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
    /// Whether the opened handle permits writes: the store ceiling's write atom.
    ceiling_writable: bool,
    poisoned: bool,
}

impl<E: ByteEngine> DurableStore<E> {
    /// Build a store over an already-open engine, minting the store ceiling from the
    /// handle's write capability.
    pub fn from_engine(engine: E, schema: StoreSchema, sites: Vec<SiteSpec>) -> Self {
        let ceiling_writable = engine.require_write_access("open").is_ok();
        Self {
            engine,
            schema,
            sites,
            ceiling_writable,
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
            .map(|site| AuthorizedSite {
                root: self.schema.root_name.clone(),
                key: self.schema.key,
                target: match site.target {
                    SiteTarget::WholePayload => AuthTarget::Entry,
                    SiteTarget::FieldLeaf(index) => {
                        let field = &self.schema.fields[index as usize];
                        AuthTarget::Field {
                            name: field.name.clone(),
                            kind: field.kind,
                            required: field.required,
                        }
                    }
                },
            })
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
        resolve_authority(demand, self.ceiling_writable, grant)
            .map_err(|Denied| SessionError::Denied)?;
        self.verify_profile()?;
        let auth = self.authorized_sites();
        let view = self.engine.read_view().map_err(SessionError::Engine)?;
        Ok(ReadSession {
            view,
            schema: &self.schema,
            auth,
        })
    }

    /// Open a transaction session after resolving effective authority, revalidating
    /// the profile, and provisioning the profile cell on a fresh store.
    pub fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, E>, SessionError> {
        resolve_authority(demand, self.ceiling_writable, grant)
            .map_err(|Denied| SessionError::Denied)?;
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

/// Resolve effective authority: `demand ⊆ ceiling ∩ grant`. Demand never grants;
/// it is only checked. The store ceiling permits reads unconditionally and writes
/// only when the handle is writable.
fn resolve_authority(
    demand: DemandCoverage,
    ceiling_writable: bool,
    grant: InvocationGrant,
) -> Result<(), Denied> {
    let read_ok = !demand.read || grant.read;
    let write_ok = !demand.write || (ceiling_writable && grant.write);
    if read_ok && write_ok {
        Ok(())
    } else {
        Err(Denied)
    }
}

/// A read session: reads observe one coherent view for the whole call. Non-`Clone`;
/// the view is released when the session drops.
pub struct ReadSession<'s, E: ByteEngine>
where
    E: 's,
{
    view: E::View<'s>,
    schema: &'s StoreSchema,
    auth: Vec<AuthorizedSite>,
}

impl<'s, E: ByteEngine + 's> Durable for ReadSession<'s, E> {
    fn site(&self, index: u16) -> AuthorizedSite {
        self.auth[index as usize].clone()
    }
    fn presence(&mut self, site: &AuthorizedSite, key: KeyScalar) -> Result<Presence, KernelFault> {
        op_presence(&self.view, site, &key)
    }
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<Option<RuntimeScalar>, KernelFault> {
        op_read_field(&self.view, site, &key)
    }
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<Option<EntryValue>, KernelFault> {
        op_read_entry(&self.view, self.schema, site, &key)
    }
    fn next_key(
        &mut self,
        site: &AuthorizedSite,
        after: Option<KeyScalar>,
    ) -> Result<NextKey, KernelFault> {
        op_next_key(&self.view, site, after)
    }
    fn set_required(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
        _value: RuntimeScalar,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn set_sparse(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
        _value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn set_sparse_present(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
        _value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn create_entry(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
        _entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn replace_entry(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
        _entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn erase_field(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
    ) -> Result<EraseOutcome, KernelFault> {
        unreachable!("verifier proved a read-only session performs no mutation")
    }
    fn erase_entry(
        &mut self,
        _site: &AuthorizedSite,
        _key: KeyScalar,
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
    /// gets its marker (created at commit).
    fn reconcile(&mut self) -> Result<(), CommitResult> {
        let root = self.schema.root_name.clone();
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
            let marker_key = physical::marker_key(&root, &key);
            let marker_present = read_raw(self.txn(), &marker_key)
                .map_err(|_| CommitResult::CommitFault)?
                .is_some();
            let mut any_leaf = false;
            let mut missing_required: Option<String> = None;
            for field in &self.schema.fields {
                let leaf = physical::field_leaf_key(&root, &key, &field.name);
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
    fn presence(&mut self, site: &AuthorizedSite, key: KeyScalar) -> Result<Presence, KernelFault> {
        op_presence(self.txn(), site, &key)
    }
    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<Option<RuntimeScalar>, KernelFault> {
        op_read_field(self.txn(), site, &key)
    }
    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<Option<EntryValue>, KernelFault> {
        op_read_entry(self.txn(), self.schema, site, &key)
    }
    fn next_key(
        &mut self,
        site: &AuthorizedSite,
        after: Option<KeyScalar>,
    ) -> Result<NextKey, KernelFault> {
        op_next_key(self.txn(), site, after)
    }
    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        value: RuntimeScalar,
    ) -> Result<(), KernelFault> {
        let name = field_name(site, true);
        let leaf = physical::field_leaf_key(&site.root, &key, name);
        let bytes = encode_value(&value).map_err(|_| KernelFault::ValueRange)?;
        self.txn_mut()
            .put(&leaf, bytes)
            .map_err(KernelFault::Engine)?;
        self.pending.insert(encode_key_value(&key));
        Ok(())
    }
    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        let name = field_name(site, false);
        let leaf = physical::field_leaf_key(&site.root, &key, name);
        match value {
            Some(value) => {
                let bytes = encode_value(&value).map_err(|_| KernelFault::ValueRange)?;
                self.txn_mut()
                    .put(&leaf, bytes)
                    .map_err(KernelFault::Engine)?;
                self.pending.insert(encode_key_value(&key));
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
        key: KeyScalar,
        value: Option<RuntimeScalar>,
    ) -> Result<(), KernelFault> {
        // The compiler's place-slot presence proof makes an absent marker
        // unreachable; assert it here as defense in depth over the trust boundary.
        // A present field leaf without a present entry marker is corruption, never
        // implicit creation (the marker law).
        let marker = physical::marker_key(&site.root, &key);
        if read_raw(self.txn(), &marker)?.is_none() {
            return Err(KernelFault::Corruption);
        }
        self.set_sparse(site, key, value)
    }
    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault> {
        let marker_key = physical::marker_key(&site.root, &key);
        if read_raw(self.txn(), &marker_key)?.is_some() {
            return Ok(CreateOutcome::AlreadyPresent);
        }
        self.write_entry(&site.root, &key, &entry)?;
        Ok(CreateOutcome::Created)
    }
    fn replace_entry(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
        entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        let marker_key = physical::marker_key(&site.root, &key);
        if read_raw(self.txn(), &marker_key)?.is_none() {
            return Ok(ReplaceOutcome::Missing);
        }
        // Exact replacement: remove the entry's marker and every field leaf by
        // exact key, then rewrite it, so unlisted sparse leaves do not survive.
        // The engine has no prefix delete; the kernel knows the entry's cells.
        self.remove_entry_cells(&site.root, &key)?;
        self.write_entry(&site.root, &key, &entry)?;
        Ok(ReplaceOutcome::Replaced)
    }
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        key: KeyScalar,
    ) -> Result<EraseOutcome, KernelFault> {
        let name = field_name(site, false);
        let leaf = physical::field_leaf_key(&site.root, &key, name);
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
        key: KeyScalar,
    ) -> Result<EraseOutcome, KernelFault> {
        let marker_key = physical::marker_key(&site.root, &key);
        let existed = read_raw(self.txn(), &marker_key)?.is_some();
        // Exact-cells removal of the whole entry: marker plus every field leaf.
        self.remove_entry_cells(&site.root, &key)?;
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
    /// Write an entry's marker and every present field leaf.
    fn write_entry(
        &mut self,
        root: &str,
        key: &KeyScalar,
        entry: &EntryValue,
    ) -> Result<(), KernelFault> {
        let mut writes: Vec<(Vec<u8>, Vec<u8>)> = vec![(
            physical::marker_key(root, key),
            physical::MARKER_VALUE.to_vec(),
        )];
        for (index, slot) in entry.fields.iter().enumerate() {
            if let Some(value) = slot {
                let name = &self.schema.fields[index].name;
                let leaf = physical::field_leaf_key(root, key, name);
                let bytes = encode_value(value).map_err(|_| KernelFault::ValueRange)?;
                writes.push((leaf, bytes));
            }
        }
        for (physical_key, value) in writes {
            self.txn_mut()
                .put(&physical_key, value)
                .map_err(KernelFault::Engine)?;
        }
        Ok(())
    }

    /// Remove every cell of an entry — its marker and one field leaf per schema
    /// field — by exact key. The engine offers only point removal, so the kernel
    /// enumerates the entry's cells from the schema rather than deleting a prefix.
    fn remove_entry_cells(&mut self, root: &str, key: &KeyScalar) -> Result<(), KernelFault> {
        let mut cells = vec![physical::marker_key(root, key)];
        for field in &self.schema.fields {
            cells.push(physical::field_leaf_key(root, key, &field.name));
        }
        for cell in cells {
            self.txn_mut().remove(&cell).map_err(KernelFault::Engine)?;
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
        AuthTarget::Entry => unreachable!("verifier proved a field-target site"),
    }
}

fn read_raw<V: ReadView>(cells: &V, key: &[u8]) -> Result<Option<Vec<u8>>, KernelFault> {
    cells.get(key).map_err(KernelFault::Engine)
}

fn op_presence<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    key: &KeyScalar,
) -> Result<Presence, KernelFault> {
    let physical_key = match &site.target {
        AuthTarget::Entry => physical::marker_key(&site.root, key),
        AuthTarget::Field { name, .. } => physical::field_leaf_key(&site.root, key, name),
    };
    Ok(match read_raw(cells, &physical_key)? {
        Some(_) => Presence::Present,
        None => Presence::Absent,
    })
}

fn op_read_field<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    key: &KeyScalar,
) -> Result<Option<RuntimeScalar>, KernelFault> {
    let AuthTarget::Field { name, kind, .. } = &site.target else {
        unreachable!("verifier proved a field read targets a field site")
    };
    let leaf = physical::field_leaf_key(&site.root, key, name);
    match read_raw(cells, &leaf)? {
        None => Ok(None),
        Some(bytes) => decode_value(&bytes, *kind)
            .map(Some)
            .ok_or(KernelFault::Corruption),
    }
}

fn op_read_entry<V: ReadView>(
    cells: &V,
    schema: &StoreSchema,
    site: &AuthorizedSite,
    key: &KeyScalar,
) -> Result<Option<EntryValue>, KernelFault> {
    let marker = physical::marker_key(&site.root, key);
    if read_raw(cells, &marker)?.is_none() {
        return Ok(None);
    }
    let mut fields = Vec::with_capacity(schema.fields.len());
    for field in &schema.fields {
        let leaf = physical::field_leaf_key(&site.root, key, &field.name);
        match read_raw(cells, &leaf)? {
            None => {
                // A present marker with a missing required field is a marker/field
                // mismatch: corruption, never implicit absence.
                if field.required {
                    return Err(KernelFault::Corruption);
                }
                fields.push(None);
            }
            Some(bytes) => {
                fields.push(Some(
                    decode_value(&bytes, field.kind).ok_or(KernelFault::Corruption)?,
                ));
            }
        }
    }
    Ok(Some(EntryValue { fields }))
}

fn op_next_key<V: ReadView>(
    cells: &V,
    site: &AuthorizedSite,
    after: Option<KeyScalar>,
) -> Result<NextKey, KernelFault> {
    let prefix = physical::entry_family_prefix(&site.root);
    let cursor = match &after {
        None => prefix.clone(),
        Some(key) => physical::cursor(&site.root, key),
    };
    let page = cells
        .scan_after(&prefix, &cursor)
        .map_err(KernelFault::Engine)?;
    let Some((cell_key, _)) = page.into_iter().next() else {
        return Ok(NextKey::End);
    };
    match physical::classify_cell(&site.root, &cell_key) {
        CellKind::Marker(key) => Ok(NextKey::Next(key)),
        CellKind::Orphan => Err(KernelFault::Corruption),
        CellKind::Foreign => Ok(NextKey::End),
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
        CommitResult, DemandCoverage, EntryValue, FieldSchema, InvocationGrant, KernelFault,
        NextKey, SessionError, SiteSpec, SiteTarget, StoreSchema,
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
                txn.create_entry(&entry, KeyScalar::Str(name.into()), value_entry(1))
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
                &physical::field_leaf_key("counters", &KeyScalar::Str("x".into()), "value"),
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
            KeyScalar::Str("x".into()),
            Some(RuntimeScalar::Str("hi".into())),
        )
        .expect("set sparse");
        assert!(matches!(txn.commit(), CommitResult::RequiredMissing { .. }));
    }
}
