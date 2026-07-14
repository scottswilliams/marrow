//! Typed source diagnostics (design failure family 1).
//!
//! A source diagnostic couples a stable `marrow-codes` string, the source file it
//! points into, a 1-based line/column, and a rendered message. It is distinct from
//! an artifact rejection, a runtime fault, and an operational error.

use marrow_syntax::SourceSpan;

/// A single source diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnostic {
    pub code: &'static str,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
}

impl SourceDiagnostic {
    pub(crate) fn at(code: &'static str, file: &str, span: SourceSpan, message: String) -> Self {
        Self {
            code,
            file: file.to_string(),
            line: span.line,
            column: span.column,
            message,
        }
    }
}
