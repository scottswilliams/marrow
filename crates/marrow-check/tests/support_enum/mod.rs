//! Shared enum-diagnostic oracle for the enum checker suites.
//!
//! Every enum suite asserts the typed `EnumDiagnostic` payload a checker error
//! carries rather than its rendered prose. This module owns the one payload
//! assertion so the focused suites share a single oracle instead of each copying
//! it.

use marrow_check::{CheckDiagnostic, DiagnosticPayload, EnumDiagnostic};

pub fn assert_enum_payload(diagnostic: &CheckDiagnostic, expected: EnumDiagnostic) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::Enum(expected),
        "{diagnostic:#?}"
    );
}
