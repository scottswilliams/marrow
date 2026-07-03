//! Type resolution tests.
//!
//! These pin how the structured [`Type`] classifies a spelling: scalar names
//! (including the `string` keyword and the `ErrorCode` spelling that map to the
//! `Str` scalar), the `sequence[T]` sugar (including trimming and nesting),
//! canonical `Id(^store)` identity, explicit `unknown`, and the bare/qualified
//! names that stay `Named` for the checker to resolve against the project.

use marrow_schema::{ScalarType, Type};
use marrow_syntax::{Declaration, TypeExpr, parse_source};

/// Parse a spelling into its structural node through the production parser, so
/// resolution is exercised over exactly the node the parser builds rather than a
/// hand-made one.
fn parse_type_expr(text: &str) -> TypeExpr {
    let source = format!("const value: {text} = 0\n");
    let parsed = parse_source(&source);
    assert!(
        !parsed.has_errors(),
        "unexpected parse errors for `{text}`: {:#?}",
        parsed.diagnostics
    );
    let Some(Declaration::Const(decl)) = parsed.file.declarations.first() else {
        panic!("expected a const declaration for `{text}`");
    };
    decl.ty
        .clone()
        .unwrap_or_else(|| panic!("expected a type annotation for `{text}`"))
}

fn resolve(text: &str) -> Type {
    Type::resolve(&parse_type_expr(text))
}

#[test]
fn scalar_names_resolve_to_their_scalar() {
    assert_eq!(resolve("int"), Type::Scalar(ScalarType::Int));
    assert_eq!(resolve("decimal"), Type::Scalar(ScalarType::Decimal));
    assert_eq!(resolve("bool"), Type::Scalar(ScalarType::Bool));
    assert_eq!(resolve("bytes"), Type::Scalar(ScalarType::Bytes));
    assert_eq!(resolve("date"), Type::Scalar(ScalarType::Date));
    assert_eq!(resolve("instant"), Type::Scalar(ScalarType::Instant));
    assert_eq!(resolve("duration"), Type::Scalar(ScalarType::Duration));
    // `ErrorCode` is a recognized spelling whose storage form is a plain string.
    assert_eq!(resolve("ErrorCode"), Type::Scalar(ScalarType::Str));
}

#[test]
fn string_keyword_bridges_to_the_str_scalar() {
    assert_eq!(resolve("string"), Type::Scalar(ScalarType::Str));
}

#[test]
fn surrounding_whitespace_is_trimmed_before_classification() {
    assert_eq!(resolve("  int  "), Type::Scalar(ScalarType::Int));
}

#[test]
fn sequence_sugar_resolves_to_a_boxed_element_type() {
    assert_eq!(
        resolve("sequence[string]"),
        Type::Sequence(Box::new(Type::Scalar(ScalarType::Str)))
    );
    assert_eq!(
        resolve("sequence[ int ]"),
        Type::Sequence(Box::new(Type::Scalar(ScalarType::Int)))
    );
}

#[test]
fn nested_sequence_recurses_on_the_element() {
    assert_eq!(
        resolve("sequence[sequence[int]]"),
        Type::Sequence(Box::new(Type::Sequence(Box::new(Type::Scalar(
            ScalarType::Int
        )))))
    );
}

#[test]
fn canonical_store_id_resolves_to_an_identity() {
    assert_eq!(resolve("Id(^books)"), Type::Identity("books".to_string()));
}

#[test]
fn qualified_type_stays_named_for_the_checker() {
    assert_eq!(
        resolve("Catalog::Key"),
        Type::Named("Catalog::Key".to_string())
    );
}

#[test]
fn explicit_unknown_resolves_to_unknown() {
    assert_eq!(resolve("unknown"), Type::Unknown);
}

#[test]
fn a_bare_name_stays_named_for_the_checker_to_resolve() {
    // A bare name is a resource reference or a typo; the schema cannot tell
    // without the project's resource names, so it stays `Named`.
    assert_eq!(resolve("Book"), Type::Named("Book".to_string()));
}

#[test]
fn a_qualified_non_id_name_stays_named_verbatim() {
    // A qualified name that is not `::Id` is not an identity; it stays `Named`
    // with the full text so the checker can still see the `::` it keys on.
    assert_eq!(
        resolve("shelf::Book"),
        Type::Named("shelf::Book".to_string())
    );
}

#[test]
fn display_round_trips_the_canonical_spelling() {
    // The inverse of resolution, used in rejection messages.
    assert_eq!(resolve("int").to_string(), "int");
    assert_eq!(resolve("string").to_string(), "string");
    assert_eq!(resolve("sequence[string]").to_string(), "sequence[string]");
    assert_eq!(resolve("Id(^books)").to_string(), "Id(^books)");
    assert_eq!(resolve("unknown").to_string(), "unknown");
    assert_eq!(resolve("Book").to_string(), "Book");
}

#[test]
fn embeds_unknown_sees_through_sequences() {
    assert!(resolve("unknown").embeds_unknown());
    assert!(resolve("sequence[unknown]").embeds_unknown());
    assert!(resolve("sequence[sequence[unknown]]").embeds_unknown());
    assert!(!resolve("sequence[int]").embeds_unknown());
    assert!(!resolve("int").embeds_unknown());
}

#[test]
fn scalar_projects_only_scalar_types() {
    assert_eq!(resolve("int").scalar(), Some(ScalarType::Int));
    assert_eq!(resolve("string").scalar(), Some(ScalarType::Str));
    assert_eq!(resolve("Status").scalar(), None);
    assert_eq!(resolve("sequence[int]").scalar(), None);
    assert_eq!(resolve("Id(^books)").scalar(), None);
    // An optional is not a scalar, so it never projects a stored cell type.
    assert_eq!(resolve("int?").scalar(), None);
}

#[test]
fn trailing_question_resolves_to_an_optional_over_its_base() {
    assert_eq!(
        resolve("string?"),
        Type::Optional(Box::new(Type::Scalar(ScalarType::Str)))
    );
    assert_eq!(
        resolve("Id(^books)?"),
        Type::Optional(Box::new(Type::Identity("books".to_string())))
    );
    assert_eq!(
        resolve("Book?"),
        Type::Optional(Box::new(Type::Named("Book".to_string())))
    );
    // An optional sequence is a code type; only `?` inside a stored slot is
    // rejected, by the saved-shape validator rather than resolution.
    assert_eq!(
        resolve("sequence[string]?"),
        Type::Optional(Box::new(Type::Sequence(Box::new(Type::Scalar(
            ScalarType::Str
        )))))
    );
    // The element optional rides inside the sequence, where the validator catches
    // it for a saved element.
    assert_eq!(
        resolve("sequence[string?]"),
        Type::Sequence(Box::new(Type::Optional(Box::new(Type::Scalar(
            ScalarType::Str
        )))))
    );
}

#[test]
fn optional_display_appends_the_question_suffix() {
    assert_eq!(resolve("string?").to_string(), "string?");
    assert_eq!(resolve("Id(^books)?").to_string(), "Id(^books)?");
    assert_eq!(
        resolve("sequence[string?]").to_string(),
        "sequence[string?]"
    );
}

#[test]
fn embeds_optional_sees_through_sequences() {
    assert!(resolve("int?").embeds_optional());
    assert!(resolve("sequence[int?]").embeds_optional());
    assert!(resolve("sequence[sequence[int?]]").embeds_optional());
    assert!(!resolve("sequence[int]").embeds_optional());
    assert!(!resolve("int").embeds_optional());
}
