use std::path::{Path, PathBuf};

use crate::support;
use marrow_check::AnalysisSnapshot;
use marrow_check::tooling::{
    SourceCatalogLocationFact, source_catalog_definition_fact_at, source_catalog_reference_facts_at,
};
use marrow_syntax::SourceSpan;

fn analyze(name: &str, source: &str) -> (AnalysisSnapshot, PathBuf) {
    let (snapshot, paths) = support::analyze_overlay(name, &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    (snapshot, paths[0].clone())
}

fn analyze_files(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
    let (snapshot, paths) = support::analyze_overlay(name, files);
    support::assert_clean(&snapshot.report);
    (snapshot, paths)
}

fn definition_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    source: &str,
    needle: &str,
    cursor_delta: usize,
) -> Option<SourceCatalogLocationFact> {
    source_catalog_definition_fact_at(snapshot, file, offset(source, needle) + cursor_delta)
}

fn references_at(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    source: &str,
    needle: &str,
    cursor_delta: usize,
    include_declaration: bool,
) -> Option<Vec<SourceCatalogLocationFact>> {
    source_catalog_reference_facts_at(
        snapshot,
        file,
        offset(source, needle) + cursor_delta,
        include_declaration,
    )
}

fn offset(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle is present")
}

fn span(source: &str, start: usize, text: &str) -> SourceSpan {
    assert_eq!(&source[start..start + text.len()], text);
    let before = &source[..start];
    SourceSpan {
        start_byte: start,
        end_byte: start + text.len(),
        line: before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1,
        column: before
            .rsplit_once('\n')
            .map_or(before.len(), |(_, line)| line.len()) as u32
            + 1,
    }
}

fn fact(file: &Path, span: SourceSpan) -> SourceCatalogLocationFact {
    SourceCatalogLocationFact {
        file: file.to_path_buf(),
        span,
    }
}

fn fact_texts<'a>(
    source: &'a str,
    facts: &[SourceCatalogLocationFact],
    file: &Path,
) -> Vec<&'a str> {
    facts
        .iter()
        .filter(|fact| fact.file == file)
        .map(|fact| &source[fact.span.start_byte..fact.span.end_byte])
        .collect()
}

#[test]
fn source_catalog_definition_fact_covers_saved_data_targets() {
    let source = "\
module a

resource Book
    required title: string
    required shelf: string

store ^books(id: int): Book
    index byShelf(shelf, id)

pub fn title(id: int): string
    return ^books(id).title ?? \"\"

pub fn countShelf(): int
    return count(^books.byShelf(\"fiction\"))
";
    let (snapshot, file) = analyze("source-catalog-definition-saved-data", source);

    let root_decl = offset(source, "store ^books") + "store ".len();
    assert_eq!(
        definition_at(&snapshot, &file, source, "^books(id)", 1),
        Some(fact(&file, span(source, root_decl, "^books")))
    );

    let title_decl = offset(source, "required title") + "required ".len();
    assert_eq!(
        definition_at(&snapshot, &file, source, ".title ??", 1),
        Some(fact(&file, span(source, title_decl, "title")))
    );

    let index_decl = offset(source, "index byShelf") + "index ".len();
    assert_eq!(
        definition_at(&snapshot, &file, source, ".byShelf", 1),
        Some(fact(&file, span(source, index_decl, "byShelf")))
    );
}

#[test]
fn source_catalog_definition_fact_covers_enum_member_literals() {
    let status = "\
module shelf::status

pub enum Status
    active
    archived
";
    let app = "\
module app
use shelf::status

fn current(): status::Status
    return status::Status::active
";
    let (snapshot, paths) = analyze_files(
        "source-catalog-definition-enum-member",
        &[("src/shelf/status.mw", status), ("src/app.mw", app)],
    );
    let status_file = &paths[0];
    let app_file = &paths[1];
    let member = offset(app, "status::Status::active") + "status::Status::".len();
    let declaration = offset(status, "active");

    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, app_file, member + 1),
        Some(fact(status_file, span(status, declaration, "active")))
    );
}

#[test]
fn source_catalog_definition_fact_covers_resource_constructors() {
    let library = "\
module shelf::library

resource Book
    required title: string

store ^library_books(id: int): Book
";
    let app = "\
module shelf::app
use shelf::library

resource Book
    required subtitle: string

fn imported()
    const book = library::Book(title: \"Dune\")

fn local()
    const book = Book(subtitle: \"Appendix\")
";
    let (snapshot, paths) = analyze_files(
        "source-catalog-definition-resource-constructor",
        &[("src/shelf/library.mw", library), ("src/shelf/app.mw", app)],
    );
    let library_file = &paths[0];
    let app_file = &paths[1];

    let imported_use = offset(app, "library::Book") + "library::".len();
    let imported_decl = offset(library, "resource Book") + "resource ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, app_file, imported_use + 1),
        Some(fact(library_file, span(library, imported_decl, "Book")))
    );

    let local_use = offset(app, "Book(subtitle");
    let local_decl = offset(app, "resource Book") + "resource ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, app_file, local_use + 1),
        Some(fact(app_file, span(app, local_decl, "Book")))
    );
}

