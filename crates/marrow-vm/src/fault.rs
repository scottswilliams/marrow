//! Typed source-mapped runtime faults (design failure family 3).
//!
//! A fault carries a stable `run.*` code and the source line/column of the
//! faulting instruction. Runtime faults are source-uncatchable: they are distinct
//! from the language `Result<T,E>`, from source diagnostics, and from artifact
//! rejections.

use std::rc::Rc;

use marrow_kernel::durable::{CommitRecovery, DurableCommitState};

/// A runtime fault, mapped to the source position of the faulting instruction.
/// `detail` carries the static author text of an `unreachable("...")` fault; every
/// other fault has none. It is presentation-only: the stable typed fault surface is
/// the code and span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFault {
    code: &'static str,
    line: u32,
    column: u32,
    detail: Option<Rc<str>>,
}

impl RuntimeFault {
    pub(crate) fn new(code: &'static str, line: u32, column: u32) -> Self {
        Self {
            code,
            line,
            column,
            detail: None,
        }
    }

    /// A fault carrying static author text (an `unreachable("...")` invariant).
    pub(crate) fn with_detail(code: &'static str, line: u32, column: u32, detail: Rc<str>) -> Self {
        Self {
            code,
            line,
            column,
            detail: Some(detail),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    pub fn column(&self) -> u32 {
        self.column
    }

    /// The static author text, present only for an `unreachable("...")` fault.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl std::fmt::Display for RuntimeFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}:{}", self.code, self.line, self.column)
    }
}

impl std::error::Error for RuntimeFault {}

/// A durable invocation that stopped without completing its bytecode path.
///
/// The runtime fault remains available for source reporting, while the durable
/// state records what is known about the transaction independently. An
/// indeterminate commit initially owns a private affine recovery fact; only the
/// attached-store lifecycle may consume that fact and replace it with a
/// classified state.
///
/// ```compile_fail
/// use marrow_vm::InvocationIncomplete;
/// fn require_partial_eq<T: PartialEq>() {}
/// fn main() {
///     require_partial_eq::<InvocationIncomplete>();
/// }
/// ```
#[must_use = "an incomplete invocation and any pending commit recovery must be handled"]
#[derive(Debug)]
pub struct InvocationIncomplete {
    fault: RuntimeFault,
    durability: IncompleteDurability,
}

#[derive(Debug)]
enum IncompleteDurability {
    Classified(DurableCommitState),
    Pending(Box<CommitRecovery>),
}

/// The consuming projection of an incomplete invocation. Product hosts must
/// exhaustively preserve either the already classified durable state or the sole
/// opaque recovery fact; no callback can forge a classification inside the VM.
///
/// ```compile_fail
/// use marrow_vm::IncompleteDisposition;
/// fn require_partial_eq<T: PartialEq>() {}
/// fn main() {
///     require_partial_eq::<IncompleteDisposition>();
/// }
/// ```
#[must_use = "an incomplete invocation disposition must be projected or its attached service retired"]
#[derive(Debug)]
pub enum IncompleteDisposition {
    Classified {
        fault: RuntimeFault,
        durable: DurableCommitState,
    },
    Pending {
        fault: RuntimeFault,
        recovery: CommitRecovery,
    },
}

impl InvocationIncomplete {
    pub(crate) fn classified(fault: RuntimeFault, state: DurableCommitState) -> Self {
        Self {
            fault,
            durability: IncompleteDurability::Classified(state),
        }
    }

    pub(crate) fn pending(fault: RuntimeFault, recovery: CommitRecovery) -> Self {
        Self {
            fault,
            durability: IncompleteDurability::Pending(Box::new(recovery)),
        }
    }

    /// The classified durable state, or `None` while the attached lifecycle
    /// still owns an unresolved commit recovery.
    pub fn durable_state(&self) -> Option<DurableCommitState> {
        match &self.durability {
            IncompleteDurability::Classified(state) => Some(*state),
            IncompleteDurability::Pending(_) => None,
        }
    }

    /// Consume this incomplete invocation into its closed host disposition. A pending
    /// branch moves the sole affine recovery fact; the VM never accepts a caller-supplied
    /// durable classification.
    pub fn into_disposition(self) -> IncompleteDisposition {
        let Self { fault, durability } = self;
        match durability {
            IncompleteDurability::Classified(durable) => {
                IncompleteDisposition::Classified { fault, durable }
            }
            IncompleteDurability::Pending(recovery) => IncompleteDisposition::Pending {
                fault,
                recovery: *recovery,
            },
        }
    }

    pub fn runtime_fault(&self) -> &RuntimeFault {
        &self.fault
    }
}

/// Failure of durable execution. A plain runtime fault means no transaction was
/// confirmed by this invocation. `Incomplete` means bytecode did not complete
/// and durable state must be reported separately from the fault.
///
/// ```compile_fail
/// use marrow_vm::DurableExecutionFault;
/// fn require_partial_eq<T: PartialEq>() {}
/// fn main() {
///     require_partial_eq::<DurableExecutionFault>();
/// }
/// ```
#[must_use]
#[derive(Debug)]
pub enum DurableExecutionFault {
    Runtime(RuntimeFault),
    Incomplete(InvocationIncomplete),
}

impl DurableExecutionFault {
    pub(crate) fn classified(fault: RuntimeFault, state: DurableCommitState) -> Self {
        Self::Incomplete(InvocationIncomplete::classified(fault, state))
    }

    pub(crate) fn pending(fault: RuntimeFault, recovery: CommitRecovery) -> Self {
        Self::Incomplete(InvocationIncomplete::pending(fault, recovery))
    }

    pub fn code(&self) -> &'static str {
        self.runtime_fault().code()
    }

    pub fn line(&self) -> u32 {
        self.runtime_fault().line()
    }

    pub fn column(&self) -> u32 {
        self.runtime_fault().column()
    }

    pub fn detail(&self) -> Option<&str> {
        self.runtime_fault().detail()
    }

    pub fn durable_state(&self) -> Option<DurableCommitState> {
        match self {
            Self::Runtime(_) => None,
            Self::Incomplete(incomplete) => incomplete.durable_state(),
        }
    }

    pub fn is_incomplete(&self) -> bool {
        matches!(self, Self::Incomplete(_))
    }

    pub fn runtime_fault(&self) -> &RuntimeFault {
        match self {
            Self::Runtime(fault) => fault,
            Self::Incomplete(incomplete) => incomplete.runtime_fault(),
        }
    }
}

impl From<RuntimeFault> for DurableExecutionFault {
    fn from(fault: RuntimeFault) -> Self {
        Self::Runtime(fault)
    }
}
