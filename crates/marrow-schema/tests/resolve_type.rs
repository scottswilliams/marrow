//! Type resolution tests.
//!
//! These pin the cases where the structured [`Type`] must classify a spelling
//! exactly as the old string probes did: scalar names (including the
//! `string`/`Str` bridge), the `sequence[T]` sugar (including trimming and
//! nesting), canonical `Id(^store)` identity, explicit `unknown`, and the
//! bare/qualified names that stay `Named` for the checker to resolve against the
//! project.

use marrow_schema::{ScalarType, Type};
use marrow_syntax::{SourceSpan, TypeRef};

fn resolve(text: &str) -> Type {
    Type::resolve(&TypeRef {
        text: text.to_string(),
        span: SourceSpan::default(),
    })
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
    // The source keyword is `string`; the scalar variant is historically `Str`.
    assert_eq!(resolve("string"), Type::Scalar(ScalarType::Str));
}

#[test]
fn surrounding_whitespace_is_trimmed_before_classification() {
    // The old probes trimmed the text; resolution does too, so `  int  ` is
    // still the int scalar, not an unknown name.
    assert_eq!(resolve("  int  "), Type::Scalar(ScalarType::Int));
}

#[test]
fn sequence_sugar_resolves_to_a_boxed_element_type() {
    assert_eq!(
        resolve("sequence[string]"),
        Type::Sequence(Box::new(Type::Scalar(ScalarType::Str)))
    );
    // The element spelling is trimmed, matching the old `strip`-then-`trim`.
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
fn resource_id_suffix_stays_named() {
    assert_eq!(resolve("Book::Id"), Type::Named("Book::Id".to_string()));
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
fn stored_scalar_reports_the_runtime_leaf_envelope() {
    assert_eq!(resolve("int").stored_scalar(), Some(ScalarType::Int));
    assert_eq!(resolve("string").stored_scalar(), Some(ScalarType::Str));
    assert_eq!(resolve("Status").stored_scalar(), Some(ScalarType::Int));
    assert_eq!(resolve("sequence[int]").stored_scalar(), None);
    assert_eq!(resolve("Id(^books)").stored_scalar(), None);
}