#[test]
fn source_catalog_definition_fact_covers_resource_type_annotations() {
    let library = "\
module shelf::library

resource Book
    required title: string

store ^library_books(id: int): Book ;; Book
";
    let app = "\
module shelf::app
use shelf::library

resource Book
    required label: string

fn imported()
    const imported: library::Book = library::Book(title: \"Dune\")

fn local()
    const local: Book = Book(label: \"local\")
";
    let (snapshot, paths) = analyze_files(
        "source-catalog-definition-resource-annotation",
        &[("src/shelf/library.mw", library), ("src/shelf/app.mw", app)],
    );
    let library_file = &paths[0];
    let app_file = &paths[1];

    let imported_annotation =
        offset(app, "const imported: library::Book") + "const imported: library::".len();
    let imported_decl = offset(library, "resource Book") + "resource ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, app_file, imported_annotation + 1),
        Some(fact(library_file, span(library, imported_decl, "Book")))
    );

    let store_resource = offset(library, "store ^library_books(id: int): Book")
        + "store ^library_books(id: int): ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, library_file, store_resource + 1),
        Some(fact(library_file, span(library, imported_decl, "Book")))
    );

    let trailing_comment = offset(library, ";; Book") + ";; ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, library_file, trailing_comment + 1),
        None
    );
}

#[test]
fn source_catalog_definition_fact_keeps_duplicate_root_resource_leaves_separate() {
    let source = "\
module shelf

resource Book
    required title: string

resource Magazine
    required issue: int

store ^items(id: int): Book
store ^items(id: int): Magazine
";
    let (snapshot, paths) = support::analyze_overlay(
        "source-catalog-duplicate-root-resource-leaves",
        &[("src/a.mw", source)],
    );
    assert!(snapshot.report.has_errors());
    let file = &paths[0];

    let book_use = offset(source, "store ^items(id: int): Book") + "store ^items(id: int): ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, file, book_use + 1),
        None
    );

    let magazine_use =
        offset(source, "store ^items(id: int): Magazine") + "store ^items(id: int): ".len();
    assert_eq!(
        source_catalog_definition_fact_at(&snapshot, file, magazine_use + 1),
        None
    );
}

