//! The evaluation environment: scopes, bindings, and control flow.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use marrow_check::{CheckedRuntimeModule, CheckedRuntimeProgram, CheckedSavedPlace};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_syntax::SourceSpan;

use crate::error::{
    Located, RUN_CAPABILITY, RUN_TRAVERSAL, RuntimeError, raise_fault, write_fault,
};
use crate::host::{Host, StepHook};
use crate::store::{DataAddress, IndexAddress, LayerAddress, catalog_id};
use crate::value::Value;
use crate::write::{WriteError, validate_required_fields_for_entry};
use crate::write_plan::{PlanStep, WritePlan};

/// Where control flow stands after a statement or block.
pub(crate) enum Flow {
    /// Fall through to the next statement.
    Normal,
    /// A `return`, carrying its value if it had one.
    Return(Option<Value>),
    /// A `break`, targeting the named loop, or the innermost when unlabeled.
    Break(Option<String>),
    /// A `continue`, targeting the named loop, or the innermost when unlabeled.
    Continue(Option<String>),
    /// A `throw`, carrying the thrown `Error` value, unwinding until a `catch`
    /// handles it or it leaves the function as an uncaught-error fault.
    Throw(Value),
}

/// A name binding: its value and whether it may be reassigned (`var` vs `let`).
pub(crate) struct Binding {
    pub(crate) value: Value,
    pub(crate) mutable: bool,
}

/// Transaction state shared by every activation in one run.
#[derive(Default)]
pub(crate) struct TransactionState {
    /// How many user `transaction` blocks are open right now. Nonzero means a
    /// managed write's own steps already ride an open savepoint, so [`WritePlan`]
    /// applies them in place instead of wrapping them in a redundant
    /// begin/commit ([`WritePlan::commit`]'s `in_txn`).
    pub(crate) depth: usize,
    /// Entries touched by single-field writes inside transactions. Whole-record
    /// and whole-entry writes validate their required fields while planning; these
    /// deferred checks cover the transaction-only case where a block builds an
    /// entry field by field and must leave it complete by commit.
    pub(crate) required_entry_checks: Vec<RequiredEntryCheck>,
    /// Entries where maintenance deliberately deleted a required field or
    /// required-bearing group in the same transaction.
    pub(crate) maintenance_required_deletes: Vec<RequiredEntryCheck>,
    /// Required fields first created inside an open transaction. A later
    /// maintenance delete of the same path must not count as repairing existing
    /// invalid data.
    pub(crate) created_required_paths: Vec<RequiredPath>,
    pub(crate) pending_commit_metadata: Vec<PendingCommitMetadata>,
}

/// A resource or keyed-group entry whose required fields must be checked before
/// the surrounding transaction commits.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RequiredEntryCheck {
    pub(crate) depth: usize,
    pub(crate) place: CheckedSavedPlace,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) layers: Vec<LayerAddress>,
}

impl RequiredEntryCheck {
    fn same_entry(&self, other: &Self) -> bool {
        self.place.store_catalog_id == other.place.store_catalog_id
            && self.identity == other.identity
            && self.layers == other.layers
    }
}

