//! The payload-free error-diagnostic constructors shared across the checks: one
//! per diagnostic code, all delegating to `error_at` so the struct shape lives in
//! one place.

use std::path::Path;

use marrow_syntax::{Severity, SourceSpan};

use crate::{
    CHECK_CALL_ARGUMENT, CHECK_KEY_TYPE, CHECK_OPERATOR_TYPE, CHECK_RANGE, CheckDiagnostic,
    DiagnosticPayload,
};

/// Build a payload-free error diagnostic. The per-code constructors delegate here
/// so the struct shape lives in one place.
pub(super) fn error_at(
    code: &'static str,
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic {
        code,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message,
        span,
        payload: DiagnosticPayload::None,
    }
}

pub(super) fn range_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    error_at(CHECK_RANGE, file, span, message)
}

/// A `check.key_type` diagnostic located at a saved access's span.
pub(crate) fn key_type_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    error_at(CHECK_KEY_TYPE, file, span, message)
}

pub(crate) fn operator_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    error_at(CHECK_OPERATOR_TYPE, file, span, message)
}

/// A `check.call_argument` diagnostic located at a call's span.
pub(crate) fn call_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    error_at(CHECK_CALL_ARGUMENT, file, span, message)
}
