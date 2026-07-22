//! The analysis snapshot projects each module file's declaration hierarchy — a pure
//! projection of the parsed AST's existing name spans and declaration ranges — and
//! distinguishes a truthful empty outline, a syntax-unavailable file, and an invalid
//! coordinate, honoring the per-file symbol count and nesting-depth bounds.

use std::sync::Arc;

use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, AnalysisSnapshot, DeclKind, DeclSymbol, Fact,
    InputRevision, MAX_DOCUMENT_SYMBOLS_PER_FILE, MAX_SYMBOL_DEPTH, QueryError, Unavailability,
    analyze, compile_with_tests,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

fn project(files: &[(&str, &str)]) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .iter()
        .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn snap(files: &[(&str, &str)]) -> Arc<AnalysisSnapshot> {
    let Ok(snapshot) = analyze(Arc::new(project(files)), InputRevision::new(1)) else {
        panic!("expected an analysis snapshot for {files:?}");
    };
    snapshot
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path).expect("canonical identity").0
}

/// The present symbol tree for a file, or a panic naming the non-present outcome.
fn present<'a>(snapshot: &'a AnalysisSnapshot, path: &str) -> &'a [DeclSymbol] {
    match snapshot.document_symbols(&identity(path)) {
        Ok(Fact::Present(symbols)) => symbols,
        Ok(Fact::Absent) => panic!("expected Present symbols for {path}, got Absent"),
        Ok(Fact::Unavailable(_)) => panic!("expected Present symbols for {path}, got Unavailable"),
        Err(_) => panic!("expected Present symbols for {path}, got a QueryError"),
    }
}

/// A `name_span` selection range must sit inside its declaration's `full_range`.
fn name_within_full(symbol: &DeclSymbol) {
    assert!(
        symbol.name_span().start_byte >= symbol.full_range().start_byte
            && symbol.name_span().end_byte <= symbol.full_range().end_byte,
        "name span must sit within the declaration range for `{}`",
        symbol.name(),
    );
    for child in symbol.children() {
        name_within_full(child);
    }
}

#[test]
fn projects_every_top_level_declaration_kind_in_source_order() {
    let source = "module app\n\n\
        alias Meters = int\n\n\
        type Age: int in 0..150\n\n\
        const LIMIT = 10\n\n\
        struct Point {\n    x: int\n    y: int\n}\n\n\
        enum Color {\n    red\n    green\n}\n\n\
        resource Book {\n    required title: string\n}\n\n\
        store ^books: Book {\n    index byTitle[title]\n}\n\n\
        pub fn area(p: Point): int {\n    return p.x\n}\n\n\
        test \"area works\" {\n    assert area(Point(x: 1, y: 2)) == 1\n}\n";
    let snapshot = snap(&[("src/app.mw", source)]);
    let symbols = present(&snapshot, "src/app.mw");

    let kinds: Vec<DeclKind> = symbols.iter().map(DeclSymbol::kind).collect();
    assert_eq!(
        kinds,
        vec![
            DeclKind::Alias,
            DeclKind::Nominal,
            DeclKind::Const,
            DeclKind::Struct,
            DeclKind::Enum,
            DeclKind::Resource,
            DeclKind::Store,
            DeclKind::Function,
            DeclKind::Test,
        ],
    );
    let names: Vec<&str> = symbols.iter().map(DeclSymbol::name).collect();
    assert_eq!(
        names,
        vec![
            "Meters",
            "Age",
            "LIMIT",
            "Point",
            "Color",
            "Book",
            "books",
            "area",
            "area works",
        ],
    );

    // Every top-level declaration but the enum is a childless leaf on this floor.
    for symbol in symbols {
        if symbol.kind() == DeclKind::Enum {
            assert_eq!(symbol.children().len(), 2, "enum keeps its two members");
        } else {
            assert!(
                symbol.children().is_empty(),
                "non-enum declaration `{}` has no member children",
                symbol.name(),
            );
        }
        name_within_full(symbol);
    }
}

#[test]
fn enum_members_nest_as_children() {
    // `category` and nested members parse (the checker rejects them as
    // `check.unsupported`), so the file is still parseable and its symbols are present.
    let source = "module app\n\n\
        enum Tree {\n\
        \x20   category live {\n\
        \x20       active\n\
        \x20       category paused {\n\
        \x20           waiting\n\
        \x20       }\n\
        \x20   }\n\
        \x20   dead\n\
        }\n";
    let snapshot = snap(&[("src/app.mw", source)]);
    let symbols = present(&snapshot, "src/app.mw");
    assert_eq!(symbols.len(), 1);
    let tree = &symbols[0];
    assert_eq!(tree.kind(), DeclKind::Enum);
    assert_eq!(tree.name(), "Tree");

    let top: Vec<&str> = tree.children().iter().map(DeclSymbol::name).collect();
    assert_eq!(top, vec!["live", "dead"]);
    for child in tree.children() {
        assert_eq!(child.kind(), DeclKind::EnumMember);
    }

    let live = &tree.children()[0];
    let live_children: Vec<&str> = live.children().iter().map(DeclSymbol::name).collect();
    assert_eq!(live_children, vec!["active", "paused"]);

    let paused = &live.children()[1];
    let paused_children: Vec<&str> = paused.children().iter().map(DeclSymbol::name).collect();
    assert_eq!(paused_children, vec!["waiting"]);
    assert!(tree.children()[1].children().is_empty(), "`dead` is a leaf");
    name_within_full(tree);
}