/// A required materialized field path created while a transaction is open.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RequiredPath {
    pub(crate) depth: usize,
    pub(crate) path: DataAddress,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PendingCommitMetadata {
    pub(crate) depth: usize,
    pub(crate) root_catalog_ids: BTreeSet<CatalogId>,
    pub(crate) index_catalog_ids: BTreeSet<CatalogId>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum TraversedLayer {
    Record {
        store: CatalogId,
    },
    Data {
        store: CatalogId,
        identity: Vec<SavedKey>,
        path: Vec<DataPathSegment>,
    },
    Index {
        index: CatalogId,
        keys: Vec<SavedKey>,
    },
}

impl TraversedLayer {
    pub(crate) fn record(
        place: &CheckedSavedPlace,
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        Ok(Self::Record {
            store: catalog_id(&place.store_catalog_id, "store", span)?,
        })
    }

    pub(crate) fn data(address: DataAddress) -> Self {
        Self::Data {
            store: address.store,
            identity: address.identity,
            path: address.path,
        }
    }

    pub(crate) fn index(address: IndexAddress) -> Self {
        Self::Index {
            index: address.index,
            keys: address.keys,
        }
    }
}

/// The ambient state every activation in a run shares: the checked program (to
/// resolve calls), the saved-data store, the host capabilities, and run-global
/// transaction bookkeeping.
#[derive(Clone)]
pub(crate) struct Context<'p> {
    pub(crate) program: &'p CheckedRuntimeProgram,
    pub(crate) store: &'p TreeStore,
    pub(crate) host: &'p Host,
    pub(crate) transaction: Rc<RefCell<TransactionState>>,
}

/// A lexical environment: a stack of scopes, the ambient run context (program,
/// store, and host capabilities), and the shared output stream (so `print`/
/// `write` from any activation append to one buffer). A resource has few locals,
/// so lookups are linear and innermost-first.
pub(crate) struct Env<'p> {
    pub(crate) scopes: Vec<Vec<(String, Binding)>>,
    pub(crate) program: &'p CheckedRuntimeProgram,
    pub(crate) store: &'p TreeStore,
    pub(crate) host: &'p Host,
    pub(crate) output: Rc<RefCell<String>>,
    /// Saved record, data, and index layers loops are actively traversing,
    /// innermost last.
    pub(crate) traversed_layers: Vec<TraversedLayer>,
    /// The name of the module this activation runs in. Empty only for internal
    /// activations with no module context, where no project module hosts the
    /// body.
    pub(crate) module: &'p str,
    /// Transaction state is shared across helper calls so writes in callees obey
    /// the surrounding transaction's commit-time validation and savepoint rules.
    pub(crate) transaction: Rc<RefCell<TransactionState>>,
    /// The opt-in statement debugger. It is `None` for every ordinary run, where
    /// the per-statement check is a single `Option::is_none`. The hook is moved
    /// out before each call so it cannot alias the `&Env` borrowed by the frame,
    /// then moved back after nested activations return.
    pub(crate) hook: Option<&'p mut dyn StepHook>,
    /// This activation's call depth: 1 for the entry function, one more per
    /// nested call. A debugger uses it to express step-over and step-out by
    /// comparing depths across statements.
    pub(crate) depth: usize,
}

/// Why an assignment did not land.
pub(crate) enum AssignError {
    Unbound,
    Immutable,
}

impl<'p> Env<'p> {
    pub(crate) fn new(
        ctx: Context<'p>,
        output: Rc<RefCell<String>>,
        module: Option<&'p CheckedRuntimeModule>,
        hook: Option<&'p mut dyn StepHook>,
        depth: usize,
    ) -> Self {
        Self {
            scopes: Vec::new(),
            output,
            program: ctx.program,
            store: ctx.store,
            host: ctx.host,
            traversed_layers: Vec::new(),
            module: module.map_or("", |module| module.name.as_str()),
            transaction: Rc::clone(&ctx.transaction),
            hook,
            depth,
        }
    }

    pub(crate) fn guard_traversed_layer(
        &self,
        affected: &TraversedLayer,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        if self.traversed_layers.iter().any(|layer| layer == affected) {
            return Err(traversal_fault(span));
        }
        Ok(())
    }

    /// Gate a maintenance-only managed operation. Returns `Ok(())` when this run
    /// holds the maintenance capability ([`Host::with_maintenance`]); otherwise
    /// raises a catchable fault with `code`/`message` so the protected operation
    /// is rejected unless a tool explicitly opted in. Routing through
    /// [`raise_fault`] keeps the rejection catchable like the other write faults.
    pub(crate) fn require_maintenance(
        &self,
        code: &'static str,
        message: String,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        if self.host.maintenance {
            Ok(())
        } else {
            Err(raise_fault(code, message, span))
        }
    }

