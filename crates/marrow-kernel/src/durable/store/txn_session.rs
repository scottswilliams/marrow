//! The transaction session: one implicit single-writer engine transaction the export's
//! call graph joins, its Durable impl, the completeness law every payload write is held
//! to, and managed-index maintenance.

use marrow_store::{ByteEngine, CommitOutcome, WriteTxn};

use super::super::physical;
use super::super::plan::{CellWrite, IndexOp, Planner};
use super::super::{
    AuthorizedSite, BoundedKeys, BoundedLimit, CommitRecovery, CommitRecoveryScope, CommitResult,
    CreateOutcome, EntryValue, EraseOutcome, IndexComponentRef, IndexSchema, KernelFault, Presence,
    ResolvedField, ResolvedGroup,
};
use super::Durable;
use super::address::{
    field_index_in_record, field_target, group_target, node_shape, node_stem, read_raw, site_record,
};
use super::handle::WITNESS;
use super::index_ops::{op_index_lookup, op_index_scan};
use super::read_ops::{op_presence, op_read_entry, op_read_field, op_read_group, probe_slot};
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
    /// One slot per image site; `None` is a parked site no verified opcode addresses.
    pub(super) auth: Vec<Option<AuthorizedSite>>,
    /// The exact before/proposed-after witness states and lifecycle scope. Present until the
    /// commit resolves; moved into the sole affine fact only for an indeterminate verdict.
    pub(super) recovery: Option<RecoveryIntent>,
    /// Each root's managed indexes, in stable declaration order, indexed by the root's
    /// declaration position (aligned to the store's schema table). A root-level write to
    /// root R keeps `indexes[R]` coherent as a consequence of the source write; a root
    /// with no index carries an empty list and skips maintenance entirely.
    pub(super) indexes: Vec<Vec<IndexSchema>>,
}

/// Require that `values` fills the record `fields` completely: one slot per declared
/// field, every required one present. A width mismatch or a missing required value is
/// [`KernelFault::Incomplete`].
fn require_record_complete(
    fields: &[ResolvedField],
    values: &[Option<ValueDomain>],
) -> Result<(), KernelFault> {
    if values.len() != fields.len() {
        return Err(KernelFault::Incomplete);
    }
    let complete = fields
        .iter()
        .zip(values)
        .all(|(field, value)| value.is_some() || !field.required);
    complete.then_some(()).ok_or(KernelFault::Incomplete)
}

/// Require that `entry` is a complete payload for a node of `fields` and `groups`: its
/// record is complete, it carries exactly one sub-record per declared group, and each
/// sub-record is complete for its group (a group nests no further group). Runs before
/// the slot probe and before any engine access, so a refused write reads and stages
/// nothing: present implies complete, whatever image drives the kernel.
fn require_complete(
    fields: &[ResolvedField],
    groups: &[ResolvedGroup],
    entry: &EntryValue,
) -> Result<(), KernelFault> {
    require_record_complete(fields, &entry.fields)?;
    if entry.groups.len() != groups.len() {
        return Err(KernelFault::Incomplete);
    }
    for (group, value) in groups.iter().zip(&entry.groups) {
        require_record_complete(&group.fields, &value.fields)?;
        if !value.groups.is_empty() {
            return Err(KernelFault::Incomplete);
        }
    }
    Ok(())
}

/// The recovery material staged for this one transaction. It is private to the kernel and
/// cannot escape unless the engine reports an indeterminate commit.
pub(super) struct RecoveryIntent {
    pub(super) scope: Option<CommitRecoveryScope>,
    pub(super) before: Option<Vec<u8>>,
    pub(super) after: Vec<u8>,
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
        if self.txn.is_none() {
            return CommitResult::SessionFinished;
        }
        if *self.poisoned {
            return CommitResult::Aborted;
        }
        // The witness rides in the same engine transaction as the staged data.
        let witness = self
            .recovery
            .as_ref()
            .expect("a live transaction retains its recovery intent")
            .after
            .clone();
        if self
            .txn_mut()
            .put(&physical::meta_key(WITNESS), witness)
            .is_err()
        {
            self.txn = None;
            return CommitResult::Aborted;
        }
        match self.txn.take().expect("checked live above").commit() {
            CommitOutcome::Confirmed => {
                self.recovery = None;
                CommitResult::Committed
            }
            // A clean abort left the store unchanged and consumes no recovery fact.
            CommitOutcome::Aborted => {
                self.recovery = None;
                CommitResult::Aborted
            }
            CommitOutcome::Indeterminate => {
                *self.poisoned = true;
                let intent = self
                    .recovery
                    .take()
                    .expect("an indeterminate commit retains its recovery intent");
                CommitResult::Indeterminate(CommitRecovery {
                    scope: intent.scope,
                    before: intent.before,
                    after: intent.after,
                })
            }
        }
    }
}

