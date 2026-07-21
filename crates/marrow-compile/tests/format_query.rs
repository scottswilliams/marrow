//! The analysis snapshot's checked whole-document format consumes the one syntax-owned
//! format policy — the same `marrow fmt` uses — and reports a typed outcome.

use std::sync::Arc;

use marrow_compile::{FormatOutcome, FormatRefusal, InputRevision, QueryError, analyze};
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

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

#[test]
fn snapshot_formats_a_clean_file() {
    // Unformatted but valid source formats to canonical form.
    let source = "pub fn f():int{\nreturn 1\n}\n";
    let Ok(snapshot) = analyze(
        Arc::new(project(&[("src/main.mw", source)])),
        InputRevision::new(1),
    ) else {
        panic!("a snapshot is produced");
    };
    match snapshot.format(&identity("src/main.mw")) {
        Ok(FormatOutcome::Formatted(formatted)) => {
            assert!(formatted.contains("pub fn f(): int"), "got: {formatted:?}");
        }
        _ => panic!("expected a formatted document"),
    }
}

#[test]
fn snapshot_format_refuses_a_parse_failed_file() {
    let source = "pub fn f(: int {\n    return 1\n}\n";
    let Ok(snapshot) = analyze(
        Arc::new(project(&[("src/main.mw", source)])),
        InputRevision::new(1),
    ) else {
        panic!("a snapshot is produced even for a broken file");
    };
    assert!(matches!(
        snapshot.format(&identity("src/main.mw")),
        Ok(FormatOutcome::Refused(FormatRefusal::ParseInvalid(_)))
    ));
}

#[test]
fn snapshot_format_of_an_unknown_file_is_a_query_error() {
    let source = "pub fn f(): int {\n    return 1\n}\n";
    let Ok(snapshot) = analyze(
        Arc::new(project(&[("src/main.mw", source)])),
        InputRevision::new(1),
    ) else {
        panic!("a snapshot is produced");
    };
    assert!(matches!(
        snapshot.format(&identity("src/other.mw")),
        Err(QueryError::UnknownFile)
    ));
}
