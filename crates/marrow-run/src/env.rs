//! The evaluation environment: scopes, bindings, and control flow.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use marrow_check::{CheckedModule, CheckedProgram};
use marrow_store::backend::{Backend, Presence};
use marrow_store::path::{PathSegment, SavedKey, decode_path, encode_path};
use marrow_syntax::SourceSpan;

use crate::error::{
    Located, RUN_STORE, RUN_TRAVERSAL, RuntimeError, raise_fault, unsupported, write_fault,
};
use crate::host::{Host, StepHook};
use crate::schema_query::find_store_resource;
use crate::value::Value;
use crate::write::{WriteError, WriteOp, WritePlan, validate_required_fields_for_entry};

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
}

/// A resource or keyed-group entry whose required fields must be checked before
/// the surrounding transaction commits.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RequiredEntryCheck {
    pub(crate) depth: usize,
    pub(crate) root: String,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) layers: Vec<(String, Vec<SavedKey>)>,
}

impl RequiredEntryCheck {
    fn same_entry(&self, other: &Self) -> bool {
        self.root == other.root
            && self.identity == other.identity
            && self.entry_layers() == other.entry_layers()
    }

    fn entry_layers(&self) -> Vec<(String, Vec<SavedKey>)> {
        self.layers
            .iter()
            .filter(|(_, keys)| !keys.is_empty())
            .cloned()
            .collect()
    }
}

/// A required materialized field path created while a transaction is open.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RequiredPath {
    pub(crate) depth: usize,
    pub(crate) path: Vec<PathSegment>,
}

/// The ambient state every activation in a run shares: the checked program (to
/// resolve calls), the saved-data store, the host capabilities, and run-global
/// transaction bookkeeping.
#[derive(Clone)]
pub(crate) struct Context<'p> {
    pub(crate) program: &'p CheckedProgram,
    pub(crate) store: &'p RefCell<dyn Backend>,
    pub(crate) host: &'p Host,
    pub(crate) transaction: Rc<RefCell<TransactionState>>,
}