    /// Apply a planned managed write: surface a planning failure as a catchable
    /// `write.*` fault, then commit the plan's staged steps. `in_txn` is whether a
    /// user `transaction` is open, so the plan rides that savepoint instead of
    /// opening its own. A store failure during commit surfaces as a runtime store
    /// error.
    pub(crate) fn apply_plan(
        &mut self,
        plan: Result<WritePlan, WriteError>,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        let mut plan = plan.map_err(|error| write_fault(error, span))?;
        self.guard_plan_traversal(&plan, span)?;
        self.guard_generated_index_mutations(&plan, span)?;
        // Offer each staged operation to an installed write observer before it
        // lands, in commit order. An ordinary run has no hook, so this is a single
        // `is_some` check; only an opt-in debugger pays the per-step iteration.
        if let Some(hook) = self.hook.as_deref_mut() {
            for (op, target, value) in plan.steps() {
                hook.before_write(op, &target, value, self.depth);
            }
        }
        let in_transaction = self.transaction_depth() > 0;
        if in_transaction {
            self.note_managed_write_metadata(&plan);
        } else {
            stamp_managed_write(
                &mut plan,
                self.program.accepted_catalog_epoch(),
                self.program.source_digest(),
                self.store,
            )
            .map_err(|error| error.located(span))?;
        }
        plan.commit(self.store, in_transaction)
            .map_err(|error| error.located(span))
    }

    pub(crate) fn transaction_depth(&self) -> usize {
        self.transaction.borrow().depth
    }

    pub(crate) fn guard_rollback_sensitive_host_effect(
        &self,
        effect: &str,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        if self.transaction_depth() == 0 {
            return Ok(());
        }
        Err(RuntimeError::fault(
            RUN_CAPABILITY,
            format!(
                "`{effect}` cannot run inside a transaction because host effects cannot be rolled back"
            ),
            span,
        ))
    }

    pub(crate) fn enter_transaction(&self) -> usize {
        let mut transaction = self.transaction.borrow_mut();
        transaction.depth += 1;
        transaction.depth
    }

    pub(crate) fn leave_transaction(&self) {
        let mut transaction = self.transaction.borrow_mut();
        debug_assert!(transaction.depth > 0);
        transaction.depth -= 1;
    }

    pub(crate) fn defer_required_entry_check(
        &mut self,
        place: &CheckedSavedPlace,
        identity: &[SavedKey],
        layers: &[LayerAddress],
    ) {
        let depth = self.transaction_depth();
        if depth == 0 {
            return;
        }
        self.transaction
            .borrow_mut()
            .required_entry_checks
            .push(RequiredEntryCheck {
                depth,
                place: place.clone(),
                identity: identity.to_vec(),
                layers: layers.to_vec(),
            });
    }

    pub(crate) fn note_maintenance_required_delete(
        &mut self,
        place: &CheckedSavedPlace,
        identity: &[SavedKey],
        layers: &[LayerAddress],
    ) {
        let depth = self.transaction_depth();
        if depth == 0 || !self.host.maintenance {
            return;
        }
        self.transaction
            .borrow_mut()
            .maintenance_required_deletes
            .push(RequiredEntryCheck {
                depth,
                place: place.clone(),
                identity: identity.to_vec(),
                layers: layers.to_vec(),
            });
    }

    pub(crate) fn note_created_required_path(&mut self, path: DataAddress) {
        let depth = self.transaction_depth();
        if depth == 0 {
            return;
        }
        self.transaction
            .borrow_mut()
            .created_required_paths
            .push(RequiredPath { depth, path });
    }

    pub(crate) fn required_path_created_in_transaction(&self, path: &DataAddress) -> bool {
        let depth = self.transaction_depth();
        self.transaction
            .borrow()
            .created_required_paths
            .iter()
            .any(|created| created.depth <= depth && created.path == *path)
    }

