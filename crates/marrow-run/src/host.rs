//! Host capabilities and the debugger `Frame`/`StepHook` view.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::RuntimeError;
use crate::value::Value;
use crate::write_plan::{WriteOp, WriteTarget};

/// The nondeterministic inputs a host or tool may capture at a run boundary.
pub trait Nondeterminism {
    fn now_nanos(&self) -> i128;
    fn entropy_u128(&mut self) -> u128;
}

/// Production nondeterminism from the operating system.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemNondeterminism;

impl SystemNondeterminism {
    pub fn new() -> Self {
        Self
    }
}

impl Nondeterminism for SystemNondeterminism {
    fn now_nanos(&self) -> i128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as i128)
            .unwrap_or(0)
    }

    fn entropy_u128(&mut self) -> u128 {
        system_entropy_u128()
    }
}

/// Fixed nondeterminism for deterministic runs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedNondeterminism {
    now_nanos: i128,
    entropy_u128: u128,
}

impl FixedNondeterminism {
    pub fn new(now_nanos: i128, entropy_u128: u128) -> Self {
        Self {
            now_nanos,
            entropy_u128,
        }
    }
}

impl Nondeterminism for FixedNondeterminism {
    fn now_nanos(&self) -> i128 {
        self.now_nanos
    }

    fn entropy_u128(&mut self) -> u128 {
        self.entropy_u128
    }
}

#[cfg(unix)]
fn system_entropy_u128() -> u128 {
    let file =
        std::fs::File::open("/dev/urandom").expect("nondeterminism entropy requires OS entropy");
    entropy_u128_from_reader(file)
}

fn entropy_u128_from_reader(mut reader: impl Read) -> u128 {
    let mut bytes = [0u8; 16];
    reader
        .read_exact(&mut bytes)
        .expect("nondeterminism entropy requires OS entropy");
    u128::from_be_bytes(bytes)
}

#[cfg(not(unix))]
fn system_entropy_u128() -> u128 {
    panic!("nondeterminism entropy requires OS entropy");
}

/// An opt-in debugger hook for statement-by-statement stepping and write
/// observation. An ordinary run installs none and pays one `Option` check per
/// statement. Returning `Err` from a callback aborts the run as a runtime fault.
pub trait StepHook {
    /// Called before the statement at `span` runs. Returning `Err` terminates the run.
    fn before_statement(
        &mut self,
        span: SourceSpan,
        frame: Frame<'_, '_>,
    ) -> Result<(), RuntimeError>;

    /// Called before each managed write (`value` is `Some`) or delete (`None`)
    /// lands, in commit order, at the `depth` of the producing statement. Purely
    /// observational: the default is a no-op and a returned value cannot abort.
    fn before_write(
        &mut self,
        op: WriteOp,
        target: &WriteTarget,
        value: Option<&[u8]>,
        depth: usize,
    ) {
        let _ = (op, target, value, depth);
    }

    /// Called when a source `transaction` block begins. `transaction_depth` is
    /// the source transaction nesting depth after entering the block.
    fn transaction_begin(&mut self, transaction_depth: usize) {
        let _ = transaction_depth;
    }

    /// Called when a source `transaction` block exits cleanly. `transaction_depth`
    /// is the source transaction nesting depth before leaving the block.
    fn transaction_commit(&mut self, transaction_depth: usize) {
        let _ = transaction_depth;
    }

    /// Called when a source `transaction` block aborts. `transaction_depth` is
    /// the source transaction nesting depth before leaving the block.
    fn transaction_rollback(&mut self, transaction_depth: usize) {
        let _ = transaction_depth;
    }
}

/// A read-only view of the current activation handed to a [`StepHook`]. It
/// borrows the live environment, so the store handle reads the run's own pending
/// writes. The two lifetimes are kept separate because `'p` is invariant (the env
/// holds a `&'p mut` hook), so a single shared lifetime would not unify.
pub struct Frame<'e, 'p> {
    pub(crate) env: &'e Env<'p>,
}

impl<'e, 'p> Frame<'e, 'p> {
    /// The locals in scope, innermost scope last and, within a scope, in binding
    /// order, so a consumer keeping the last occurrence per name sees shadowing.
    pub fn locals(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.env
            .scopes
            .iter()
            .flat_map(|scope| scope.iter())
            .map(|(name, binding)| (name.as_str(), &binding.value))
    }

    /// The live saved-data store handle — the same one the run reads and writes,
    /// so a hook sees the activation's own pending writes.
    pub fn store(&self) -> &TreeStore {
        self.env.store
    }

    /// The activation depth: 1 for the entry function, one more per nested call.
    /// Debuggers use it to express step-over and step-out by comparing depths
    /// across statements.
    pub fn depth(&self) -> usize {
        self.env.depth
    }

    /// The source file this activation runs in, or `None` without module context.
    /// Found by module name, since a span carries only line and column, not file.
    pub fn file(&self) -> Option<&std::path::Path> {
        self.env
            .program
            .modules()
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
    /// UTC instant in nanoseconds since the epoch, captured once so every
    /// `std::clock::now()` in the run sees one consistent instant.
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
    /// Whether the run may perform maintenance-only managed operations (dropping a
    /// whole managed root, deleting a required field). Tools select it explicitly
    /// for repair and administration; an ordinary run never does, so the protected
    /// operations stay unreachable on the default path.
    pub(crate) maintenance: bool,
}

impl Host {
    /// A host that provides no capabilities.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clock captures one instant from the given nondeterminism provider.
    pub fn with_nondeterminism(mut self, nondeterminism: &impl Nondeterminism) -> Self {
        self.clock = Some(nondeterminism.now_nanos());
        self
    }

    /// Clock returns a fixed instant (nanoseconds since the Unix epoch, UTC),
    /// for deterministic runs and tests.
    pub fn with_clock(mut self, nanos: i128) -> Self {
        self.clock = Some(nanos);
        self
    }

    /// Environment is the process's real environment variables, captured now.
    pub fn with_system_environment(mut self) -> Self {
        self.environment = Some(std::env::vars().collect());
        self
    }

    /// Environment is the given variables, for deterministic runs and tests.
    pub fn with_environment(mut self, variables: HashMap<String, String>) -> Self {
        self.environment = Some(variables);
        self
    }

    /// `std::log` output collects into the caller-owned `sink`, so a command can
    /// flush it to standard error and a test can inspect it.
    pub fn with_log_sink(mut self, sink: Rc<RefCell<String>>) -> Self {
        self.log = Some(sink);
        self
    }

    /// Grant `std::io` access to the real filesystem.
    pub fn with_filesystem(mut self) -> Self {
        self.filesystem = true;
        self
    }

    /// Opt in to maintenance-only managed operations. Reserved for explicit repair,
    /// restore, and administration tooling, never an ordinary run.
    pub fn with_maintenance(mut self) -> Self {
        self.maintenance = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    struct FailingEntropy;

    impl Read for FailingEntropy {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("entropy unavailable"))
        }
    }

    #[test]
    #[should_panic(expected = "nondeterminism entropy requires OS entropy")]
    fn entropy_reader_failure_panics() {
        super::entropy_u128_from_reader(FailingEntropy);
    }
}
