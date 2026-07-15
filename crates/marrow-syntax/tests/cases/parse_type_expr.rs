//! The structural type node the parser owns. Every type spelling is classified
//! once here — `sequence[T]`, `Id(^root)`, the `?` suffix, and otherwise a name —
//! and downstream crates match on the node rather than re-reading the spelling.
//! These tests pin the node shape and the whitespace-free render that the
//! formatter and the durable digest depend on.

use marrow_syntax::{Declaration, IdentityTypeExpr, TypeExpr, parse_source};

/// Parse a field spelling into its structural node through the production parser.
fn field_type(spelling: &str) -> TypeExpr {
    let source = format!("resource R\n    value: {spelling}\n");
    let parsed = parse_source(&source);
    assert!(
        !parsed.has_errors(),
        "unexpected parse errors for `{spelling}`: {:#?}",
        parsed.diagnostics
    );
    let Some(Declaration::Resource(resource)) = parsed.file.declarations.first() else {
        panic!("expected a resource declaration for `{spelling}`");
    };
    match resource.members.first() {
        Some(marrow_syntax::ResourceMember::Field(field)) => field.ty.clone(),
        other => panic!("expected a field for `{spelling}`: {other:#?}"),
    }
}

#[test]
fn a_scalar_or_named_spelling_is_a_name() {
    assert!(matches!(field_type("int"), TypeExpr::Name { text, .. } if text == "int"));
    assert!(matches!(field_type("Book"), TypeExpr::Name { text, .. } if text == "Book"));
    assert!(
        matches!(field_type("shelf::Book"), TypeExpr::Name { text, .. } if text == "shelf::Book")
    );
    assert!(matches!(field_type("unknown"), TypeExpr::Name { text, .. } if text == "unknown"));
}

#[test]
fn a_bracket_bearing_name_is_a_generic_application() {
    // Any identifier head carrying a `[...]` group is a generic type application
    // (`Option[T]`, `List[T]`, or a user `struct`/`enum` template); the semantic
    // owner resolves the head, so the parser accepts an arbitrary one. A
    // paren-bearing name is not an application: it stays an unresolvable name the
    // checker reports.
    let TypeExpr::Apply { head, args, .. } = field_type("Foo[bar]") else {
        panic!("expected a generic application for `Foo[bar]`");
    };
    assert_eq!(head, "Foo");
    assert!(matches!(args.as_slice(), [TypeExpr::Name { text, .. }] if text == "bar"));
    assert!(matches!(field_type("Foo(bar)"), TypeExpr::Name { text, .. } if text == "Foo(bar)"));
}

#[test]
fn sequence_recurses_on_its_element() {
    let TypeExpr::Sequence { element, .. } = field_type("sequence[int]") else {
        panic!("expected a sequence");
    };
    assert!(matches!(*element, TypeExpr::Name { text, .. } if text == "int"));

    let TypeExpr::Sequence { element, .. } = field_type("sequence[sequence[int]]") else {
        panic!("expected a nested sequence");
    };
    assert!(matches!(*element, TypeExpr::Sequence { .. }));
}

#[test]
fn identity_records_the_root_and_its_spans() {
    let source = "resource R\n    value: Id(^books)\n";
    let parsed = parse_source(source);
    let Declaration::Resource(resource) = &parsed.file.declarations[0] else {
        panic!("expected a resource");
    };
    let marrow_syntax::ResourceMember::Field(field) = &resource.members[0] else {
        panic!("expected a field");
    };
    let TypeExpr::Identity(IdentityTypeExpr {
        root,
        keyword_span,
        caret_span,
        root_span,
        span,
    }) = &field.ty
    else {
        panic!("expected an identity");
    };
    // Every recorded span addresses exactly its part of the spelling, so tooling
    // reaches the constructor and the saved root without re-lexing.
    assert_eq!(root, "books");
    assert_eq!(
        &source[keyword_span.start_byte..keyword_span.end_byte],
        "Id"
    );
    assert_eq!(&source[caret_span.start_byte..caret_span.end_byte], "^");
    assert_eq!(&source[root_span.start_byte..root_span.end_byte], "books");
    assert_eq!(&source[span.start_byte..span.end_byte], "Id(^books)");
}

#[test]
fn a_trailing_question_wraps_the_base_as_optional() {
    let TypeExpr::Optional { inner, .. } = field_type("string?") else {
        panic!("expected an optional");
    };
    assert!(matches!(*inner, TypeExpr::Name { text, .. } if text == "string"));

    // The `?` binds outside the whole `Id(^root)`, not its root.
    let TypeExpr::Optional { inner, .. } = field_type("Id(^books)?") else {
        panic!("expected an optional identity");
    };
    assert!(matches!(*inner, TypeExpr::Identity(_)));

    // A `?` inside a sequence element rides with the element.
    let TypeExpr::Sequence { element, .. } = field_type("sequence[int?]") else {
        panic!("expected a sequence of optionals");
    };
    assert!(matches!(*element, TypeExpr::Optional { .. }));
}