#[test]
fn source_catalog_reference_facts_cover_resource_type_annotations() {
    let library = "\
module shelf::library

resource Book
    required title: string

store ^library_books(id: int): Book

fn template()
    const template: Book = Book(title: \"Catalog\")
";
    let app = "\
module shelf::app
use shelf::library

resource Book
    required label: string

fn imported()
    const imported: library::Book = library::Book(title: \"Dune\")

fn local()
    const local: Book = Book(label: \"local\")
";
    let archive = "\
module shelf::archive

resource Book
    required code: string

fn archived()
    const archived: Book = Book(code: \"old\")
";
    let (snapshot, paths) = analyze_files(
        "source-catalog-references-resource-annotation",
        &[
            ("src/shelf/library.mw", library),
            ("src/shelf/app.mw", app),
            ("src/shelf/archive.mw", archive),
        ],
    );
    let library_file = &paths[0];
    let app_file = &paths[1];
    let archive_file = &paths[2];

    let refs = source_catalog_reference_facts_at(
        &snapshot,
        app_file,
        offset(app, "const imported: library::Book") + "const imported: library::".len() + 1,
        true,
    )
    .expect("resource annotation references");

    assert!(refs.contains(&fact(
        library_file,
        span(
            library,
            offset(library, "resource Book") + "resource ".len(),
            "Book",
        ),
    )));
    assert!(refs.contains(&fact(
        library_file,
        span(
            library,
            offset(library, "const template: Book") + "const template: ".len(),
            "Book"
        ),
    )));
    assert!(refs.contains(&fact(
        library_file,
        span(
            library,
            offset(library, "store ^library_books(id: int): Book")
                + "store ^library_books(id: int): ".len(),
            "Book"
        ),
    )));
    assert!(refs.contains(&fact(
        library_file,
        span(library, offset(library, "Book(title: \"Catalog\")"), "Book"),
    )));
    assert!(refs.contains(&fact(
        app_file,
        span(
            app,
            offset(app, "const imported: library::Book") + "const imported: library::".len(),
            "Book"
        ),
    )));
    assert!(refs.contains(&fact(
        app_file,
        span(
            app,
            offset(app, "library::Book(title") + "library::".len(),
            "Book"
        ),
    )));

    for excluded in [
        fact(
            app_file,
            span(
                app,
                offset(app, "resource Book") + "resource ".len(),
                "Book",
            ),
        ),
        fact(
            app_file,
            span(
                app,
                offset(app, "const local: Book") + "const local: ".len(),
                "Book",
            ),
        ),
        fact(
            app_file,
            span(app, offset(app, "Book(label: \"local\")"), "Book"),
        ),
        fact(
            archive_file,
            span(
                archive,
                offset(archive, "resource Book") + "resource ".len(),
                "Book",
            ),
        ),
        fact(
            archive_file,
            span(
                archive,
                offset(archive, "const archived: Book") + "const archived: ".len(),
                "Book",
            ),
        ),
        fact(
            archive_file,
            span(archive, offset(archive, "Book(code: \"old\")"), "Book"),
        ),
    ] {
        assert!(!refs.contains(&excluded));
    }
}

#[test]
fn source_catalog_reference_facts_from_resource_declarations_use_catalog_resource() {
    let library = "\
module shelf::library

resource Book
    required title: string

store ^library_books(id: int): Book

fn template()
    const template: Book = Book(title: \"Catalog\")
";
    let app = "\
module shelf::app
use shelf::library

resource Book
    required label: string

fn imported()
    const imported: library::Book = library::Book(title: \"Dune\")

fn local()
    const local: Book = Book(label: \"local\")
";
    let (snapshot, paths) = analyze_files(
        "source-catalog-references-resource-declaration",
        &[("src/shelf/library.mw", library), ("src/shelf/app.mw", app)],
    );
    let library_file = &paths[0];
    let app_file = &paths[1];
    let declaration = offset(library, "resource Book") + "resource ".len();

    let from_declaration =
        source_catalog_reference_facts_at(&snapshot, library_file, declaration + 1, true)
            .expect("resource declaration references");
    let from_annotation = source_catalog_reference_facts_at(
        &snapshot,
        app_file,
        offset(app, "const imported: library::Book") + "const imported: library::".len() + 1,
        true,
    )
    .expect("resource annotation references");

    assert_eq!(from_declaration, from_annotation);
}

#[test]
fn source_catalog_reference_facts_cover_resource_constructors() {
    let source = "\
module a

resource Book
    required title: string

fn first()
    const book = Book(title: \"Dune\")

fn second()
    const book = Book(title: \"Foundation\")
";
    let (snapshot, file) = analyze("source-catalog-references-resource-constructor", source);

    let with = references_at(&snapshot, &file, source, "Book(title: \"Dune\")", 1, true)
        .expect("resource constructor references");
    let without = references_at(&snapshot, &file, source, "Book(title: \"Dune\")", 1, false)
        .expect("resource constructor references");

    assert_eq!(
        fact_texts(source, &with, &file),
        vec!["Book", "Book", "Book"]
    );
    assert_eq!(fact_texts(source, &without, &file), vec!["Book", "Book"]);

    let declaration = span(
        source,
        offset(source, "resource Book") + "resource ".len(),
        "Book",
    );
    assert!(with.iter().any(|fact| fact.span == declaration));
    assert!(without.iter().all(|fact| fact.span != declaration));
}

#[test]
fn source_catalog_reference_facts_honor_include_declaration_for_saved_roots() {
    let source = "\
module a

resource Book
    required title: string

store ^books(id: int): Book

pub fn first(): string
    return ^books(1).title ?? \"\"

pub fn second(): string
    return ^books(2).title ?? \"\"
";
    let (snapshot, file) = analyze("source-catalog-references-saved-root", source);

    let with = references_at(&snapshot, &file, source, "^books(1)", 1, true)
        .expect("saved-root references");
    let without = references_at(&snapshot, &file, source, "^books(1)", 1, false)
        .expect("saved-root references");

    assert_eq!(
        fact_texts(source, &with, &file),
        vec!["^books", "^books", "^books"]
    );
    assert_eq!(
        fact_texts(source, &without, &file),
        vec!["^books", "^books"]
    );

    let declaration = span(
        source,
        offset(source, "store ^books") + "store ".len(),
        "^books",
    );
    assert!(with.iter().any(|fact| fact.span == declaration));
    assert!(without.iter().all(|fact| fact.span != declaration));
}

#[test]
fn source_catalog_reference_facts_filter_enum_type_annotations() {
    let status = "\
module shelf::status

pub enum Status
    active
    archived
";
    let app = "\
module app
use shelf::status

enum Color
    active

fn current(): status::Status
    const value: status::Status = status::Status::active
    const color: Color = Color::active
    return value
";
    let (snapshot, paths) = analyze_files(
        "source-catalog-references-enum-annotations",
        &[("src/shelf/status.mw", status), ("src/app.mw", app)],
    );
    let status_file = &paths[0];
    let app_file = &paths[1];
    let annotation = offset(app, "const value: status::Status") + "const value: status::".len();

    let refs =
        source_catalog_reference_facts_at(&snapshot, app_file, annotation + 1, true).unwrap();

    assert_eq!(fact_texts(status, &refs, status_file), vec!["Status"]);
    assert_eq!(fact_texts(app, &refs, app_file), vec!["Status", "Status"]);
    assert!(
        fact_texts(app, &refs, app_file)
            .iter()
            .all(|text| *text != "active" && *text != "Color")
    );

    let member = offset(app, "status::Status::active") + "status::Status::".len();
    let member_refs =
        source_catalog_reference_facts_at(&snapshot, app_file, member + 1, true).unwrap();
    assert_eq!(
        fact_texts(status, &member_refs, status_file),
        vec!["active"]
    );
    assert_eq!(fact_texts(app, &member_refs, app_file), vec!["active"]);
}
