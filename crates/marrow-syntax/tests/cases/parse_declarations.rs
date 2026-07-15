//! Top-level declaration structure: modules, imports, consts, visibility,
//! source order, and the reference sample's declaration shape.

use crate::common;
use common::{lexer_reason, parse_reason, reason_count};
use marrow_syntax::{
    Declaration, ExpectedSyntax, LexerDiagnosticReason, PARSE_SYNTAX, ParseDiagnosticReason,
    ResourceMember, parse_source,
};

/// Corpus smoke test (one owner): every fenced `mw` block that opens with
/// `module` is a complete library file and must parse without diagnostics. It
/// guards the documented examples as a whole; the per-construct parse contracts
/// are owned by the focused `parse_*` suites. Signature-only and fragment
/// examples are illustrative and excluded here; the lexer corpus covers all
/// blocks.
#[test]
fn parses_all_documented_module_files() {
    let blocks = common::documented_module_blocks();
    assert!(
        blocks.len() >= 5,
        "expected several documented module files, found {}",
        blocks.len()
    );
    for block in blocks {
        let parsed = parse_source(&block.source);
        assert!(
            parsed.diagnostics.is_empty(),
            "{}#{} should parse cleanly, got:\n{:#?}\n--- source ---\n{}",
            block.path,
            block.index,
            parsed.diagnostics,
            block.source
        );
    }
}

/// Structure smoke over the canonical `sample.md` library: it spot-checks that
/// the documented end-to-end example still parses to the expected resource,
/// store, index, and function shape. The construct-level parse contracts are
/// owned by the focused `parse_*` suites; this only guards that the reference
/// sample keeps its overall shape.
#[test]
fn parses_reference_sample_structure() {
    let parsed = parse_source(&common::reference_sample());

    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed.file.module.as_ref().map(|m| m.name.as_str()),
        Some("shelf::sample")
    );

    let book = parsed.file.resource("Book").expect("Book resource");
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(store.root.root, "books");
    assert_eq!(store.root.keys[0].name, "id");
    assert_eq!(store.root.keys[0].ty.to_string(), "int");
    assert_eq!(store.resource, "Book");

    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Field(field)
            if field.required && field.name == "title" && field.ty.to_string() == "string"
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Field(field)
            if !field.required
                && field.name == "tags"
                && field.keys.len() == 1
                && field.ty.to_string() == "string"
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Group(group)
            if group.name == "versions"
                && group.keys.len() == 1
                && group.members.iter().any(|child| matches!(
                    child,
                    ResourceMember::Field(field)
                        if field.required
                            && field.name == "changedAt"
                            && field.ty.to_string() == "instant"
                ))
    )));
    assert!(
        store
            .indexes
            .iter()
            .any(|index| index.name == "byShelf" && index.args == ["shelf", "id"] && !index.unique)
    );

    let add = parsed.file.function("add").expect("add function");
    assert!(add.public);
    assert_eq!(
        add.params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        ["title", "author", "shelf", "changedAt"]
    );
    assert_eq!(
        add.return_type.as_ref().map(ToString::to_string).as_deref(),
        Some("Id(^books)")
    );
}

