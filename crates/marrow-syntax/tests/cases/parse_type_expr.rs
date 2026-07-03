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
fn a_bracket_or_paren_bearing_name_stays_a_name() {
    // Only `sequence[...]` and `Id(^root)` are recognized forms; any other name
    // that carries a group is an unresolvable name the checker reports, not a
    // structural sequence or identity.
    assert!(matches!(field_type("Foo[bar]"), TypeExpr::Name { text, .. } if text == "Foo[bar]"));
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

#[test]
fn a_doubled_question_is_rejected_not_a_nested_optional() {
    // `T??` has no second optional layer to denote; the parser rejects the
    // doubled suffix rather than building a nested optional.
    let parsed = parse_source("resource R\n    value: int??\n");
    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("an optional type is written `T?`")),
        "expected the doubled-optional guidance: {:#?}",
        parsed.diagnostics
    );
}