    pub(crate) fn validate_required_entry_checks(
        &self,
        depth: usize,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        let transaction = self.transaction.borrow();
        let checks: Vec<RequiredEntryCheck> = transaction
            .required_entry_checks
            .iter()
            .filter(|check| check.depth == depth)
            .cloned()
            .collect();
        let maintenance_deletes: Vec<RequiredEntryCheck> = transaction
            .maintenance_required_deletes
            .iter()
            .filter(|check| check.depth <= depth)
            .cloned()
            .collect();
        drop(transaction);
        for check in checks {
            if maintenance_deletes
                .iter()
                .any(|deleted| deleted.same_entry(&check))
            {
                continue;
            }
            let exempt_layers: Vec<Vec<LayerAddress>> = maintenance_deletes
                .iter()
                .filter(|deleted| {
                    deleted.place.store_catalog_id == check.place.store_catalog_id
                        && deleted.identity == check.identity
                })
                .map(|deleted| deleted.layers.clone())
                .collect();
            validate_required_fields_for_entry(
                &check.place,
                &check.identity,
                &check.layers,
                &exempt_layers,
                self.store,
                span,
            )
            .map_err(|error| write_fault(error, span))?;
        }
        Ok(())
    }

    pub(crate) fn commit_required_entry_checks(&mut self, depth: usize) {
        let mut transaction = self.transaction.borrow_mut();
        lower_savepoint_level(&mut transaction.required_entry_checks, depth, |check| {
            &mut check.depth
        });
        lower_savepoint_level(
            &mut transaction.maintenance_required_deletes,
            depth,
            |check| &mut check.depth,
        );
        lower_savepoint_level(&mut transaction.created_required_paths, depth, |created| {
            &mut created.depth
        });
    }

    pub(crate) fn discard_required_entry_checks(&mut self, depth: usize) {
        let mut transaction = self.transaction.borrow_mut();
        transaction
            .required_entry_checks
            .retain(|check| check.depth < depth);
        transaction
            .maintenance_required_deletes
            .retain(|check| check.depth < depth);
        transaction
            .created_required_paths
            .retain(|created| created.depth < depth);
    }

    pub(crate) fn stamp_transaction_commit(
        &mut self,
        depth: usize,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        if depth != 1 {
            return Ok(());
        }
        let Some(catalog_epoch) = self.program.accepted_catalog_epoch() else {
            return Ok(());
        };
        let pending = self.pending_commit_metadata(depth);
        let stamp = build_commit_metadata_stamp(
            catalog_epoch,
            self.program.source_digest(),
            pending.root_catalog_ids.into_iter().collect(),
            pending.index_catalog_ids.into_iter().collect(),
            self.store,
        )
        .map_err(|error| error.located(span))?;
        let Some(stamp) = stamp else {
            return Ok(());
        };
        WritePlan { steps: vec![stamp] }
            .commit(self.store, true)
            .map_err(|error| error.located(span))
    }

    pub(crate) fn commit_transaction_metadata(&mut self, depth: usize) {
        let mut transaction = self.transaction.borrow_mut();
        lower_savepoint_level(&mut transaction.pending_commit_metadata, depth, |pending| {
            &mut pending.depth
        });
        merge_pending_commit_metadata(&mut transaction.pending_commit_metadata);
    }

    pub(crate) fn discard_transaction_metadata(&mut self, depth: usize) {
        let mut transaction = self.transaction.borrow_mut();
        transaction
            .pending_commit_metadata
            .retain(|pending| pending.depth < depth);
    }

