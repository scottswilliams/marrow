use std::collections::HashMap;

use crate::support;
use marrow_check::tooling::{
    DocumentSymbolKind, SourceSymbolKind, document_symbols, source_symbols, source_symbols_matching,
};
use marrow_syntax::{SourceSpan, parse_source};

#[test]
fn source_symbols_report_checked_kinds_locations_and_owners() {
    let source = "\
module a

const LIMIT: int = 10

enum Status
    active

resource Book
    required title: string

store ^books(id: int): Book
    index byTitle(title, id)

pub fn add(title: string): Id(^books)
    return nextId(^books)
";
    let (snapshot, paths) = support::analyze_overlay("source-symbols", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);
    let path = paths.into_iter().next().expect("source path");

    let symbols = source_symbols(&snapshot);
    let by_name: HashMap<&str, Vec<&marrow_check::tooling::SourceSymbol>> =
        symbols.iter().fold(HashMap::new(), |mut by_name, symbol| {
            by_name
                .entry(symbol.name.as_str())
                .or_default()
                .push(symbol);
            by_name
        });

    assert_symbol(
        only(&by_name, "LIMIT"),
        SourceSymbolKind::Constant,
        Some("a"),
        &path,
        "const LIMIT",
        source,
    );
    assert_symbol(
        only(&by_name, "add"),
        SourceSymbolKind::Function,
        Some("a"),
        &path,
        "pub fn add",
        source,
    );
    assert_symbol(
        only(&by_name, "Book"),
        SourceSymbolKind::Resource,
        Some("a"),
        &path,
        "Book",
        source,
    );
    assert_symbol(
        only(&by_name, "^books"),
        SourceSymbolKind::Store,
        Some("a"),
        &path,
        "^books",
        source,
    );
    assert_symbol(
        only(&by_name, "byTitle"),
        SourceSymbolKind::StoreIndex,
        Some("a::^books"),
        &path,
        "byTitle",
        source,
    );
    assert_symbol(
        only(&by_name, "title"),
        SourceSymbolKind::ResourceMember,
        Some("a::Book"),
        &path,
        "title",
        source,
    );
    assert_symbol(
        only(&by_name, "Status"),
        SourceSymbolKind::Enum,
        Some("a"),
        &path,
        "Status",
        source,
    );
    assert_symbol(
        only(&by_name, "active"),
        SourceSymbolKind::EnumMember,
        Some("a::Status"),
        &path,
        "active",
        source,
    );
}

#[test]
fn source_symbols_include_best_effort_functions_with_invalid_signature_types() {
    let source = "\
module a

fn f(a: Booook): Alsobad
    return a
";
    let (snapshot, paths) =
        support::analyze_overlay("source-symbols-bad-function", &[("src/a.mw", source)]);
    assert!(
        snapshot.report.has_errors(),
        "unknown signature types should produce diagnostics"
    );
    let path = paths.into_iter().next().expect("source path");

    let symbol = source_symbols(&snapshot)
        .into_iter()
        .find(|symbol| symbol.name == "f")
        .expect("best-effort function symbol remains visible");

    assert_symbol(
        &symbol,
        SourceSymbolKind::Function,
        Some("a"),
        &path,
        "fn f",
        source,
    );
}

