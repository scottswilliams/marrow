//! The analysis snapshot's checked whole-document format consumes the one syntax-owned
//! format policy — the same `marrow fmt` uses — and reports a typed outcome.

use std::sync::Arc;

use marrow_compile::{FormatOutcome, FormatRefusal, InputRevision, QueryError, analyze};

use super::{identity, project, project_bytes};

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
fn snapshot_format_of_a_non_utf8_file_is_the_typed_invalid_utf8_outcome() {
    let input = project_bytes(&[("src/main.mw", vec![0xFF])]);
    let Ok(snapshot) = analyze(Arc::new(input), InputRevision::new(1)) else {
        panic!("a snapshot is produced even for a non-UTF-8 file");
    };
    assert!(matches!(
        snapshot.format(&identity("src/main.mw")),
        Ok(FormatOutcome::InvalidUtf8)
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
