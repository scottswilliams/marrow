//! Interpolation edge cases pin accepted and rejected brace handling through
//! typed lexer and parser behavior, not rendered prose.

use marrow_syntax::{
    Declaration, DiagnosticReason, ExpectedSyntax, Expression, InterpolationPart,
    LexerDiagnosticReason, ParseDiagnosticReason, lex_source, parse_source,
};

mod common;

use common::{has_reason, lexer_reason, parse_reason};

/// An interpolation expression with no closing `}` before the string ends is a
/// lexer error: the lexer scans for the expression terminator, reaches the end
/// of the line, and reports an unterminated interpolation expression rather than
/// silently treating the rest as text.
#[test]
fn unterminated_interpolation_expression_is_a_lexer_error() {
    let lexed = lex_source("fn main()\n    write($\"book {id\")\n");
    assert!(
        lexed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == lexer_reason(LexerDiagnosticReason::UnterminatedInterpolationExpression)),
        "expected an unterminated-interpolation-expression diagnostic: {:#?}",
        lexed.diagnostics
    );
}

/// A `{` nested inside an interpolation expression is rejected the same way: the
/// expression scanner does not recurse into a second `{`, so the inner brace
/// terminates the scan as unterminated rather than opening a nested
/// interpolation. Interpolation expressions are ordinary expressions and never
/// contain another interpolation opener.
#[test]
fn nested_brace_inside_interpolation_expression_is_a_lexer_error() {
    let lexed = lex_source("fn main()\n    write($\"book {a{b}}\")\n");
    assert!(
        lexed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == lexer_reason(LexerDiagnosticReason::UnterminatedInterpolationExpression)),
        "expected a nested-brace interpolation to be rejected: {:#?}",
        lexed.diagnostics
    );
}

/// An empty interpolation `{}` lexes cleanly — the lexer emits an expression
/// start and end with nothing between — but holds no expression, so the parser
/// rejects it: an interpolation expression part is an ordinary expression and
/// must contain one. The lexer raises no interpolation diagnostic for the empty
/// braces; the failure surfaces as a parser "expected an expression" when the
/// empty expression part is parsed in value position.
#[test]
fn empty_interpolation_expression_is_rejected_by_the_parser() {
    let source = "const Made = $\"book {}\"\n";

    // The lexer alone accepts the empty braces: it carries no lexical
    // interpolation diagnostic, so the rejection is the parser's job.
    let lexed = lex_source(source);
    assert!(
        !lexed.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.reason,
            DiagnosticReason::Lexer(
                LexerDiagnosticReason::UnterminatedInterpolationExpression
                    | LexerDiagnosticReason::UnterminatedInterpolationString
            )
        )),
        "empty `{{}}` should lex without an interpolation diagnostic: {:#?}",
        lexed.diagnostics
    );

    let parsed = parse_source(source);
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression)),
        ),
        "expected an `expected an expression` diagnostic for `{{}}`: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn a_lone_closing_brace_is_literal_interpolation_text() {
    let parsed = parse_source("const Label = $\"book }\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Interpolation { parts, .. }) = &decl.value else {
        panic!("expected interpolation, got {:?}", decl.value);
    };
    assert!(
        matches!(&parts[..], [InterpolationPart::Text { text, .. }] if text == "book }"),
        "expected the closing brace to stay text: {parts:#?}"
    );
}

/// An unterminated interpolation *string* — text after a closed expression that
/// never reaches the closing quote — is its own typed reason, distinct from the
/// unterminated-expression case above.
#[test]
fn unterminated_interpolation_string_is_a_lexer_error() {
    let lexed = lex_source("fn main()\n    write($\"book {id} more\n");
    assert!(
        lexed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == lexer_reason(LexerDiagnosticReason::UnterminatedInterpolationString)),
        "expected an unterminated-interpolation-string diagnostic: {:#?}",
        lexed.diagnostics
    );
}