#[test]
fn display_round_trips_the_whitespace_free_spelling() {
    // The formatter re-emits and the durable digest hashes this render, so a
    // parsed spelling renders back byte-identically, dropping only whitespace.
    for spelling in [
        "int",
        "sequence[int]",
        "sequence[sequence[int]]",
        "Id(^books)",
        "string?",
        "Id(^books)?",
        "sequence[int?]",
        "sequence[Id(^books)]",
        "Foo[bar]",
        "shelf::Book",
    ] {
        assert_eq!(field_type(spelling).to_string(), spelling);
    }
    // Whitespace inside a spelling is dropped in the stored render.
    assert_eq!(field_type("sequence[ int ]").to_string(), "sequence[int]");
}

/// The structurally malformed spellings the parser now rejects, each paired with
/// the exact parse-level message it yields. The string carrier used to slice these
/// into an `Identity`/`Optional` or fold them to a `Name` that the checker then
/// misreported as "not a declared enum"; as the sole owner of type grammar the
/// parser names the real problem here.
const MALFORMED: &[(&str, &str)] = &[
    (
        "Id(^a.b)",
        "the root of `Id(...)` must be a single saved-root name",
    ),
    (
        "Id(^)",
        "the root of `Id(...)` must be a single saved-root name",
    ),
    (
        "Id(^a.b.c)",
        "the root of `Id(...)` must be a single saved-root name",
    ),
    ("Id(^a)(b)", "unexpected tokens after `Id(...)`"),
    ("?", "expected a type before `?`"),
    ("int??", "an optional type is written `T?`"),
];

/// Every type-annotation position, each routing through `parse_type` with its own
/// context. `%` marks where the spelling is placed.
const POSITIONS: &[(&str, &str)] = &[
    ("resource field", "resource R\n    value: %\n"),
    ("keyed key", "resource R\n    value(k: %): int\n"),
    ("function parameter", "fn f(x: %)\n    return\n"),
    ("function return", "fn f(): %\n    return\n"),
    ("const", "const c: % = 0\n"),
    ("local var", "fn f()\n    var x: % = 0\n    return\n"),
];

#[test]
fn a_malformed_type_is_a_parse_error_in_every_position() {
    for (spelling, message) in MALFORMED {
        for (position, template) in POSITIONS {
            let source = template.replace('%', spelling);
            let parsed = parse_source(&source);
            let matched = parsed.diagnostics.iter().find(|diagnostic| {
                diagnostic.code == "parse.syntax" && diagnostic.message == *message
            });
            assert!(
                matched.is_some(),
                "`{spelling}` in {position} position must be the parse error \
                 `{message}`, got:\n{:#?}",
                parsed.diagnostics
            );
        }
    }
}

#[test]
fn a_malformed_identity_leaves_no_identity_node_for_tooling_to_read() {
    // The field fails to parse, so no `Identity` node reaches the AST — the
    // saved-root cursor scan walks the parsed type nodes and finds nothing, so no
    // spurious `^a` root fact survives a malformed `Id(^a.b)`.
    let parsed = parse_source("resource R\n    ref: Id(^a.b)\n");
    let identities = parsed
        .file
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .flat_map(|resource| &resource.members)
        .filter_map(|member| match member {
            marrow_syntax::ResourceMember::Field(field) => Some(&field.ty),
            marrow_syntax::ResourceMember::Group(_) => None,
        })
        .filter(|ty| matches!(ty, TypeExpr::Identity(_)))
        .count();
    assert_eq!(identities, 0, "{parsed:#?}");
}

#[test]
fn a_malformed_type_span_points_at_the_offending_token() {
    for (spelling, offending) in [
        ("Id(^a.b)", "."),
        ("Id(^)", ")"),
        ("Id(^a.b.c)", "."),
        ("Id(^a)(b)", "("),
        ("?", "?"),
        ("int??", "??"),
    ] {
        let source = format!("resource R\n    value: {spelling}\n");
        let parsed = parse_source(&source);
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "parse.syntax")
            .unwrap_or_else(|| panic!("expected a parse error for `{spelling}`: {parsed:#?}"));
        assert_eq!(
            &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
            offending,
            "`{spelling}` should point its diagnostic at `{offending}`"
        );
    }
}
