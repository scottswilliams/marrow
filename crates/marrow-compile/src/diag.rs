//! Typed source diagnostics (design failure family 1).
//!
//! A source diagnostic couples a stable `marrow-codes` string, the source file it
//! points into, a 1-based line/column, and a rendered message. It is distinct from
//! an artifact rejection, a runtime fault, and an operational error.

use marrow_project::{IdentityAnchor, IdentityKind};
use marrow_syntax::SourceSpan;

/// A single source diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnostic {
    pub code: &'static str,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    /// The typed durable-identity gap behind a `check.durable_identity`
    /// diagnostic, `None` for every other code. The CLI's `marrow run` mint
    /// action consumes this — never the rendered message — to learn which
    /// anchors to mint, so the classifier stays in the compiler.
    pub identity: Option<IdentityGap>,
}

/// Why a durable declaration's identity is incomplete: its ledger anchor has no
/// row (mintable), or it names a retired anchor that can never be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityGap {
    pub kind: IdentityKind,
    pub path: String,
    pub retired: bool,
}

impl IdentityGap {
    /// The `(kind, path)` anchor this gap names.
    pub fn anchor(&self) -> IdentityAnchor {
        IdentityAnchor::new(self.kind, self.path.clone())
    }
}

impl SourceDiagnostic {
    pub(crate) fn at(code: &'static str, file: &str, span: SourceSpan, message: String) -> Self {
        Self {
            code,
            file: file.to_string(),
            line: span.line,
            column: span.column,
            message,
            identity: None,
        }
    }

    pub(crate) fn identity_gap(
        code: &'static str,
        file: &str,
        span: SourceSpan,
        message: String,
        gap: IdentityGap,
    ) -> Self {
        Self {
            code,
            file: file.to_string(),
            line: span.line,
            column: span.column,
            message,
            identity: Some(gap),
        }
    }
}
