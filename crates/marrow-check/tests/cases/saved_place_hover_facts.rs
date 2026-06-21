use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::tooling::{
    SavedPlaceHoverFact, SavedPlaceHoverKeyParam, saved_place_hover_fact_at,
};
use marrow_check::{AnalysisSnapshot, BindingIndex, build_binding_index};

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, BindingIndex, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths[0].clone())
}

fn analyze_files(
    name: &str,
    files: &[(&str, &str)],
) -> (AnalysisSnapshot, BindingIndex, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    let index = build_binding_index(&snapshot);
    (snapshot, index, paths)
}

fn fact_at(
    snapshot: &AnalysisSnapshot,
    index: &BindingIndex,
    file: &Path,
    offset: usize,
) -> Option<SavedPlaceHoverFact> {
    saved_place_hover_fact_at(snapshot, index, file, offset)
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn offset_after(source: &str, needle: &str, prefix: &str) -> usize {
    offset(source, needle) + prefix.len()
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .map(|line| line.len() + 1)
        .unwrap_or(1);
    (line, column)
}

fn fixture() -> &'static str {
    "\
module a

resource Book
    ;; The displayed title.
    required title: string
    ;; Notes by label.
    notes(noteId: string)
        ;; Body text.
        text: string

store ^books(id: int): Book
    ;; Books by display title.
    index byTitle(title, id) unique

pub fn title(id: Id(^books)): string
    return ^books(id).title ?? \"\"

pub fn note(id: Id(^books), noteId: string): string
    return ^books(id).notes(noteId).text ?? \"\"
"
}

#[test]
fn saved_place_hover_fact_covers_field_declaration_name() {
    let source = fixture();
    let (snapshot, index, file) = analyze("saved-place-hover-field-declaration", source);
    let offset = offset_after(source, "required title", "required ") + 1;

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset),
        Some(SavedPlaceHoverFact::Field {
            name: "title".to_string(),
            key_params: Vec::new(),
            ty: "string".to_string(),
            required: true,
            docs: vec!["The displayed title.".to_string()],
        })
    );
}

#[test]
fn saved_place_hover_fact_covers_group_declaration_name() {
    let source = fixture();
    let (snapshot, index, file) = analyze("saved-place-hover-group-declaration", source);
    let offset = offset(source, "notes(noteId") + 1;

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset),
        Some(SavedPlaceHoverFact::Layer {
            name: "notes".to_string(),
            key_params: vec![SavedPlaceHoverKeyParam {
                name: "noteId".to_string(),
                ty: "string".to_string(),
            }],
            docs: vec!["Notes by label.".to_string()],
        })
    );
}

#[test]
fn saved_place_hover_fact_covers_store_index_declaration_name() {
    let source = fixture();
    let (snapshot, index, file) = analyze("saved-place-hover-index-declaration", source);
    let offset = offset(source, "byTitle") + 1;

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset),
        Some(SavedPlaceHoverFact::Index {
            name: "byTitle".to_string(),
            args: vec!["title".to_string(), "id".to_string()],
            unique: true,
            docs: vec!["Books by display title.".to_string()],
        })
    );
}

#[test]
fn saved_place_hover_fact_covers_member_reference_leaf() {
    let source = fixture();
    let (snapshot, index, file) = analyze("saved-place-hover-member-reference", source);
    let offset = source.rfind(").title").expect("title reference") + ").".len() + 1;

    assert_eq!(
        fact_at(&snapshot, &index, &file, offset),
        Some(SavedPlaceHoverFact::Field {
            name: "title".to_string(),
            key_params: Vec::new(),
            ty: "string".to_string(),
            required: true,
            docs: vec!["The displayed title.".to_string()],
        })
    );
}

#[test]
fn saved_place_hover_fact_returns_none_for_saved_path_prefix() {
    let source = fixture();
    let (snapshot, index, file) = analyze("saved-place-hover-prefix-none", source);
    let offset = offset(source, "notes(noteId).text") + 1;

    assert_eq!(fact_at(&snapshot, &index, &file, offset), None);
}

