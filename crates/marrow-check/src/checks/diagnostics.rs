//! The payload-free error-diagnostic constructors shared across the checks: one
//! per diagnostic code, binding a fixed code so call sites read by intent.

use std::path::Path;

use marrow_syntax::SourceSpan;

use crate::{
    CHECK_CALL_ARGUMENT, CHECK_KEY_TYPE, CHECK_OPERATOR_TYPE, CHECK_RANGE, CheckDiagnostic,
};

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

pub(crate) fn operator_diagnostic(
    file: &Path,
    span: SourceSpan,
    message: String,
) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_OPERATOR_TYPE, file, span, message)
}

/// A `check.call_argument` diagnostic located at a call's span.
pub(crate) fn call_diagnostic(file: &Path, span: SourceSpan, message: String) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_CALL_ARGUMENT, file, span, message)
}