#[test]
fn source_symbols_matching_empty_search_returns_all_symbols() {
    let source = "\
module a

const LIMIT: int = 10

resource Book
    required title: string

store ^books(id: int): Book
";
    let (snapshot, _) =
        support::analyze_overlay("source-symbols-empty-search", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(
        source_symbols_matching(&snapshot, " \t\n"),
        source_symbols(&snapshot)
    );
}

#[test]
fn source_symbols_matching_name_is_case_insensitive() {
    let source = "\
module a

const LIMIT: int = 10

resource Book
    required title: string
";
    let (snapshot, _) =
        support::analyze_overlay("source-symbols-case-search", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(
        symbol_names(source_symbols_matching(&snapshot, "limit")),
        ["LIMIT"]
    );
}

#[test]
fn source_symbols_matching_uses_container_qualified_paths() {
    let source = "\
module a

resource Book
    required title: string
";
    let (snapshot, _) =
        support::analyze_overlay("source-symbols-qualified-search", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(
        symbol_names(source_symbols_matching(&snapshot, "a::Book")),
        ["Book", "title"]
    );
}

#[test]
fn source_symbols_matching_ranks_name_matches_before_qualified_matches() {
    let source = "\
module a

const bookValue: int = 1

resource Book
    required title: string
";
    let (snapshot, _) =
        support::analyze_overlay("source-symbols-rank-search", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(
        symbol_names(source_symbols_matching(&snapshot, "book")),
        ["Book", "bookValue", "title"]
    );
    let symbols = source_symbols_matching(&snapshot, "book");
    assert_eq!(symbols[0].container.as_deref(), Some("a"));
    assert_eq!(symbols[1].container.as_deref(), Some("a"));
    assert_eq!(symbols[2].container.as_deref(), Some("a::Book"));
}

#[test]
fn source_symbols_matching_preserves_source_order_within_equal_ranks() {
    let source = "\
module a

const apple: int = 1
const apply: int = 2
";
    let (snapshot, _) =
        support::analyze_overlay("source-symbols-tie-search", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(
        symbol_names(source_symbols_matching(&snapshot, "app")),
        ["apple", "apply"]
    );
}

#[test]
fn source_symbols_matching_finds_store_roots_without_the_caret() {
    let source = "\
module a

resource Book
    required title: string

store ^books(id: int): Book
";
    let (snapshot, _) =
        support::analyze_overlay("source-symbols-store-root-search", &[("src/a.mw", source)]);
    support::assert_clean(&snapshot.report);

    assert_eq!(
        symbol_names(source_symbols_matching(&snapshot, "books")),
        ["^books"]
    );
}

#[test]
fn document_symbols_report_parsed_outline_facts() {
    let source = "\
module shelf

const LIMIT: int = 10

enum Status
    active
    archived

resource Book
    required title: string
    notes(noteId: string)
        text: string

store ^books(id: int): Book
    index byTitle(title, id)

surface Books from ^books
    fields title

evolve
    rename Book.title -> Book.name
    default Book.name = \"untitled\"
    retire Book.title
    transform ^books
        const id: Id(^books) = 1

pub fn add(title: string): Id(^books)
    return nextId(^books)
";
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

    let symbols = document_symbols(&parsed.file, source);

    assert_document_symbol(
        find_document_symbol(&symbols, "LIMIT"),
        DocumentSymbolKind::Constant,
        Some("int"),
        "const LIMIT",
        "LIMIT",
        source,
    );
    assert_document_symbol(
        find_document_symbol(&symbols, "add"),
        DocumentSymbolKind::Function,
        Some("(title: string): Id(^books)"),
        "pub fn add",
        "add",
        source,
    );
    assert_document_symbol(
        find_document_symbol(&symbols, "Status"),
        DocumentSymbolKind::Enum,
        None,
        "enum Status",
        "Status",
        source,
    );
    assert_document_symbol(
        find_child(find_document_symbol(&symbols, "Status"), "active"),
        DocumentSymbolKind::EnumMember,
        None,
        "    active",
        "active",
        source,
    );
    let book = find_document_symbol(&symbols, "Book");
    assert_document_symbol(
        book,
        DocumentSymbolKind::Resource,
        None,
        "resource Book",
        "Book",
        source,
    );
    assert_document_symbol(
        find_child(book, "title"),
        DocumentSymbolKind::ResourceField,
        Some("string"),
        "    required title",
        "title",
        source,
    );
    let notes = find_child(book, "notes");
    assert_document_symbol(
        notes,
        DocumentSymbolKind::ResourceGroup,
        None,
        "    notes(noteId: string)",
        "notes",
        source,
    );
    assert_document_symbol(
        find_child(notes, "text"),
        DocumentSymbolKind::ResourceField,
        Some("string"),
        "        text: string",
        "text",
        source,
    );
    let store = find_document_symbol(&symbols, "^books");
    assert_document_symbol(
        store,
        DocumentSymbolKind::Store,
        Some("Book"),
        "store ^books",
        "^books",
        source,
    );
    assert_document_symbol(
        find_child(store, "byTitle"),
        DocumentSymbolKind::StoreIndex,
        Some("index(title, id)"),
        "    index byTitle",
        "byTitle",
        source,
    );
    assert_document_symbol(
        find_document_symbol(&symbols, "Books"),
        DocumentSymbolKind::Surface,
        Some("^books"),
        "surface Books",
        "Books",
        source,
    );
    let evolve = find_document_symbol(&symbols, "evolve");
    assert_document_symbol(
        evolve,
        DocumentSymbolKind::Evolve,
        None,
        "evolve",
        "evolve",
        source,
    );
    for (name, starts_with) in [
        ("rename", "    rename Book.title"),
        ("default", "    default Book.name"),
        ("retire", "    retire Book.title"),
        ("transform", "    transform ^books"),
    ] {
        assert_document_symbol(
            find_child(evolve, name),
            DocumentSymbolKind::EvolveStep,
            None,
            starts_with,
            name,
            source,
        );
    }
}

#[test]
fn document_symbols_preserve_parsed_outline_for_broken_source() {
    let source = "\
resource Book
    required title: string

fn broken(
";
    let parsed = parse_source(source);
    assert!(
        !parsed.diagnostics.is_empty(),
        "source should have parse diagnostics"
    );

    let symbols = document_symbols(&parsed.file, source);
    let book = find_document_symbol(&symbols, "Book");
    assert_document_symbol(
        book,
        DocumentSymbolKind::Resource,
        None,
        "resource Book",
        "Book",
        source,
    );
    assert_document_symbol(
        find_child(book, "title"),
        DocumentSymbolKind::ResourceField,
        Some("string"),
        "    required title",
        "title",
        source,
    );
}

#[test]
fn document_symbols_do_not_panic_on_mismatched_source_text() {
    let source = "\
resource Book
    title: string
";
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

    let symbols = document_symbols(&parsed.file, "");

    let book = find_document_symbol(&symbols, "Book");
    assert_eq!(book.kind, DocumentSymbolKind::Resource);
    assert_eq!(book.selection_span, book.span);

    let title = find_child(book, "title");
    assert_eq!(title.kind, DocumentSymbolKind::ResourceField);
    assert_eq!(title.selection_span, title.span);
}

fn only<'a>(
    by_name: &'a HashMap<&str, Vec<&'a marrow_check::tooling::SourceSymbol>>,
    name: &str,
) -> &'a marrow_check::tooling::SourceSymbol {
    let symbols = by_name
        .get(name)
        .unwrap_or_else(|| panic!("missing symbol `{name}`"));
    assert_eq!(
        symbols.len(),
        1,
        "expected one `{name}` symbol: {symbols:#?}"
    );
    symbols[0]
}

fn symbol_names(symbols: Vec<marrow_check::tooling::SourceSymbol>) -> Vec<String> {
    symbols.into_iter().map(|symbol| symbol.name).collect()
}

fn assert_symbol(
    symbol: &marrow_check::tooling::SourceSymbol,
    kind: SourceSymbolKind,
    container: Option<&str>,
    path: &std::path::Path,
    span_text: &str,
    source: &str,
) {
    assert_eq!(symbol.kind, kind, "{symbol:#?}");
    assert_eq!(symbol.container.as_deref(), container, "{symbol:#?}");
    assert_eq!(symbol.file, path, "{symbol:#?}");
    assert_span_text(symbol.span, span_text, source);
}

fn assert_span_text(span: SourceSpan, expected: &str, source: &str) {
    assert_eq!(
        &source[span.start_byte..span.start_byte + expected.len()],
        expected,
        "{span:?}"
    );
}

fn find_document_symbol<'a>(
    symbols: &'a [marrow_check::tooling::DocumentSymbol],
    name: &str,
) -> &'a marrow_check::tooling::DocumentSymbol {
    symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("missing document symbol `{name}` in {symbols:#?}"))
}

fn find_child<'a>(
    symbol: &'a marrow_check::tooling::DocumentSymbol,
    name: &str,
) -> &'a marrow_check::tooling::DocumentSymbol {
    find_document_symbol(&symbol.children, name)
}

fn assert_document_symbol(
    symbol: &marrow_check::tooling::DocumentSymbol,
    kind: DocumentSymbolKind,
    detail: Option<&str>,
    span_starts_with: &str,
    selection_text: &str,
    source: &str,
) {
    assert_eq!(symbol.kind, kind, "{symbol:#?}");
    assert_eq!(symbol.detail.as_deref(), detail, "{symbol:#?}");
    assert_span_text(symbol.span, span_starts_with, source);
    assert_eq!(
        &source[symbol.selection_span.start_byte..symbol.selection_span.end_byte],
        selection_text,
        "{symbol:#?}"
    );
}
