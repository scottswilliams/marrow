use std::collections::HashMap;

use crate::support;
use marrow_check::tooling::{SourceSymbolKind, source_symbols};
use marrow_syntax::SourceSpan;

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