#[test]
fn parses_optional_function_return_type() {
    let parsed = parse_source(
        "module app\n\
         fn f(): int?\n\
         \x20   return absent\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("function");
    assert_eq!(
        f.return_type.as_ref().map(ToString::to_string).as_deref(),
        Some("int?")
    );
}

#[test]
fn parses_optional_parameter_type() {
    // `T?` is a first-class parameter type; the trailing `?` rides in the type
    // spelling exactly as a return or local annotation does.
    let parsed = parse_source(
        "module app\n\
         fn f(value: int?): int\n\
         \x20   return 1\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("function");
    assert_eq!(f.params[0].ty.to_string(), "int?");
}

#[test]
fn rejects_a_double_optional_type() {
    // Optionality does not nest, so the `??` spelling in type position is a parse
    // error pointing at the doubled marker.
    let parsed = parse_source(
        "module app\n\
         fn f(): int??\n\
         \x20   return absent\n",
    );

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("an optional type is written `T?`")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn attaches_doc_comments_to_resource_members() {
    let parsed = parse_source(
        r#"module shelf::books

resource Book
    ;; Display title.
    required title: string
store ^books(id: int): Book
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let book = parsed.file.resource("Book").expect("Book resource");
    let ResourceMember::Field(title) = &book.members[0] else {
        panic!("expected field, got {:?}", book.members[0]);
    };
    assert_eq!(title.docs, ["Display title."]);
}

#[test]
fn parses_trailing_comments_on_declaration_lines() {
    let parsed = parse_source(
        "module app ; module comment\n\
         use std::math ; use comment\n\
         const Max: int = 5 ; const comment\n\
         resource Book ; resource comment\n\
         \x20   title: string ; field comment\n\
         \x20   notes(noteId: string) ; group comment\n\
         \x20       text: string ; nested field comment\n\
         store ^books(id: int): Book ; store comment\n\
         \x20   index byTitle(title) ; index comment\n\
         enum Status ; enum comment\n\
         \x20   active ; member comment\n\
         fn main() ; function comment\n\
         \x20   return ; statement comment\n",
    );

    assert!(
        parsed.diagnostics.is_empty(),
        "declaration trailing comments should be trivia: {:#?}",
        parsed.diagnostics
    );
    assert!(parsed.file.resource("Book").is_some());
    assert!(parsed.file.enum_decl("Status").is_some());
    assert!(parsed.file.function("main").is_some());
}

#[test]
fn merges_lexer_and_parser_diagnostics_in_source_order() {
    let parsed = parse_source(concat!(
        "module ;-bad-name\n",
        "fn main()\n",
        "    return ~~~\n",
    ));

    assert!(parsed.has_errors());
    let mut lines = parsed
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.span.line)
        .collect::<Vec<_>>();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted, "diagnostics not in source order: {lines:?}");
    lines.dedup();
    assert!(
        lines.contains(&1) && lines.contains(&3),
        "expected diagnostics on lines 1 and 3, saw {lines:?}"
    );
}

#[test]
fn rejects_internal_and_private_visibility() {
    for visibility in ["internal", "private"] {
        let parsed = parse_source(&format!("module app\n{visibility} fn main()\n    return\n"));

        assert!(parsed.has_errors(), "expected error for {visibility}");
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::InvalidVisibility)),
            "diagnostics for {visibility}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_pub_on_resource_and_store_without_cascade() {
    // `pub` gates only `fn` and `enum`; a `pub resource`/`pub store` is reported
    // once at the `pub` token with the remove-`pub` remedy in the message, then
    // recovered by parsing the rest of the declaration so its members do not raise
    // a cascade of follow-on errors.
    for (keyword, decl) in [
        ("resource", "resource Book\n    title: string"),
        ("store", "store ^books(id: int): Book"),
    ] {
        let source = format!("module app\npub {decl}\n");
        let parsed = parse_source(&source);
        let visibility: Vec<_> = parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::InvalidVisibility)
            })
            .collect();
        assert_eq!(
            visibility.len(),
            1,
            "expected exactly one visibility error for `pub {keyword}`: {:#?}",
            parsed.diagnostics
        );
        // The span points at the `pub` token, on the declaration line.
        assert_eq!(visibility[0].span.line, 2, "{:#?}", visibility[0]);
        assert_eq!(
            &source[visibility[0].span.start_byte..visibility[0].span.end_byte],
            "pub",
            "{:#?}",
            visibility[0]
        );
        // The remedy rides in the message, not `help`: the checker drops `help`
        // when it lowers parse diagnostics, so only an in-message remedy reaches
        // `marrow check`.
        assert!(
            visibility[0].message.contains("remove `pub`"),
            "expected a remove-`pub` remedy in the message: {:#?}",
            visibility[0]
        );
        // Recovery parses the declaration, so there is no field-line cascade: the
        // visibility error is the only diagnostic.
        assert_eq!(
            parsed.diagnostics.len(),
            1,
            "expected no cascade for `pub {keyword}`: {:#?}",
            parsed.diagnostics
        );
        // Recovery yields the underlying declaration, dropping only the `pub`.
        match keyword {
            "resource" => assert!(parsed.file.resource("Book").is_some(), "{:#?}", parsed.file),
            _ => assert_eq!(parsed.file.declarations.len(), 1, "{:#?}", parsed.file),
        }
    }
}

#[test]
fn requires_indented_resource_and_function_bodies() {
    let parsed = parse_source(
        r#"module app
resource Book
store ^books(id: int): Book
pub fn main()
"#,
    );

    assert_eq!(parsed.diagnostics.len(), 2, "{:#?}", parsed.diagnostics);
    assert!(parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
        == parse_reason(ParseDiagnosticReason::Expected(
            ExpectedSyntax::ResourceBody
        ))));
    assert!(parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
        == parse_reason(ParseDiagnosticReason::Expected(
            ExpectedSyntax::FunctionBody
        ))));
}

#[test]
fn rejects_const_without_value() {
    let parsed = parse_source(
        r#"module app
const MaxLoans: int
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == PARSE_SYNTAX
                && diagnostic.reason == parse_reason(ParseDiagnosticReason::ConstRequiresValue)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_module_names() {
    let parsed = parse_source("module 123\n");

    assert!(parsed.has_errors());
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::ModuleName))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn reserved_words_as_module_segments_are_rejected() {
    // A reserved word in a module path earns a precise reserved-word diagnostic at
    // the offending segment, not a generic "expected module name" at the keyword.
    for (source, word, column) in [
        ("module journal\n", "journal", 8),
        ("module app::sensitive\n", "sensitive", 13),
        ("module app::declassify\n", "declassify", 13),
        ("module app::Id\n", "Id", 13),
    ] {
        let parsed = parse_source(source);
        let segment: Vec<_> = parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::KeywordPathSegment)
            })
            .collect();
        assert_eq!(segment.len(), 1, "for {source}: {:#?}", parsed.diagnostics);
        assert!(
            segment[0].message.contains(word),
            "for {source}: {:#?}",
            segment[0]
        );
        assert_eq!(segment[0].span.line, 1, "for {source}: {:#?}", segment[0]);
        assert_eq!(
            segment[0].span.column, column,
            "for {source}: {:#?}",
            segment[0]
        );
    }
}