/// A lexical environment: a stack of scopes, the ambient run context (program,
/// store, and host capabilities), and the shared output stream (so `print`/
/// `write` from any activation append to one buffer). A resource has few locals,
/// so lookups are linear and innermost-first.
pub(crate) struct Env<'p> {
    pub(crate) scopes: Vec<Vec<(String, Binding)>>,
    pub(crate) program: &'p CheckedProgram,
    pub(crate) store: &'p RefCell<dyn Backend>,
    pub(crate) host: &'p Host,
    pub(crate) output: Rc<RefCell<String>>,
    /// Encoded path prefixes of the saved layers loops are actively traversing,
    /// innermost last. A write/delete/append whose affected layer is in this
    /// set mutates a layer being iterated, which is a [`RUN_TRAVERSAL`] fault.
    pub(crate) traversed_layers: Vec<Vec<u8>>,
    /// Encoded prefixes for active generated-index traversals. Managed field and
    /// resource writes may mutate these through generated index maintenance even
    /// when the user-visible target is an ordinary field.
    pub(crate) traversed_index_layers: Vec<Vec<u8>>,
    /// The name of the module this activation runs in, so a call inside it
    /// resolves from the right module (a bare name in its own module, a qualified
    /// name elsewhere) through the unified resolver, and a bare `Enum::member`
    /// resolves to that module's enum first — the same same-module identity the
    /// checker recorded. Empty only for internal activations with no module
    /// context, where no project module hosts the body.
    pub(crate) module: &'p str,
    /// The active function's module short→full import alias map, so a short-form
    /// call (`clock::now()`) expands to its full path (`std::clock::now`) before
    /// dispatch, exactly as the checker resolved it. Empty for modules with no
    /// imports, making expansion a strict no-op there.
    pub(crate) aliases: HashMap<String, Vec<String>>,
    /// Transaction state is shared across helper calls so writes in callees obey
    /// the surrounding transaction's commit-time validation and savepoint rules.
    pub(crate) transaction: Rc<RefCell<TransactionState>>,
    /// The opt-in statement debugger, installed only by
    /// [`run_entry_with_debugger`]; `None` for every ordinary run, where the
    /// per-statement check is a single `Option::is_none`. The hook is moved
    /// (`Option::take`) out before each call so it cannot alias the `&Env` that
    /// the [`Frame`] borrows, then moved back — threading the borrow with no
    /// `unsafe`. It rides each nested activation by being moved into the callee's
    /// [`invoke`] and returned to the caller afterward.
    pub(crate) hook: Option<&'p mut dyn StepHook>,
    /// This activation's call depth: 1 for the entry function, one more per
    /// nested call. Exposed via [`Frame::depth`] so a debugger can express
    /// step-over and step-out by comparing depths across statements.
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
        module: Option<&'p CheckedModule>,
        hook: Option<&'p mut dyn StepHook>,
        depth: usize,
    ) -> Self {
        // The activation's module supplies both its name (for resolving the calls
        // inside it and a bare `Enum::member`) and its short→full import aliases.
        let aliases = module
            .map(|module| marrow_check::build_alias_map(&module.imports))
            .unwrap_or_default();
        Self {
            scopes: Vec::new(),
            output,
            program: ctx.program,
            store: ctx.store,
            host: ctx.host,
            traversed_layers: Vec::new(),
            traversed_index_layers: Vec::new(),
            module: module.map_or("", |module| module.name.as_str()),
            aliases,
            transaction: Rc::clone(&ctx.transaction),
            hook,
            depth,
        }
    }

    /// Fault if `affected` (an encoded saved-layer prefix) is a layer a loop is
    /// actively traversing. Called before a write/delete/append commits, so
    /// a self-mutating traversal stops before it changes the iterated key set.
    pub(crate) fn guard_traversed_layer(
        &self,
        affected: &[PathSegment],
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        let affected = encode_path(affected);
        if self.traversed_layers.iter().any(|layer| layer == &affected) {
            return Err(RuntimeError {
                throw: None,
                origin: None,
                code: RUN_TRAVERSAL,
                message: "this write changes the saved layer a loop is traversing; \
                          collect the keys into a local sequence first"
                    .into(),
                span,
            });
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
        let plan = plan.map_err(|error| write_fault(error, span))?;
        self.guard_plan_traversal(&plan, span)?;
        self.guard_generated_index_mutations(&plan, span)?;
        // Offer each staged operation to an installed write observer before it
        // lands, in commit order. An ordinary run has no hook, so this is a single
        // `is_some` check; only an opt-in debugger pays the per-step iteration.
        if let Some(hook) = self.hook.as_deref_mut() {
            for (op, path, value) in plan.steps() {
                hook.before_write(op, path, value, self.depth);
            }
        }
        plan.commit(&mut *self.store.borrow_mut(), self.transaction_depth() > 0)
            .map_err(|error| error.located(span))
    }

    pub(crate) fn transaction_depth(&self) -> usize {
        self.transaction.borrow().depth
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
        root: &str,
        identity: &[SavedKey],
        layers: &[(&str, &[SavedKey])],
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
                root: root.to_string(),
                identity: identity.to_vec(),
                layers: layers
                    .iter()
                    .map(|(name, keys)| ((*name).to_string(), keys.to_vec()))
                    .collect(),
            });
    }

    pub(crate) fn note_maintenance_required_delete(
        &mut self,
        root: &str,
        identity: &[SavedKey],
        layers: &[(&str, &[SavedKey])],
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
                root: root.to_string(),
                identity: identity.to_vec(),
                layers: layers
                    .iter()
                    .map(|(name, keys)| ((*name).to_string(), keys.to_vec()))
                    .collect(),
            });
    }

    pub(crate) fn note_created_required_path(&mut self, path: Vec<PathSegment>) {
        let depth = self.transaction_depth();
        if depth == 0 {
            return;
        }
        self.transaction
            .borrow_mut()
            .created_required_paths
            .push(RequiredPath { depth, path });
    }

    pub(crate) fn required_path_created_in_transaction(&self, path: &[PathSegment]) -> bool {
        let depth = self.transaction_depth();
        self.transaction
            .borrow()
            .created_required_paths
            .iter()
            .any(|created| created.depth <= depth && created.path == path)
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
        let store = self.store.borrow();
        for check in checks {
            if maintenance_deletes
                .iter()
                .any(|deleted| deleted.same_entry(&check))
            {
                continue;
            }
            let exempt_layers: Vec<Vec<(String, Vec<SavedKey>)>> = maintenance_deletes
                .iter()
                .filter(|deleted| deleted.root == check.root && deleted.identity == check.identity)
                .map(RequiredEntryCheck::entry_layers)
                .collect();
            let (store_schema, resource) = find_store_resource(self.program, &check.root)
                .ok_or_else(|| {
                    unsupported("validating required fields for this saved root", span)
                })?;
            let layer_refs: Vec<(&str, &[SavedKey])> = check
                .layers
                .iter()
                .map(|(name, keys)| (name.as_str(), keys.as_slice()))
                .collect();
            validate_required_fields_for_entry(
                store_schema,
                resource,
                &check.identity,
                &layer_refs,
                &exempt_layers,
                &*store,
            )
            .map_err(|error| write_fault(error, span))?;
        }
        Ok(())
    }

    pub(crate) fn commit_required_entry_checks(&mut self, depth: usize) {
        let mut transaction = self.transaction.borrow_mut();
        if depth > 1 {
            for check in &mut transaction.required_entry_checks {
                if check.depth == depth {
                    check.depth -= 1;
                }
            }
        } else {
            transaction
                .required_entry_checks
                .retain(|check| check.depth < depth);
        }
        if depth > 1 {
            for check in &mut transaction.maintenance_required_deletes {
                if check.depth == depth {
                    check.depth -= 1;
                }
            }
        } else {
            transaction
                .maintenance_required_deletes
                .retain(|check| check.depth < depth);
        }
        if depth > 1 {
            for created in &mut transaction.created_required_paths {
                if created.depth == depth {
                    created.depth -= 1;
                }
            }
        } else {
            transaction
                .created_required_paths
                .retain(|created| created.depth < depth);
        }
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

    fn guard_plan_traversal(&self, plan: &WritePlan, span: SourceSpan) -> Result<(), RuntimeError> {
        if self.traversed_layers.is_empty() {
            return Ok(());
        }
        for (op, path, _) in plan.steps() {
            let path = decode_path(path).ok_or_else(|| RuntimeError {
                throw: None,
                origin: None,
                code: RUN_STORE,
                message: "planned write path is malformed".into(),
                span,
            })?;
            for layer in &self.traversed_layers {
                let layer = decode_path(layer).ok_or_else(|| RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_STORE,
                    message: "active traversal path is malformed".into(),
                    span,
                })?;
                if plan_step_changes_layer_keys(op, &path, &layer, self, span)? {
                    return Err(RuntimeError {
                        throw: None,
                        origin: None,
                        code: RUN_TRAVERSAL,
                        message: "this write changes the saved layer a loop is traversing; \
                                  collect the keys into a local sequence first"
                            .into(),
                        span,
                    });
                }
            }
        }
        Ok(())
    }

    fn guard_generated_index_mutations(
        &self,
        plan: &WritePlan,
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        if self.traversed_index_layers.is_empty() {
            return Ok(());
        }
        for (_, path, _) in plan.steps() {
            if self
                .traversed_index_layers
                .iter()
                .any(|prefix| path.starts_with(prefix.as_slice()))
            {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_TRAVERSAL,
                    message: "this write changes the saved layer a loop is traversing; \
                              collect the keys into a local sequence first"
                        .into(),
                    span,
                });
            }
        }
        Ok(())
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

fn plan_step_changes_layer_keys(
    op: WriteOp,
    path: &[PathSegment],
    layer: &[PathSegment],
    env: &Env<'_>,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    let Some(child) = path.get(layer.len()) else {
        return Ok(false);
    };
    if path[..layer.len()] != *layer
        || !matches!(child, PathSegment::RecordKey(_) | PathSegment::IndexKey(_))
    {
        return Ok(false);
    }
    let child_path = encode_path(&path[..=layer.len()]);
    match op {
        WriteOp::Write => {
            let presence = env
                .store
                .borrow()
                .presence(&child_path)
                .map_err(|error| error.located(span))?;
            Ok(matches!(presence, Presence::Absent))
        }
        WriteOp::Delete => Ok(path.len() == layer.len() + 1),
    }
}
