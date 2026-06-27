use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    SourceTypeAnnotationCursorFact, source_type_annotation_cursor_fact_at,
};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    (snapshot, paths[0].clone())
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceTypeAnnotationCursorFact> {
    source_type_annotation_cursor_fact_at(snapshot, file, offset)
}

fn span_text<'a>(source: &'a str, fact: &SourceTypeAnnotationCursorFact) -> &'a str {
    &source[fact.span.start_byte..fact.span.end_byte]
}

fn assert_type_fact_at(
    source: &str,
    snapshot: &AnalysisSnapshot,
    file: &Path,
    start: usize,
    expected: &str,
) {
    let fact = fact_at(snapshot, file, start)
        .unwrap_or_else(|| panic!("type annotation cursor fact for `{expected}` at byte {start}"));
    assert_eq!(span_text(source, &fact), expected);
    assert_eq!(fact.text, expected);

    let end = start + expected.len();
    let end_fact = fact_at(snapshot, file, end).expect("type annotation cursor fact at end");
    assert_eq!(end_fact, fact);
}

#[test]
fn source_type_annotation_cursor_fact_covers_declaration_and_body_annotations() {
    let source = "\
module a

enum Status
    active
    archived

resource Book
    required title: string
    required status: Status
    tags(pos: int): string

store ^books(id: int): Book

fn update(id: Id(^books), next: Status): Status
    const current: Status = next
    var seen(k: int): bool
    try
        return current
    catch err: Error
        return Status::archived

evolve
    transform Book.status
        const fallback: Status = Status::active
        return fallback
";
    let (snapshot, file) = analyze("source-type-annotation-cursor", source);
    support::assert_clean(&snapshot.report);

    for (line, prefix, expected) in [
        ("required title: string", "required title: ", "string"),
        ("required status: Status", "required status: ", "Status"),
        ("tags(pos: int): string", "tags(pos: ", "int"),
        ("tags(pos: int): string", "tags(pos: int): ", "string"),
        ("store ^books(id: int): Book", "store ^books(id: ", "int"),
        ("id: Id(^books)", "id: ", "Id(^books)"),
        ("next: Status", "next: ", "Status"),
        ("): Status", "): ", "Status"),
        ("const current: Status", "const current: ", "Status"),
        ("var seen(k: int): bool", "var seen(k: ", "int"),
        ("var seen(k: int): bool", "var seen(k: int): ", "bool"),
        ("catch err: Error", "catch err: ", "Error"),
    ] {
        let start = offset(source, line) + prefix.len();
        assert_type_fact_at(source, &snapshot, &file, start, expected);
    }

    let transform_status = offset(source, "const fallback: Status") + "const fallback: ".len();
    let fact =
        fact_at(&snapshot, &file, transform_status).expect("transform body type annotation fact");
    assert_eq!(span_text(source, &fact), "Status");
    assert_eq!(fact.text, "Status");

    let outside = offset(source, "return current") + "return ".len();
    assert_eq!(fact_at(&snapshot, &file, outside), None);
}

#[test]
fn source_type_annotation_cursor_fact_covers_unresolved_types_in_broken_buffers() {
    let source = "\
module a

pub fn run(value: MissingType): MissingType
    return value
";
    let (snapshot, file) = analyze("source-type-annotation-cursor-unresolved", source);

    let param = fact_at(&snapshot, &file, offset(source, "MissingType"))
        .expect("unresolved param type fact");
    assert_eq!(span_text(source, &param), "MissingType");
    assert_eq!(param.text, "MissingType");

    let return_type =
        fact_at(&snapshot, &file, offset(source, "MissingType\n")).expect("unresolved return fact");
    assert_eq!(span_text(source, &return_type), "MissingType");
    assert_eq!(return_type.text, "MissingType");
}