#[test]
fn rejects_invalid_import_names() {
    let parsed = parse_source(
        r#"module app
use *
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::ImportName))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn reserved_words_as_import_segments_are_rejected() {
    // A reserved word in an import path earns a precise reserved-word diagnostic at
    // the offending segment, on line 2 (the `use` line).
    for (source, word, column) in [
        ("module app\nuse journal\n", "journal", 5),
        ("module app\nuse pkg::sensitive\n", "sensitive", 10),
        ("module app\nuse pkg::declassify\n", "declassify", 10),
        ("module app\nuse pkg::Id\n", "Id", 10),
        ("module app\nuse std::Id\n", "Id", 10),
    ] {
        let parsed = parse_source(source);
        let segment: Vec<_> = parsed
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::KeywordPathSegment)
            })
            .collect();
        assert_eq!(segment.len(), 1, "for {source}: {:#?}", parsed.diagnostics);
        assert!(
            segment[0].message.contains(word),
            "for {source}: {:#?}",
            segment[0]
        );
        assert_eq!(segment[0].span.line, 2, "for {source}: {:#?}", segment[0]);
        assert_eq!(
            segment[0].span.column, column,
            "for {source}: {:#?}",
            segment[0]
        );
    }

    let std_bytes = parse_source("module app\nuse std::bytes\n");
    assert!(
        std_bytes.diagnostics.is_empty(),
        "std::bytes import remains valid: {:#?}",
        std_bytes.diagnostics
    );
}

#[test]
fn rejects_invalid_const_names() {
    let parsed = parse_source(
        r#"module app
const : int = 1
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::ConstName))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn reserved_word_as_const_name_is_rejected() {
    // A const name, like a parameter, member, or key name, is an `identifier`,
    // so a reserved word (`while`) in any of those positions is a parse error.
    let parsed = parse_source("module app\nconst while = 5\n");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::ConstName))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn future_surface_words_as_const_names_are_rejected() {
    for word in ["journal", "sensitive", "declassify", "Id"] {
        let parsed = parse_source(&format!("module app\nconst {word} = 5\n"));
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::ConstName))),
            "expected const-name diagnostic for {word}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn future_surface_words_as_function_names_are_rejected() {
    for word in ["journal", "sensitive", "declassify", "Id"] {
        let parsed = parse_source(&format!("module app\nfn {word}()\n    return\n"));
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::FunctionName
                ))),
            "expected function-name diagnostic for {word}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_late_or_duplicate_module_declarations() {
    let parsed = parse_source(
        r#"module app
fn main()
    return
module later
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::LateModuleDeclaration)),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn keeps_top_level_declarations_in_source_order() {
    let parsed = parse_source(
        r#"module app
const MaxLoans: int = 5
resource Book
    title: string
store ^books(id: int): Book
fn normalize(title: string): string
    return title
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let names = parsed
        .file
        .declarations
        .iter()
        .map(|decl| match decl {
            Declaration::Const(decl) => decl.name.as_str(),
            Declaration::Resource(decl) => decl.name.as_str(),
            Declaration::Store(decl) => decl.root.root.as_str(),
            Declaration::Function(decl) => decl.name.as_str(),
            Declaration::Enum(decl) => decl.name.as_str(),
            Declaration::Evolve(_) => "evolve",
            Declaration::Test(decl) => decl.name.as_str(),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["MaxLoans", "Book", "books", "normalize"]);
}

#[test]
fn rejects_tabs_because_marrow_blocks_are_space_indented() {
    let parsed = parse_source("module app\n\tpub fn main()\n");

    assert!(parsed.has_errors());
    assert_eq!(parsed.diagnostics[0].code, PARSE_SYNTAX);
    assert_eq!(parsed.diagnostics[0].span.line, 2);
    assert_eq!(parsed.diagnostics[0].span.column, 1);
    assert_eq!(
        parsed.diagnostics[0].reason,
        lexer_reason(LexerDiagnosticReason::TabIndentation)
    );
    let tab_reports = reason_count(
        &parsed.diagnostics,
        lexer_reason(LexerDiagnosticReason::TabIndentation),
    );
    assert_eq!(tab_reports, 1, "{:#?}", parsed.diagnostics);
}
