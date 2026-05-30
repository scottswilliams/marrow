//! Host capabilities and the debugger `Frame`/`StepHook` view.

use crate::*;

/// An opt-in debugger hook: the runtime calls [`StepHook::before_statement`]
/// once for each statement it is about to evaluate, in program order, so an
/// adapter (e.g. marrow-dap) can step statement-by-statement and inspect the
/// activation. A hook is installed only by [`run_entry_with_debugger`]; an
/// ordinary run installs none and pays at most one `if let Some` per statement.
///
/// Returning `Err` aborts the run with that error (a debugger 'terminate'),
/// surfacing it the way any runtime fault would.
pub trait StepHook {
    /// Called just before the statement at `span` runs, with a read-only view of
    /// the current activation. Returning `Err` terminates the run.
    fn before_statement(
        &mut self,
        span: SourceSpan,
        frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError>;

    /// Called just before each managed write or delete lands, once per staged
    /// operation in commit order, at the activation `depth` of the statement that
    /// produced it. `value` is `Some` for a write, `None` for a delete; `path` is
    /// the encoded saved path. The default is a no-op, so a statement-only hook is
    /// unaffected and an ordinary run that installs no hook pays nothing. Unlike
    /// [`StepHook::before_statement`], it is purely observational and cannot abort
    /// the run — the write proceeds regardless.
    fn before_write(&mut self, op: WriteOp, path: &[u8], value: Option<&[u8]>, depth: usize) {
        let _ = (op, path, value, depth);
    }
}

/// A read-only view of the current activation handed to a [`StepHook`]. It
/// borrows the live environment, so locals reflect the bindings in scope and the
/// store handle reads the run's own writes (read-your-writes). It exposes only
/// already-public types ([`Value`], [`Backend`]); the environment itself stays
/// private. The two lifetimes are the borrow (`'e`) and the run's borrowed state
/// (`'p`); they differ because `'p` is invariant (the env holds a `&'p mut`
/// hook), so a single shared lifetime would not unify.
pub struct Frame<'e, 'p> {
    pub(crate) env: &'e Env<'p>,
}

impl<'e, 'p> Frame<'e, 'p> {
    /// The locals visible at this point, innermost scope last and, within a
    /// scope, in binding order. A name rebound in an inner scope therefore appears
    /// after the outer one it shadows, so a consumer keeping the last occurrence
    /// per name observes shadowing correctly.
    pub fn locals(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.env
            .scopes
            .iter()
            .flat_map(|scope| scope.iter())
            .map(|(name, binding)| (name.as_str(), &binding.value))
    }

    /// The live saved-data store handle — the same one the run reads and writes,
    /// so a hook sees the activation's own pending writes.
    pub fn store(&self) -> &RefCell<dyn Backend> {
        self.env.store
    }

    /// The activation depth: 1 for the entry function, one more per nested call.
    /// A debugger compares depths across statements to express step-over and
    /// step-out.
    pub fn depth(&self) -> usize {
        self.env.depth
    }

    /// The source file this activation runs in, so a trace can render a statement
    /// as `file:line`. `None` for the bare-program path, which has no module and
    /// thus no source file. Found by the activation's module name, since a span
    /// carries only its line and column, not its file.
    pub fn file(&self) -> Option<&std::path::Path> {
        self.env
            .program
            .modules
            .iter()
            .find(|module| module.name == self.env.module)
            .map(|module| module.source_file.as_path())
    }
}

/// The host capabilities a run may use. Pure runs need none; host modules such
/// as `std::clock` require the matching capability, and a call made without it
/// raises a typed capability error (`run.capability`). A command or embedding
/// provides the capabilities its run needs.
#[derive(Debug, Clone, Default)]
pub struct Host {
    /// The run's UTC instant in nanoseconds since the epoch, when a clock
    /// capability is provided. Captured once, so every `std::clock::now()` in
    /// the run sees one consistent instant.
    pub(crate) clock: Option<i128>,
    /// The run's environment variables, when an environment capability is
    /// provided. A run without it cannot use `std::env`.
    pub(crate) environment: Option<HashMap<String, String>>,
    /// The run's log sink, when a log capability is provided. `std::log` appends
    /// formatted lines here; the command or embedding decides where they go
    /// (e.g. standard error). A run without it cannot use `std::log`.
    pub(crate) log: Option<Rc<RefCell<String>>>,
    /// Whether the run may touch the real filesystem through `std::io`. Marrow
    /// does not sandbox paths; the host either grants filesystem access or not.
    pub(crate) filesystem: bool,
    /// Whether this run may perform maintenance-only managed operations:
    /// dropping a whole managed root, deleting a required field on its own, and
    /// raw (quoted-segment) writes/reads under a managed root. Default `false`;
    /// a tool opts in explicitly with [`Host::with_maintenance`]. Maintenance is
    /// not a language feature — it is a host capability tools select for
    /// migration, repair, and restore. An ordinary `marrow run` of the default
    /// entry never sets it, so the protected operations stay unreachable there.
    pub(crate) maintenance: bool,
}

impl Host {
    /// A host that provides no capabilities.
    pub fn new() -> Self {
        Self::default()
    }

    /// A host whose clock reads the real system time, captured now.
    pub fn with_system_clock(mut self) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as i128)
            .unwrap_or(0);
        self.clock = Some(nanos);
        self
    }

    /// A host whose clock returns a fixed instant (nanoseconds since the Unix
    /// epoch, UTC), for deterministic runs and tests.
    pub fn with_clock(mut self, nanos: i128) -> Self {
        self.clock = Some(nanos);
        self
    }

    /// A host whose environment is the process's real environment variables,
    /// captured now.
    pub fn with_system_environment(mut self) -> Self {
        self.environment = Some(std::env::vars().collect());
        self
    }

    /// A host whose environment is the given variables, for deterministic runs
    /// and tests.
    pub fn with_environment(mut self, variables: HashMap<String, String>) -> Self {
        self.environment = Some(variables);
        self
    }

    /// A host that collects `std::log` output into `sink`. The caller owns the
    /// sink (a shared buffer), so a command can flush it to standard error and a
    /// test can inspect it.
    pub fn with_log_sink(mut self, sink: Rc<RefCell<String>>) -> Self {
        self.log = Some(sink);
        self
    }

    /// A host that grants `std::io` access to the real filesystem.
    pub fn with_filesystem(mut self) -> Self {
        self.filesystem = true;
        self
    }

    /// A host that may perform maintenance-only managed operations: dropping a
    /// whole managed root, deleting a required field on its own, and raw
    /// (quoted-segment) writes/reads under a managed root. Selected only by
    /// explicit tooling (migration, repair, restore) — never by an ordinary run,
    /// so the default path can never reach maintenance behavior.
    pub fn with_maintenance(mut self) -> Self {
        self.maintenance = true;
        self
    }
}
