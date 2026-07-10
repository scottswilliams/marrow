//! The payload-free error-diagnostic constructors shared across the checks: one
//! per diagnostic code, binding a fixed code so call sites read by intent.

use std::path::Path;

use marrow_syntax::{Severity, SourceSpan};

use crate::{CHECK_KEY_TYPE, CHECK_OPERATOR_TYPE, CHECK_RANGE, CheckDiagnostic};

/// A typed checkpoint over a diagnostic sink. It observes only diagnostics
/// appended after construction, and only error severity makes the delta fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErrorCheckpoint {
    diagnostics_len: usize,
}

impl ErrorCheckpoint {
    pub(crate) fn new(diagnostics: &[CheckDiagnostic]) -> Self {
        Self {
            diagnostics_len: diagnostics.len(),
        }
    }

    pub(crate) fn has_new_error(self, diagnostics: &[CheckDiagnostic]) -> bool {
        diagnostics[self.diagnostics_len..]
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// A `check.range` diagnostic located at a range expression's span.
pub(super) fn range_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_RANGE, file, span, message)
}

/// A `check.key_type` diagnostic located at a saved access's span.
pub(crate) fn key_type_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_KEY_TYPE, file, span, message)
}

/// A `check.operator_type` diagnostic located at an operator expression's span.
pub(super) fn operator_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_OPERATOR_TYPE, file, span, message)
}

#[cfg(test)]
mod error_checkpoint_tests {
    use std::path::Path;

    use marrow_syntax::SourceSpan;

    use super::ErrorCheckpoint;
    use crate::CheckDiagnostic;

    fn error() -> CheckDiagnostic {
        CheckDiagnostic::error(
            "test.error",
            Path::new("test.mw"),
            SourceSpan::default(),
            "error",
        )
    }

    fn warning() -> CheckDiagnostic {
        CheckDiagnostic::warning(
            "test.warning",
            Path::new("test.mw"),
            SourceSpan::default(),
            "warning",
        )
    }

    #[test]
    fn error_checkpoint_ignores_prior_errors_and_new_warnings() {
        let mut diagnostics = vec![error()];
        let checkpoint = ErrorCheckpoint::new(&diagnostics);

        assert!(!checkpoint.has_new_error(&diagnostics));
        diagnostics.push(warning());
        assert!(!checkpoint.has_new_error(&diagnostics));
    }

    #[test]
    fn error_checkpoint_detects_a_new_error() {
        let mut diagnostics = Vec::new();
        let checkpoint = ErrorCheckpoint::new(&diagnostics);

        diagnostics.push(error());
        assert!(checkpoint.has_new_error(&diagnostics));
    }
}