    fn note_managed_write_metadata(&mut self, plan: &WritePlan) {
        if plan
            .steps
            .iter()
            .any(|step| matches!(step, PlanStep::StampMetadata { .. }))
        {
            return;
        }
        let (changed_root_catalog_ids, changed_index_catalog_ids) =
            changed_catalog_ids(&plan.steps);
        if changed_root_catalog_ids.is_empty() && changed_index_catalog_ids.is_empty() {
            return;
        }
        let depth = self.transaction_depth();
        let mut transaction = self.transaction.borrow_mut();
        let pending = pending_commit_metadata_at_depth(&mut transaction, depth);
        pending.root_catalog_ids.extend(changed_root_catalog_ids);
        pending.index_catalog_ids.extend(changed_index_catalog_ids);
    }

    fn pending_commit_metadata(&self, depth: usize) -> PendingCommitMetadata {
        let transaction = self.transaction.borrow();
        let mut pending = PendingCommitMetadata {
            depth,
            root_catalog_ids: BTreeSet::new(),
            index_catalog_ids: BTreeSet::new(),
        };
        for change in transaction
            .pending_commit_metadata
            .iter()
            .filter(|change| change.depth == depth)
        {
            pending
                .root_catalog_ids
                .extend(change.root_catalog_ids.iter().cloned());
            pending
                .index_catalog_ids
                .extend(change.index_catalog_ids.iter().cloned());
        }
        pending
    }

    fn guard_plan_traversal(&self, plan: &WritePlan, span: SourceSpan) -> Result<(), RuntimeError> {
        for step in &plan.steps {
            match step {
                PlanStep::WriteData { address, .. } => self.guard_data_write(address, span)?,
                PlanStep::DeleteData { address } => self.guard_data_delete(address, span)?,
                PlanStep::DeleteRecordSubtree { address } => {
                    self.guard_record_subtree_delete(address, span)?
                }
                PlanStep::WriteIndex { .. }
                | PlanStep::DeleteIndex { .. }
                | PlanStep::DeleteIndexSubtree { .. }
                | PlanStep::StampMetadata { .. } => {}
            }
        }
        Ok(())
    }

    fn guard_generated_index_mutations(
        &self,
        plan: &WritePlan,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        for step in &plan.steps {
            match step {
                PlanStep::WriteIndex { address, .. } | PlanStep::DeleteIndex { address, .. } => {
                    self.guard_index_mutation(address, span)?;
                }
                PlanStep::DeleteIndexSubtree { address } => {
                    self.guard_index_subtree_delete(address, span)?
                }
                PlanStep::WriteData { .. }
                | PlanStep::DeleteData { .. }
                | PlanStep::DeleteRecordSubtree { .. }
                | PlanStep::StampMetadata { .. } => {}
            }
        }
        Ok(())
    }

