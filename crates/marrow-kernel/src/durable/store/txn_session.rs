//! The transaction session: one implicit single-writer engine transaction the export's
//! call graph joins, its Durable impl, commit reconcile, and managed-index maintenance.

use std::collections::BTreeMap;

use marrow_store::{ByteEngine, CommitOutcome, WriteTxn};

use super::super::physical;
use super::super::plan::{CellWrite, IndexOp, Planner};
use super::super::{
    AuthTarget, AuthorizedSite, BoundedKeys, BoundedLimit, CommitResult, CreateOutcome, EntryValue,
    EraseOutcome, FieldSchema, IndexComponent, IndexSchema, KernelFault, Presence, ReplaceOutcome,
};
use super::Durable;
use super::address::{
    field_index_in_record, field_name, group_target, node_shape, node_stem, read_raw, site_record,
};
use super::handle::WITNESS;
use super::index_ops::{op_index_lookup, op_index_scan};
use super::read_ops::{
    SlotClass, op_presence, op_read_entry, op_read_field, op_read_group, probe_slot,
};
use super::traverse::{op_family_populated, op_iterate_bounded};
use crate::codec::key::KeyScalar;
use crate::codec::value::{decode_domain, encode_domain};
use crate::equality::ValueDomain;

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
    pub(super) txn: Option<E::Txn<'s>>,
    /// The store's poison flag, set on an indeterminate commit so a reopen
    /// reclassifies.
    pub(super) poisoned: &'s mut bool,
    pub(super) auth: Vec<AuthorizedSite>,
    pub(super) token: [u8; 16],
    /// Each root's managed indexes, in stable declaration order, indexed by the root's
    /// declaration position (aligned to the store's schema table). A root-level write to
    /// root R keeps `indexes[R]` coherent as a consequence of the source write; a root
    /// with no index carries an empty list and skips maintenance entirely.
    pub(super) indexes: Vec<Vec<IndexSchema>>,
    /// The durable nodes whose fields were staged this transaction, keyed by the
    /// node's marker stem so several field sets on one node stage it once. Each is
    /// reconciled at commit to decide created vs required-missing — a root node or a
    /// branch node identically, since the stem and record are resolved when the field
    /// is staged rather than re-derived from the root schema.
    pub(super) pending: BTreeMap<Vec<u8>, PendingNode>,
}

/// A durable node staged for commit reconcile: its own record fields and the leaf-most
/// key of its address (for a `RequiredMissing` report). The node's marker stem is the
/// map key. This is what makes reconcile node-parametric — it validates the staged
/// node's marker and required fields at its own physical stem, one level down for a
/// branch node.
pub(super) struct PendingNode {
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
