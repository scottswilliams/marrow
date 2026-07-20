//! The analysis snapshot answers hover at a source position with the compiler's
//! canonical type display for a resolved local or parameter use, and distinguishes a
//! genuine absence from a syntax-unavailable position and from an invalid coordinate.

use std::sync::Arc;

use marrow_compile::{AnalysisSnapshot, Fact, InputRevision, QueryError, Unavailability, analyze};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

fn project(files: &[(&str, &str)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// Analyze a project and unwrap its snapshot (the opaque `AnalysisFailure` is not
/// `Debug`, so a `let`-else keeps the failure boundary opaque).
fn snap(files: &[(&str, &str)]) -> Arc<AnalysisSnapshot> {
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot for {files:?}");
    };
    snapshot
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

/// The byte offset of the first occurrence of `needle` in `source`.
fn offset_of(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle present in source")
}

#[test]
fn hover_on_a_parameter_use_shows_its_value_type() {
    let source = "pub fn f(x: int): int {\n    return x\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = offset_of(source, "return x") + "return ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!(
            "expected Present(int), got a different outcome: {}",
            label(&other)
        ),
    }
}

#[test]
fn hover_on_a_local_use_shows_its_inferred_type() {
    let source = "pub fn f(): int {\n    const n = 7\n    return n\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    let use_offset = offset_of(source, "return n") + "return ".len();
    match snapshot.hover(&identity("src/main.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!("expected Present(int), got {}", label(&other)),
    }
}

#[test]
fn hover_on_a_valid_position_with_no_fact_is_absent() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    // The `1` literal is a valid position with no local/parameter fact.
    let literal = offset_of(source, "return 1") + "return ".len();
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), literal),
        Ok(Fact::Absent)
    ));
}

#[test]
fn hover_in_an_unknown_file_is_a_query_error() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(matches!(
        snapshot.hover(&identity("src/other.mw"), 0),
        Err(QueryError::UnknownFile)
    ));
}

#[test]
fn hover_at_an_out_of_range_offset_is_a_query_error_not_absence() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/main.mw", source)]);
    assert!(matches!(
        snapshot.hover(&identity("src/main.mw"), source.len() + 1),
        Err(QueryError::OffsetOutOfRange)
    ));
}

#[test]
fn hover_in_a_parse_failed_module_is_syntax_unavailable() {
    // The broken module still parses to an identity; a hover in it is Unavailable(Syntax).
    let broken = "module broken\n\npub fn g(: int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/broken.mw", broken)]);
    assert!(matches!(
        snapshot.hover(&identity("src/broken.mw"), 0),
        Ok(Fact::Unavailable(Unavailability::Syntax))
    ));
}

#[test]
fn a_valid_module_keeps_hover_facts_past_a_sibling_parse_error() {
    let valid = "module valid\n\npub fn h(x: int): int {\n    return x\n}\n";
    let broken = "module broken\n\npub fn g(: int {\n    return 1\n}\n";
    let snapshot = snap(&[("src/valid.mw", valid), ("src/broken.mw", broken)]);
    let use_offset = offset_of(valid, "return x") + "return ".len();
    match snapshot.hover(&identity("src/valid.mw"), use_offset) {
        Ok(Fact::Present(hover)) => assert_eq!(hover.display(), "int"),
        other => panic!(
            "expected Present(int) in the valid module, got {}",
            label(&other)
        ),
    }
}

fn label<T>(fact: &Result<Fact<T>, QueryError>) -> &'static str {
    match fact {
        Ok(Fact::Present(_)) => "Present",
        Ok(Fact::Absent) => "Absent",
        Ok(Fact::Unavailable(Unavailability::Syntax)) => "Unavailable(Syntax)",
        Ok(Fact::Unavailable(Unavailability::Dependency)) => "Unavailable(Dependency)",
        Err(QueryError::UnknownFile) => "Err(UnknownFile)",
        Err(QueryError::OffsetOutOfRange) => "Err(OffsetOutOfRange)",
    }
}
