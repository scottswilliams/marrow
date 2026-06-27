use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    SourceSavedRootCursorFact, SourceSavedRootCursorKind, source_saved_root_cursor_fact_at,
};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    (snapshot, paths[0].clone())
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn last_offset(source: &str, needle: &str) -> usize {
    source.rfind(needle).expect("needle is present")
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
) -> Option<SourceSavedRootCursorFact> {
    source_saved_root_cursor_fact_at(snapshot, file, offset)
}

fn span_text<'a>(source: &'a str, fact: &SourceSavedRootCursorFact) -> &'a str {
    &source[fact.span.start_byte..fact.span.end_byte]
}

fn assert_fact(
    source: &str,
    snapshot: &AnalysisSnapshot,
    file: &Path,
    offset: usize,
    kind: SourceSavedRootCursorKind,
) {
    let fact = fact_at(snapshot, file, offset).expect("saved-root cursor fact");
    assert_eq!(fact.root, "books");
    assert_eq!(fact.kind, kind);
    assert_eq!(span_text(source, &fact), "^books");
}

#[test]
fn source_saved_root_cursor_fact_covers_saved_root_source_roles() {
    let source = "\
module a

resource Book
    required title: string
    required author: string

store ^books(id: int): Book
    index byAuthor(author, id)

surface Books from ^books
    fields title
    collection ^books.byAuthor as byAuthor

pub fn title(id: Id(^books)): string
    return ^books(id).title ?? \"\"
";
    let (snapshot, file) = analyze("source-saved-root-cursor-roles", source);
    support::assert_clean(&snapshot.report);

    let store_root = offset(source, "store ^books") + "store ".len();
    assert_fact(
        source,
        &snapshot,
        &file,
        store_root,
        SourceSavedRootCursorKind::Declaration,
    );
    assert_fact(
        source,
        &snapshot,
        &file,
        store_root + "^books".len(),
        SourceSavedRootCursorKind::Declaration,
    );

    let surface_root = offset(source, "surface Books from ^books") + "surface Books from ".len();
    assert_fact(
        source,
        &snapshot,
        &file,
        surface_root + 1,
        SourceSavedRootCursorKind::SurfaceTarget,
    );

    let collection_root = offset(source, "collection ^books") + "collection ".len();
    assert_fact(
        source,
        &snapshot,
        &file,
        collection_root + 2,
        SourceSavedRootCursorKind::SurfaceTarget,
    );

    let type_root = offset(source, "Id(^books") + "Id(".len();
    assert_fact(
        source,
        &snapshot,
        &file,
        type_root,
        SourceSavedRootCursorKind::TypeAnnotation,
    );

    let expression_root = last_offset(source, "^books");
    assert_fact(
        source,
        &snapshot,
        &file,
        expression_root + 1,
        SourceSavedRootCursorKind::Expression,
    );

    assert_eq!(
        fact_at(&snapshot, &file, store_root + "^books".len() + 1),
        None
    );
}

#[test]
fn source_saved_root_cursor_fact_covers_unresolved_roots_in_broken_buffers() {
    let source = "\
module a

resource Book
    title: string

pub fn title(id: Id(^missing)): string
    return ^missing(id).title ?? \"\"
";
    let (snapshot, file) = analyze("source-saved-root-cursor-unresolved", source);

    let type_root = offset(source, "Id(^missing") + "Id(".len();
    let type_fact = fact_at(&snapshot, &file, type_root).expect("unresolved type root fact");
    assert_eq!(type_fact.root, "missing");
    assert_eq!(type_fact.kind, SourceSavedRootCursorKind::TypeAnnotation);
    assert_eq!(span_text(source, &type_fact), "^missing");

    let expression_root = last_offset(source, "^missing");
    let expression_fact =
        fact_at(&snapshot, &file, expression_root + 1).expect("unresolved expression root fact");
    assert_eq!(expression_fact.root, "missing");
    assert_eq!(expression_fact.kind, SourceSavedRootCursorKind::Expression);
    assert_eq!(span_text(source, &expression_fact), "^missing");
}

#[test]
fn source_saved_root_cursor_fact_covers_evolve_targets() {
    let source = "\
module a

resource Book
    title: string

store ^books(id: int): Book
store ^archive(id: int): Book

evolve
    rename ^books -> ^archive
    default ^books.title = ^archive(1).title ?? \"\"
    retire ^books
    transform ^books
        return old
";
    let (snapshot, file) = analyze("source-saved-root-cursor-evolve", source);

    for needle in [
        "rename ^books",
        "-> ^archive",
        "default ^books",
        "retire ^books",
        "transform ^books",
    ] {
        let root = offset(source, needle) + needle.find('^').expect("needle has saved root");
        let fact = fact_at(&snapshot, &file, root + 1).expect("evolve target root fact");
        assert_eq!(
            fact.kind,
            SourceSavedRootCursorKind::EvolutionTarget,
            "{needle}"
        );
        assert_eq!(
            span_text(source, &fact),
            &source[root..root + fact.root.len() + 1]
        );
    }

    let value_root = offset(source, "= ^archive") + "= ".len();
    let fact = fact_at(&snapshot, &file, value_root + 1).expect("default value root fact");
    assert_eq!(fact.kind, SourceSavedRootCursorKind::Expression);
    assert_eq!(span_text(source, &fact), "^archive");
}