#[test]
fn saved_place_hover_fact_uses_the_symbol_file_when_spans_match() {
    let first = "\
module a

resource Book
    title: string

store ^books(id: int): Book
";
    let second = "\
module b

resource Book
    title: int

store ^items(id: int): Book

pub fn title(id: Id(^items)): int
    return ^items(id).title ?? 0
";
    let (snapshot, index, paths) = analyze_files(
        "saved-place-hover-same-byte-span",
        &[("src/a.mw", first), ("src/b.mw", second)],
    );
    let second_file = &paths[1];

    for offset in [
        offset(second, "title: int") + 1,
        second.rfind(").title").expect("title reference") + ").".len() + 1,
    ] {
        assert_eq!(
            fact_at(&snapshot, &index, second_file, offset),
            Some(SavedPlaceHoverFact::Field {
                name: "title".to_string(),
                key_params: Vec::new(),
                ty: "int".to_string(),
                required: false,
                docs: Vec::new(),
            })
        );
    }
}

#[test]
fn saved_place_hover_fact_accepts_cross_file_references_with_definition_spans() {
    let member_use = "\
module reader
use inventory

fn read(id: Id(^items)): string
    return ^items(id).name ?? \"\"
";
    let member_use_offset = member_use.rfind(").name").expect("name reference") + ").".len();
    let declaration = format!(
        "\
module inventory

resource Item
{}
{}name: string

store ^items(id: int): Item
{}index byName(name) unique
",
        " ".repeat(28),
        " ".repeat(22),
        " ".repeat(19)
    );
    let member_decl_offset = offset(&declaration, "name: string");
    assert_eq!(member_decl_offset, member_use_offset);
    assert_eq!(
        line_column(&declaration, member_decl_offset),
        line_column(member_use, member_use_offset)
    );

    let index_decl_offset = offset(&declaration, "byName");
    let index_use_base = "\
module searcher
use inventory




fn lookup(name: string)
    if const id = ^items.byName(name)
        return
";
    let index_use_base_offset = offset(index_use_base, ".byName") + ".".len();
    assert_eq!(
        line_column(&declaration, index_decl_offset),
        line_column(index_use_base, index_use_base_offset)
    );
    let index_use = format!(
        "\
module searcher
use inventory
{}



fn lookup(name: string)
    if const id = ^items.byName(name)
        return
",
        " ".repeat(index_decl_offset - index_use_base_offset)
    );
    let index_use_offset = offset(&index_use, ".byName") + ".".len();
    assert_eq!(index_decl_offset, index_use_offset);
    assert_eq!(
        line_column(&declaration, index_decl_offset),
        line_column(&index_use, index_use_offset)
    );

    let (snapshot, index, paths) = analyze_files(
        "saved-place-hover-reference-definition-spans",
        &[
            ("src/inventory.mw", declaration.as_str()),
            ("src/reader.mw", member_use),
            ("src/searcher.mw", index_use.as_str()),
        ],
    );

    assert_eq!(
        fact_at(&snapshot, &index, &paths[1], member_use_offset + 1),
        Some(SavedPlaceHoverFact::Field {
            name: "name".to_string(),
            key_params: Vec::new(),
            ty: "string".to_string(),
            required: false,
            docs: Vec::new(),
        })
    );
    assert_eq!(
        fact_at(&snapshot, &index, &paths[2], index_use_offset + 1),
        Some(SavedPlaceHoverFact::Index {
            name: "byName".to_string(),
            args: vec!["name".to_string()],
            unique: true,
            docs: Vec::new(),
        })
    );
}

#[test]
fn saved_place_hover_fact_uses_the_binding_under_the_cursor_for_shared_resource_roots() {
    let source = "\
module a

resource Book
    title: string

store ^books(id: int): Book
store ^items(id: int): Book

fn book_title(id: Id(^books)): string
    return ^books(id).title ?? \"\"

fn item_title(id: Id(^items)): string
    return ^items(id).title ?? \"\"
";
    let (snapshot, index, file) = analyze("saved-place-hover-shared-resource-roots", source);
    let item_title_offset = source.rfind(").title").expect("item title reference") + ").".len();

    assert_eq!(
        fact_at(&snapshot, &index, &file, item_title_offset + 1),
        Some(SavedPlaceHoverFact::Field {
            name: "title".to_string(),
            key_params: Vec::new(),
            ty: "string".to_string(),
            required: false,
            docs: Vec::new(),
        })
    );
}
