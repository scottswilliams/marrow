//! Shared conversion-diagnostic oracle for the conversion checker suites.

#![allow(dead_code)]

use marrow_check::{
    ConversionTarget, ConversionUnsupportedSourceDiagnostic, DiagnosticPayload, MarrowType,
};

pub fn conversion_source_payload(
    target: ConversionTarget,
    source: MarrowType,
) -> DiagnosticPayload {
    DiagnosticPayload::ConversionUnsupportedSource(ConversionUnsupportedSourceDiagnostic {
        target,
        source,
        accepted_sources: target.accepted_source_types(),
    })
}
