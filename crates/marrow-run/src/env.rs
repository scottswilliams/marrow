//! The evaluation environment: scopes, bindings, and control flow.

use crate::*;

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

/// The ambient state every activation in a run shares: the checked program (to
/// resolve calls), the saved-data store, and the host capabilities. All three
/// are borrowed for the run's lifetime, so the context is cheap to copy.
#[derive(Clone, Copy)]
pub(crate) struct Context<'p> {
    pub(crate) program: &'p CheckedProgram,
    pub(crate) store: &'p RefCell<dyn Backend>,
    pub(crate) host: &'p Host,
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
    /// innermost last. A write/delete/append/merge whose affected layer is in this
    /// set mutates a layer being iterated, which is a [`RUN_TRAVERSAL`] fault.
    pub(crate) traversed_layers: Vec<Vec<u8>>,
    /// The name of the module this activation runs in, so a call inside it
    /// resolves from the right module (a bare name in its own module, a qualified
    /// name elsewhere) through the unified resolver, and a bare `Enum::member`
    /// resolves to that module's enum first — the same same-module identity the
    /// checker recorded. Empty for the bare-program [`evaluate_function`] path,
    /// where no project module hosts the body and enums are project-unique.
    pub(crate) module: &'p str,
    /// The active function's module short→full import alias map, so a short-form
    /// call (`clock::now()`) expands to its full path (`std::clock::now`) before
    /// dispatch, exactly as the checker resolved it. Empty for the bare-program
    /// [`evaluate_function`] path and any module with no imports, making expansion
    /// a strict no-op there.
    pub(crate) aliases: HashMap<String, Vec<String>>,
    /// How many user `transaction` blocks are open right now. Nonzero means a
    /// managed write's own steps already ride an open savepoint, so [`WritePlan`]
    /// applies them in place instead of wrapping them in a redundant
    /// begin/commit ([`WritePlan::commit`]'s `in_txn`). Incremented on entering a
    /// `transaction` block and decremented as it commits or rolls back.
    pub(crate) transaction_depth: usize,
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
        // The bare-program path has no module: an empty name and no aliases
        // (expansion is a no-op).
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
            module: module.map_or("", |module| module.name.as_str()),
            aliases,
            transaction_depth: 0,
            hook,
            depth,
        }
    }

    /// Fault if `affected` (an encoded saved-layer prefix) is a layer a loop is
    /// actively traversing. Called before a write/delete/append/merge commits, so
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
        // Offer each staged operation to an installed write observer before it
        // lands, in commit order. An ordinary run has no hook, so this is a single
        // `is_some` check; only an opt-in debugger pays the per-step iteration.
        if let Some(hook) = self.hook.as_deref_mut() {
            for (op, path, value) in plan.steps() {
                hook.before_write(op, path, value, self.depth);
            }
        }
        plan.commit(&mut *self.store.borrow_mut(), self.transaction_depth > 0)
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
