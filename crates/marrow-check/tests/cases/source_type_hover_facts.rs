use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{SourceTypeHoverFact, source_type_hover_fact_at};
use marrow_check::{AnalysisSnapshot, BindingIndex, MarrowType, ScalarType, build_binding_index};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, BindingIndex, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths[0].clone())
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    source: &str,
    needle: &str,
) -> Option<SourceTypeHoverFact> {
    source_type_hover_fact_at(
        snapshot,
        index,
        file,
        source.find(needle).expect("needle is present"),
    )
}

fn int_ty() -> MarrowType {
    MarrowType::Primitive(ScalarType::Int)
}

#[test]
fn source_type_hover_fact_returns_plain_expression_type() {
    let source = "\
module a

pub fn f(id: int): int
    const doubled = id + 1
    return doubled
";
    let (snapshot, index, file) = analyze("source-type-hover-local", source);

    assert_eq!(
        fact_at(&snapshot, &index, &file, source, "doubled\n"),
        Some(SourceTypeHoverFact {
            ty: int_ty(),
            docs: Vec::new(),
        })
    );
}

#[test]
fn source_type_hover_fact_attaches_source_symbol_docs() {
    let source = "\
module a

;; Maximum count.
const LIMIT: int = 10

pub fn caller(): int
    return LIMIT
";
    let (snapshot, index, file) = analyze("source-type-hover-docs", source);

    assert_eq!(
        fact_at(&snapshot, &index, &file, source, "LIMIT\n"),
        Some(SourceTypeHoverFact {
            ty: int_ty(),
            docs: vec!["Maximum count.".to_string()],
        })
    );
}
