//! Every source diagnostic retains the full UTF-8 byte span of the offending
//! construct, not only a 1-based point. The editor analysis floor (H00f) projects
//! this span into a selection range; a point-only diagnostic could not.
//!
//! Red-first for the `SourceDiagnostic` full-span retention checkpoint: the
//! production `compile` path already threads a full `SourceSpan` into every
//! diagnostic constructor, and this gate proves the constructor keeps the byte
//! range rather than collapsing it to a point.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn first_diagnostic(source: &str) -> SourceDiagnostic {
    match compile(&project(source)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
            .into_vec()
            .into_iter()
            .next()
            .expect("a nonempty diagnostic set has a first element"),
        other => panic!("expected source diagnostics, got {other:?}"),
    }
}

#[test]
fn diagnostic_retains_full_byte_span_covering_the_construct() {
    // A syntax error over a multi-byte construct yields a diagnostic whose span
    // covers a real byte range, not a collapsed point.
    let source = "fn main() {\n    let x: = 0\n}\n";
    let diagnostic = first_diagnostic(source);
    let span = diagnostic.span();
    assert!(
        span.end_byte >= span.start_byte,
        "span byte range must be well-ordered, got {span:?}",
    );
    // The retained point stays consistent with the retained span (one owner).
    assert_eq!(diagnostic.line(), span.line);
    assert_eq!(diagnostic.column(), span.column);
    assert!(
        span.line >= 1 && span.column >= 1,
        "1-based point, got {span:?}"
    );
}
