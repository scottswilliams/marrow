use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{SourceOperatorHoverFact, source_operator_hover_fact_at};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    (snapshot, paths[0].clone())
}

fn analyze_files_with_diagnostics(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    (snapshot, paths)
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceOperatorHoverFact> {
    source_operator_hover_fact_at(snapshot, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

#[test]
fn source_operator_hover_fact_covers_checked_expression_operators_only() {
    let source = "\
module a

pub fn sum(x: int, y: int): int
    return x + y

pub fn both(left: bool, right: bool): bool
    return left and right
";
    let (snapshot, file) = analyze("source-operator-hover-expression", source);

    assert_eq!(
        fact_at(&snapshot, &file, offset(source, "+")),
        Some(SourceOperatorHoverFact {
            spelling: "+".to_string(),
            description: "addition.".to_string(),
        })
    );
    assert_eq!(
        fact_at(&snapshot, &file, offset(source, "and")),
        Some(SourceOperatorHoverFact {
            spelling: "and".to_string(),
            description: "logical conjunction.".to_string(),
        })
    );
}

#[test]
fn source_operator_hover_fact_refuses_keyword_path_and_declaration_segments() {
    let and_source = "\
module and

pub fn value(): int
    return 1
";
    let app_source = "\
module app

use and

pub fn run(): int
    return and::value()
";
    let (snapshot, paths) = analyze_files_with_diagnostics(
        "source-operator-hover-keyword-paths",
        &[("src/and.mw", and_source), ("src/app.mw", app_source)],
    );
    let and_file = &paths[0];
    let app_file = &paths[1];

    assert_eq!(
        fact_at(
            &snapshot,
            and_file,
            offset(and_source, "module and") + "module ".len()
        ),
        None
    );
    assert_eq!(
        fact_at(
            &snapshot,
            app_file,
            offset(app_source, "use and") + "use ".len()
        ),
        None
    );
    assert_eq!(
        fact_at(&snapshot, app_file, offset(app_source, "and::value")),
        None
    );
}