#[test]
fn each_module_projects_its_own_tree() {
    let files = &[
        (
            "src/a.mw",
            "module a\n\npub fn fa(): int {\n    return 0\n}\n",
        ),
        ("src/b.mw", "module b\n\nstruct Sb {\n    x: int\n}\n"),
    ];
    let snapshot = snap(files);

    let a = present(&snapshot, "src/a.mw");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].kind(), DeclKind::Function);
    assert_eq!(a[0].name(), "fa");

    let b = present(&snapshot, "src/b.mw");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].kind(), DeclKind::Struct);
    assert_eq!(b[0].name(), "Sb");
}

#[test]
fn a_broken_file_is_syntax_unavailable_while_a_sibling_stays_present() {
    let files = &[
        (
            "src/broken.mw",
            "module broken\n\npub fn g(: int {\n    return 1\n}\n",
        ),
        (
            "src/valid.mw",
            "module valid\n\npub fn h(): int {\n    return 0\n}\n",
        ),
    ];
    let snapshot = snap(files);
    assert!(matches!(
        snapshot.document_symbols(&identity("src/broken.mw")),
        Ok(Fact::Unavailable(Unavailability::Syntax)),
    ));
    let valid = present(&snapshot, "src/valid.mw");
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].name(), "h");
}

#[test]
fn an_unknown_file_is_a_query_error() {
    let snapshot = snap(&[("src/main.mw", "pub fn f(): int {\n    return 1\n}\n")]);
    assert!(matches!(
        snapshot.document_symbols(&identity("src/other.mw")),
        Err(QueryError::UnknownFile),
    ));
}

#[test]
fn a_declaration_free_module_is_present_and_empty() {
    let snapshot = snap(&[("src/main.mw", "// only a comment, no declarations\n")]);
    let symbols = present(&snapshot, "src/main.mw");
    assert!(
        symbols.is_empty(),
        "a truthful empty outline, not an absence"
    );
}

/// Flat enums, each within `MAX_VARIANTS`, that together publish more symbols than one
/// file admits — without tripping any aggregate compile bound.
fn many_symbols_source() -> String {
    let mut source = String::from("module app\n\n");
    let enums = 17;
    let members = 250;
    for e in 0..enums {
        source.push_str(&format!("enum E{e} {{\n"));
        for m in 0..members {
            source.push_str(&format!("    m{m}\n"));
        }
        source.push_str("}\n\n");
    }
    // enums + enums*members = 17 + 4250 = 4267 symbols, over MAX_DOCUMENT_SYMBOLS_PER_FILE.
    assert!((enums + enums * members) as u64 > MAX_DOCUMENT_SYMBOLS_PER_FILE);
    source
}

#[test]
fn per_file_symbol_count_overflow_refuses_the_snapshot() {
    let source = many_symbols_source();
    let failure = analyze(
        Arc::new(project(&[("src/app.mw", &source)])),
        InputRevision::new(3),
    )
    .err()
    .expect("a symbol-count overflow produces no snapshot");
    assert!(
        matches!(
            failure,
            AnalysisFailure::ResourceLimit {
                limit: AnalysisResourceLimit::DocumentSymbolCount { limit },
                ..
            } if limit == MAX_DOCUMENT_SYMBOLS_PER_FILE,
        ),
        "expected a DocumentSymbolCount refusal",
    );
}

#[test]
fn a_symbol_count_overflow_does_not_affect_compilation() {
    // The declaration-hierarchy fact is analysis-path only: a file that overflows the
    // symbol count still compiles to an image, since compilation collects no symbols.
    let source = many_symbols_source();
    assert!(
        compile_with_tests(&project(&[("src/app.mw", &source)])).is_ok(),
        "symbol-count overflow must not fail compilation",
    );
}

/// An enum whose `category` members nest past `MAX_SYMBOL_DEPTH`. Nested members parse
/// (a `check.unsupported` diagnostic), so the file is snapshot-producible but for the
/// depth refusal.
fn deeply_nested_enum_source() -> String {
    let levels = (MAX_SYMBOL_DEPTH as usize) + 4;
    let mut source = String::from("module app\n\nenum Deep {\n");
    for level in 0..levels {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str(&format!("category c{level} {{\n"));
    }
    source.push_str(&"    ".repeat(levels + 1));
    source.push_str("leaf\n");
    for level in (0..levels).rev() {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str("}\n");
    }
    source.push_str("}\n");
    source
}

#[test]
fn per_file_symbol_depth_overflow_refuses_the_snapshot() {
    let source = deeply_nested_enum_source();
    let failure = analyze(
        Arc::new(project(&[("src/app.mw", &source)])),
        InputRevision::new(5),
    )
    .err()
    .expect("a symbol-depth overflow produces no snapshot");
    assert!(
        matches!(
            failure,
            AnalysisFailure::ResourceLimit {
                limit: AnalysisResourceLimit::DocumentSymbolDepth { limit },
                ..
            } if limit == MAX_SYMBOL_DEPTH,
        ),
        "expected a DocumentSymbolDepth refusal",
    );
}
