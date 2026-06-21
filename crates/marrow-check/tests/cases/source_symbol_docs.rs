use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::source_symbol_docs_at;
use marrow_check::{AnalysisSnapshot, BindingIndex, build_binding_index};

fn analyze(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn docs_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<Vec<String>> {
    source_symbol_docs_at(snapshot, index, file, offset).map(|docs| docs.lines)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn offset_after(source: &str, needle: &str, prefix: &str) -> usize {
    offset(source, needle) + prefix.len()
}

#[test]
fn source_symbol_docs_follow_binding_index_definition_identity() {
    let a = "\
module a

;; A module add.
pub fn add(): int
    return 1
";
    let b = "\
module b
use a

;; B module add.
pub fn add(): int
    return 2

pub fn run(): int
    return a::add()
";
    let (snapshot, index, paths) = analyze(
        "source-symbol-docs-cross-module",
        &[("src/a.mw", a), ("src/b.mw", b)],
    );
    let b_file = &paths[1];
    let call_offset = offset_after(b, "a::add", "a::") + 1;

    assert_eq!(
        docs_at(&snapshot, &index, b_file, call_offset),
        Some(vec!["A module add.".to_string()])
    );
}

#[test]
fn source_symbol_docs_cover_declaration_symbol_kinds() {
    let source = "\
module a

;; Limit docs.
const LIMIT: int = 10

;; Book docs.
resource Book
    ;; Title docs.
    required title: string
    ;; Notes docs.
    notes(noteId: string)
        ;; Note text docs.
        text: string

store ^books(id: int): Book
    ;; Title index docs.
    index byTitle(title, id)

;; Status docs.
enum Status
    ;; Open docs.
    open
    category closed
        ;; Archived docs.
        archived
";
    let (snapshot, index, paths) =
        analyze("source-symbol-docs-declarations", &[("src/a.mw", source)]);
    let file = &paths[0];

    for (needle, add, expected) in [
        ("LIMIT: int", 1, "Limit docs."),
        ("resource Book", "resource ".len(), "Book docs."),
        ("title: string", 1, "Title docs."),
        ("notes(noteId", 1, "Notes docs."),
        ("byTitle", 1, "Title index docs."),
        ("enum Status", "enum ".len(), "Status docs."),
        ("open", 1, "Open docs."),
        ("archived", 1, "Archived docs."),
    ] {
        assert_eq!(
            docs_at(&snapshot, &index, file, offset(source, needle) + add),
            Some(vec![expected.to_string()]),
            "{needle}"
        );
    }
}

#[test]
fn source_symbol_docs_return_none_for_locals_params_and_empty_offsets() {
    let source = "\
module a

pub fn f(n: int): int
    const local = n
    return local
";
    let (snapshot, index, paths) = analyze("source-symbol-docs-excluded", &[("src/a.mw", source)]);
    let file = &paths[0];

    for offset in [
        offset_after(source, "const local", "const "),
        offset_after(source, "return local", "return "),
        offset_after(source, "n: int", ""),
        offset_after(source, "= n", "= "),
        offset(source, "module"),
    ] {
        assert_eq!(docs_at(&snapshot, &index, file, offset), None);
    }
}
