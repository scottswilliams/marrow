use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{SourceTypeHoverFact, source_type_hover_fact_at};
use marrow_check::{
    AnalysisSnapshot, BindingIndex, MarrowType, ScalarType, build_binding_index, type_at,
};

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

fn type_at_needle(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    source: &str,
    needle: &str,
) -> Option<MarrowType> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)
        .expect("analyzed file is present");
    type_at(
        &snapshot.program,
        file,
        &analyzed.parsed,
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

#[test]
fn explicit_dynamic_parameter_and_local_have_source_type_hover_facts() {
    let source = "\
module a

pub fn passthrough(input: unknown): unknown
    const local: unknown = input
    return local
";
    let (snapshot, index, file) = analyze("source-type-hover-dynamic", source);

    for needle in ["input\n", "local\n"] {
        assert_eq!(
            type_at_needle(&snapshot, &file, source, needle),
            Some(MarrowType::Dynamic),
        );
        assert_eq!(
            fact_at(&snapshot, &index, &file, source, needle),
            Some(SourceTypeHoverFact {
                ty: MarrowType::Dynamic,
                docs: Vec::new(),
            }),
        );
    }
}

#[test]
fn unresolved_name_has_no_source_type_hover_fact() {
    let source = "\
module a

pub fn broken(): int
    return missing
";
    let (snapshot, paths) =
        support::analyze_overlay("source-type-hover-unresolved-name", &[("src/a.mw", source)]);
    let index = build_binding_index(&snapshot);

    assert_eq!(
        type_at_needle(&snapshot, &paths[0], source, "missing"),
        Some(MarrowType::Invalid),
    );
    assert_eq!(
        fact_at(&snapshot, &index, &paths[0], source, "missing"),
        None,
    );
}

#[test]
fn diagnosed_saved_access_has_invalid_type_provenance() {
    let source = "\
module a

resource Book
    shelf: string

store ^books(id: int): Book

pub fn broken(): string
    return ^books.shelf
";
    let (snapshot, paths) = support::analyze_overlay(
        "source-type-hover-invalid-saved-access",
        &[("src/a.mw", source)],
    );

    assert_eq!(
        type_at_needle(&snapshot, &paths[0], source, "shelf\n"),
        Some(MarrowType::Invalid),
    );
}

#[test]
fn optional_field_access_preserves_invalid_member_type_poison() {
    let source = "\
module a

resource Book
    value: Missing

pub fn broken(book: Book): int
    return book?.value
";
    let (snapshot, paths) = support::analyze_overlay(
        "source-type-hover-optional-field-unknown",
        &[("src/a.mw", source)],
    );
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == paths[0])
        .expect("analyzed file is present");

    assert_eq!(
        type_at(
            &snapshot.program,
            &paths[0],
            &analyzed.parsed,
            source.rfind("value").expect("field access is present"),
        ),
        Some(MarrowType::Invalid),
    );
}

#[test]
fn nested_unresolved_type_has_no_source_type_hover_fact() {
    let source = "\
module a

pub fn broken(values: sequence[Missing])
    print(values)
";
    let (snapshot, paths) = support::analyze_overlay(
        "source-type-hover-nested-unresolved",
        &[("src/a.mw", source)],
    );
    let index = build_binding_index(&snapshot);
    let expected = MarrowType::Sequence(Box::new(MarrowType::Unknown));

    assert_eq!(
        type_at_needle(&snapshot, &paths[0], source, "values)"),
        Some(expected),
    );
    assert_eq!(
        fact_at(&snapshot, &index, &paths[0], source, "values)"),
        None,
    );
}

#[test]
fn no_value_call_has_no_source_type_hover_fact() {
    let source = "\
module a

fn finish()
    return

pub fn caller()
    finish()
    return
";
    let (snapshot, index, file) = analyze("source-type-hover-no-value", source);
    let offset = source.rfind("finish()").expect("call is present") + "finish(".len();
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)
        .expect("analyzed file is present");

    assert_eq!(
        type_at(&snapshot.program, &file, &analyzed.parsed, offset),
        Some(MarrowType::NoValue),
    );
    assert_eq!(
        source_type_hover_fact_at(&snapshot, &index, &file, offset),
        None,
    );
}
