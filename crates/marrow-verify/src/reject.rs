//! The typed artifact-decode/verify rejection family (design failure family 2).
//!
//! A rejection names the verifier phase whose invariant the image violated and
//! carries a short static detail. The phase determines the stable `image.*` code;
//! tests assert the phase-specific code, and may inspect the detail. This family is
//! distinct from source diagnostics, runtime faults, and operational errors.

use marrow_codes::Code;

/// The verifier phase that rejected an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyPhase {
    /// Phase 1: magic, version, digest, section framing.
    Envelope,
    /// Phase 2: table decode and grammar.
    Table,
    /// Phase 3: per-function structural/type/local-init.
    Function,
    /// Phase 4: call/effect closure (cycle rejection).
    Closure,
    /// Phase 5: transaction-flow validation.
    Flow,
    /// Test-entry phase: the TEST-ENTRY table and `assert` legality.
    TestEntry,
}

impl VerifyPhase {
    /// The stable code for a rejection in this phase.
    pub fn code(self) -> Code {
        match self {
            VerifyPhase::Envelope => Code::ImageEnvelope,
            VerifyPhase::Table => Code::ImageTable,
            VerifyPhase::Function => Code::ImageFunction,
            VerifyPhase::Closure => Code::ImageClosure,
            VerifyPhase::Flow => Code::ImageFlow,
            VerifyPhase::TestEntry => Code::ImageTestEntry,
        }
    }
}

/// A typed image rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRejection {
    phase: VerifyPhase,
    detail: &'static str,
}

impl VerifyRejection {
    pub(crate) fn new(phase: VerifyPhase, detail: &'static str) -> Self {
        Self { phase, detail }
    }

    pub fn phase(&self) -> VerifyPhase {
        self.phase
    }

    /// The stable `image.*` code for the rejecting phase.
    pub fn code(&self) -> Code {
        self.phase.code()
    }

    pub fn detail(&self) -> &'static str {
        self.detail
    }
}

impl std::fmt::Display for VerifyRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code().as_str(), self.detail)
    }
}

impl std::error::Error for VerifyRejection {}