impl<'s, E: ByteEngine + 's> Durable for TxnSession<'s, E> {
    fn site(&self, index: u16) -> AuthorizedSite {
        self.auth
            .get(index as usize)
            .cloned()
            .flatten()
            .expect("a verified durable opcode addresses a resolved site in the table")
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
        op_read_entry(self.txn(), site, keys)
    }
    fn read_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        op_read_group(self.txn(), site, keys)
    }
    fn read_group_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EntryValue, KernelFault> {
        op_read_group(self.txn(), site, keys)?.ok_or(KernelFault::Corruption)
    }
    fn replace_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: EntryValue,
    ) -> Result<(), KernelFault> {
        let (number, fields) = group_target(site);
        require_record_complete(fields, &value.fields)?;
        if !value.groups.is_empty() {
            return Err(KernelFault::Incomplete);
        }
        let stem = node_stem(site, keys)?;
        // A group has no independent existence: the compiler's presence proof makes an
        // absent marker unreachable, so one here is a marker/payload mismatch.
        if read_raw(self.txn(), &stem)?.is_none() {
            return Err(KernelFault::Corruption);
        }
        let group_stem = physical::group_stem(&stem, number);
        let planner = Planner::new();
        // Exact replacement scoped to the group's own leaves through the group-parametric
        // planner: remove them all, then write the present ones. The entry marker, the
        // entry's top-level fields, its sibling groups, and its branches are outside the
        // group prefix and untouched. A group leaf is not index-projected, so no managed
        // index maintenance runs.
        let mut ops = planner.group_erase(&group_stem, fields);
        ops.extend(planner.group_write(&group_stem, fields, &value)?);
        self.apply(ops)
    }
    fn erase_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        let (number, fields) = group_target(site);
        // A group holding a required leaf is part of every present entry; it is erased
        // only with its entry.
        if fields.iter().any(|field| field.required) {
            return Err(KernelFault::Incomplete);
        }
        let stem = node_stem(site, keys)?;
        let group_stem = physical::group_stem(&stem, number);
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
    fn set_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: ValueDomain,
    ) -> Result<(), KernelFault> {
        // The compiler's presence proof makes an absent marker unreachable; assert it
        // here as defense in depth over the trust boundary. A field leaf without an
        // entry marker is corruption, never implicit creation (the marker law).
        let stem = node_stem(site, keys)?;
        if read_raw(self.txn(), &stem)?.is_none() {
            return Err(KernelFault::Corruption);
        }
        let (number, _) = field_target(site);
        let leaf = physical::stem_field_leaf(&stem, number);
        let bytes = encode_domain(&value).map_err(|_| KernelFault::ValueRange)?;
        let maintenance = self.field_maintenance_before(site, &stem)?;
        self.txn_mut()
            .put(&leaf, bytes)
            .map_err(KernelFault::Engine)?;
        self.maintain_field_write(site, keys, maintenance, Some(value))
    }
    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault> {
        let (fields, groups) = node_shape(site);
        require_complete(fields, groups, &entry)?;
        let stem = node_stem(site, keys)?;
        let planner = Planner::new();
        // The probe refuses orphan payload before staging. The planner touches only
        // this entry's marker and own payload; other families remain independent.
        match probe_slot(self.txn(), &stem)? {
            Presence::Present => Ok(CreateOutcome::AlreadyPresent),
            Presence::Absent => {
                let ops = planner.node_write(&stem, fields, groups, &entry)?;
                self.apply(ops)?;
                if self.maintains_root(site) {
                    self.maintain_indexes(site, keys, None, Some(&entry.fields))?;
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
    ) -> Result<(), KernelFault> {
        let (fields, groups) = node_shape(site);
        require_complete(fields, groups, &entry)?;
        let stem = node_stem(site, keys)?;
        let planner = Planner::new();
        // The compiler lowers a whole assignment as exists?→replace:create, so replace
        // runs only on the present edge; a markerless node here is a marker/payload
        // mismatch, refused before any descendant could be touched.
        if read_raw(self.txn(), &stem)?.is_none() {
            return Err(KernelFault::Corruption);
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
            self.maintain_indexes(site, keys, Some(&old), Some(&entry.fields))?;
        }
        Ok(())
    }
    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        // A required field is present whenever its entry is, so it is never erased on
        // its own.
        let (number, required) = field_target(site);
        if required {
            return Err(KernelFault::Incomplete);
        }
        let stem = node_stem(site, keys)?;
        let leaf = physical::stem_field_leaf(&stem, number);
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
        // Remove the marker and every own field/group leaf by exact key. Child
        // entries occupy separate families and survive erasure of this payload.
        let ops = planner.node_erase(&stem, fields, groups);
        self.apply(ops)?;
        if maintains {
            self.maintain_indexes(site, keys, existed.then_some(old.as_slice()), None)?;
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
                    .projection()
                    .iter()
                    .filter_map(|component| match component.view() {
                        IndexComponentRef::Field(field) => Some(field as usize),
                        IndexComponentRef::Key(_) => None,
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
                index.projection().iter().any(|component| {
                    matches!(component.view(), IndexComponentRef::Field(field) if field as usize == position)
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
        record: &[ResolvedField],
        positions: &[usize],
    ) -> Result<Vec<Option<ValueDomain>>, KernelFault> {
        let mut fields = vec![None; record.len()];
        for &position in positions {
            let field = &record[position];
            let leaf = physical::stem_field_leaf(stem, field.number);
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
        let ops = Planner::new().index_writes(
            site.root_number,
            &indexes,
            keys,
            Some(&old),
            Some(&new),
        )?;
        self.apply_index_ops(ops)
    }

    /// Maintain every managed index for a whole root entry write. Each state carries
    /// entry presence separately from projected fields; an absent entry contributes
    /// no index cell, even when every component comes from its key.
    fn maintain_indexes(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        old: Option<&[Option<ValueDomain>]>,
        new: Option<&[Option<ValueDomain>]>,
    ) -> Result<(), KernelFault> {
        let ops =
            Planner::new().index_writes(site.root_number, self.indexes_of(site), keys, old, new)?;
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