    fn guard_data_write(
        &self,
        address: &DataAddress,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        for layer in &self.traversed_layers {
            match layer {
                TraversedLayer::Record { store }
                    if store == &address.store
                        && !address.identity.is_empty()
                        && (address.path.is_empty() || !self.record_exists(address, span)?) =>
                {
                    return Err(traversal_fault(span));
                }
                TraversedLayer::Data {
                    store,
                    identity,
                    path,
                } if store == &address.store
                    && identity == &address.identity
                    && data_child_under(path, &address.path).is_some()
                    && !self.data_child_exists(address, path, span)? =>
                {
                    return Err(traversal_fault(span));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn guard_data_delete(
        &self,
        address: &DataAddress,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        for layer in &self.traversed_layers {
            match layer {
                TraversedLayer::Record { store }
                    if store == &address.store && !address.identity.is_empty() =>
                {
                    return Err(traversal_fault(span));
                }
                TraversedLayer::Data {
                    store,
                    identity,
                    path,
                } if store == &address.store
                    && identity == &address.identity
                    && data_child_under(path, &address.path).is_some() =>
                {
                    return Err(traversal_fault(span));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn guard_record_subtree_delete(
        &self,
        address: &DataAddress,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        for layer in &self.traversed_layers {
            match layer {
                TraversedLayer::Record { store } if store == &address.store => {
                    return Err(traversal_fault(span));
                }
                TraversedLayer::Data {
                    store, identity, ..
                } if store == &address.store && identity.starts_with(&address.identity) => {
                    return Err(traversal_fault(span));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn guard_index_mutation(
        &self,
        address: &IndexAddress,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        for layer in &self.traversed_layers {
            if let TraversedLayer::Index { index, keys } = layer
                && index == &address.index
                && address.keys.starts_with(keys)
            {
                return Err(traversal_fault(span));
            }
        }
        Ok(())
    }

    fn guard_index_subtree_delete(
        &self,
        address: &IndexAddress,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        for layer in &self.traversed_layers {
            if let TraversedLayer::Index { index, keys } = layer
                && index == &address.index
                && keys.starts_with(&address.keys)
            {
                return Err(traversal_fault(span));
            }
        }
        Ok(())
    }

    fn record_exists(&self, address: &DataAddress, span: SourceSpan) -> Result<bool, RuntimeError> {
        self.store
            .data_subtree_exists(&address.store, &address.identity, &[])
            .map_err(|error| error.located(span))
    }

    fn data_child_exists(
        &self,
        address: &DataAddress,
        active_path: &[DataPathSegment],
        span: SourceSpan,
    ) -> Result<bool, RuntimeError> {
        let Some(child) = data_child_under(active_path, &address.path) else {
            return Ok(true);
        };
        let mut child_path = active_path.to_vec();
        child_path.push(child.clone());
        self.store
            .data_subtree_exists(&address.store, &address.identity, &child_path)
            .map_err(|error| error.located(span))
    }

    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Bind `name` in the innermost scope, shadowing any binding further out.
    pub(crate) fn bind(&mut self, name: String, value: Value, mutable: bool) {
        self.scopes
            .last_mut()
            .expect("a scope is open")
            .push((name, Binding { value, mutable }));
    }

    /// The value bound to `name`, searching innermost scope first.
    pub(crate) fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(bound, _)| bound == name)
            .map(|(_, binding)| &binding.value)
    }

    /// Reassign an existing mutable binding.
    pub(crate) fn assign(&mut self, name: &str, value: Value) -> Result<(), AssignError> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some((_, binding)) = scope.iter_mut().rev().find(|(bound, _)| bound == name) {
                if !binding.mutable {
                    return Err(AssignError::Immutable);
                }
                binding.value = value;
                return Ok(());
            }
        }
        Err(AssignError::Unbound)
    }
}

fn stamp_managed_write(
    plan: &mut WritePlan,
    accepted_epoch: Option<u64>,
    source_digest: &str,
    store: &TreeStore,
) -> Result<(), marrow_store::StoreError> {
    let Some(catalog_epoch) = accepted_epoch else {
        return Ok(());
    };
    if plan
        .steps
        .iter()
        .any(|step| matches!(step, PlanStep::StampMetadata { .. }))
    {
        return Ok(());
    }
    let (changed_root_catalog_ids, changed_index_catalog_ids) = changed_catalog_ids(&plan.steps);
    if let Some(stamp) = build_commit_metadata_stamp(
        catalog_epoch,
        source_digest,
        changed_root_catalog_ids,
        changed_index_catalog_ids,
        store,
    )? {
        plan.steps.push(stamp);
    }
    Ok(())
}

/// Build the metadata-stamp step that records a commit against the accepted
/// catalog epoch, or `None` when nothing changed (so no stamp is owed). The
/// commit id is the next id after the last recorded commit.
fn build_commit_metadata_stamp(
    catalog_epoch: u64,
    source_digest: &str,
    changed_root_catalog_ids: Vec<CatalogId>,
    changed_index_catalog_ids: Vec<CatalogId>,
    store: &TreeStore,
) -> Result<Option<PlanStep>, marrow_store::StoreError> {
    if changed_root_catalog_ids.is_empty() && changed_index_catalog_ids.is_empty() {
        return Ok(None);
    }
    let commit_id = store
        .read_commit_metadata()?
        .map(|commit| commit.commit_id + 1)
        .unwrap_or(1);
    Ok(Some(crate::evolution::metadata_stamp(
        crate::evolution::StampFacts {
            catalog_epoch,
            commit_id,
            source_digest: source_digest.to_string(),
            changed_root_catalog_ids,
            changed_index_catalog_ids,
            activation: None,
        },
    )))
}

fn changed_catalog_ids(steps: &[PlanStep]) -> (Vec<CatalogId>, Vec<CatalogId>) {
    let mut roots = BTreeSet::new();
    let mut indexes = BTreeSet::new();
    for step in steps {
        match step {
            PlanStep::WriteData { address, .. }
            | PlanStep::DeleteData { address }
            | PlanStep::DeleteRecordSubtree { address } => {
                roots.insert(address.store.clone());
            }
            PlanStep::WriteIndex { address, .. }
            | PlanStep::DeleteIndex { address, .. }
            | PlanStep::DeleteIndexSubtree { address } => {
                indexes.insert(address.index.clone());
            }
            PlanStep::StampMetadata { .. } => {}
        }
    }
    (roots.into_iter().collect(), indexes.into_iter().collect())
}

fn pending_commit_metadata_at_depth(
    transaction: &mut TransactionState,
    depth: usize,
) -> &mut PendingCommitMetadata {
    if let Some(index) = transaction
        .pending_commit_metadata
        .iter()
        .position(|pending| pending.depth == depth)
    {
        return &mut transaction.pending_commit_metadata[index];
    }
    transaction
        .pending_commit_metadata
        .push(PendingCommitMetadata {
            depth,
            root_catalog_ids: BTreeSet::new(),
            index_catalog_ids: BTreeSet::new(),
        });
    transaction
        .pending_commit_metadata
        .last_mut()
        .expect("pending metadata was just inserted")
}

/// Lower one savepoint level on commit. A nested commit (`depth > 1`) folds items
/// recorded at this level into the enclosing savepoint by decrementing their depth;
/// the outermost commit (`depth == 1`) discards anything that level alone owned.
fn lower_savepoint_level<T>(
    items: &mut Vec<T>,
    depth: usize,
    item_depth: impl Fn(&mut T) -> &mut usize,
) {
    if depth > 1 {
        for item in items.iter_mut() {
            let item_depth = item_depth(item);
            if *item_depth == depth {
                *item_depth -= 1;
            }
        }
    } else {
        items.retain_mut(|item| *item_depth(item) < depth);
    }
}

fn merge_pending_commit_metadata(changes: &mut Vec<PendingCommitMetadata>) {
    let mut merged: Vec<PendingCommitMetadata> = Vec::new();
    for change in changes.drain(..) {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| existing.depth == change.depth)
        {
            existing.root_catalog_ids.extend(change.root_catalog_ids);
            existing.index_catalog_ids.extend(change.index_catalog_ids);
        } else {
            merged.push(change);
        }
    }
    *changes = merged;
}

fn data_child_under<'a>(
    active_path: &[DataPathSegment],
    affected_path: &'a [DataPathSegment],
) -> Option<&'a DataPathSegment> {
    if affected_path.len() <= active_path.len() || !affected_path.starts_with(active_path) {
        return None;
    }
    match &affected_path[active_path.len()] {
        segment @ DataPathSegment::Key(_) => Some(segment),
        DataPathSegment::Member(_) => None,
    }
}

fn traversal_fault(span: SourceSpan) -> RuntimeError {
    RuntimeError {
        throw: None,
        origin: None,
        code: RUN_TRAVERSAL,
        message: "this write changes the saved layer a loop is traversing; collect the keys into a local sequence first"
            .into(),
        span,
    }
}
