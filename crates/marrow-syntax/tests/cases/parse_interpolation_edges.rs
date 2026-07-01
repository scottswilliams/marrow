//! Interpolation edge cases pin accepted and rejected brace handling through
//! typed lexer and parser behavior, not rendered prose.

use crate::common;
use common::{has_reason, lexer_reason, parse_reason};
use marrow_syntax::{
    Declaration, DiagnosticReason, ExpectedSyntax, Expression, InterpolationPart,
    LexerDiagnosticReason, NESTING_DEPTH_LIMIT, ParseDiagnosticReason, lex_source, parse_source,
};

/// An interpolation hole may itself hold another interpolation literal, and that
/// nesting is unbounded up to the documented depth limit. The lexer scans a
/// nested `$"..."` as a full interpolation — recursing into its own holes — while
/// looking for the outer hole-closing brace, so a three-deep nest parses cleanly
/// and builds nested interpolation parts rather than being rejected as an
/// unterminated interpolation expression.
#[test]
fn deeply_nested_interpolation_parses_and_nests() {
    let source = "const Label = $\"a{$\"b{$\"c{x}d\"}e\"}f\"\n";
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);

    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let mut expr = decl.value.as_ref().expect("const value");
    for _ in 0..2 {
        let Expression::Interpolation { parts, .. } = expr else {
            panic!("expected an interpolation, got {expr:?}");
        };
        expr = parts
            .iter()
            .find_map(|part| match part {
                InterpolationPart::Expr(inner) => Some(inner),
                InterpolationPart::Text { .. } => None,
            })
            .expect("interpolation hole expression");
    }
    assert!(
        matches!(expr, Expression::Interpolation { .. }),
        "the innermost hole should hold a third interpolation: {expr:?}"
    );
}

/// Interpolation nesting is bounded by the documented depth limit, and a nest
/// past it reports the same `check.nesting_limit` finding every other over-deep
/// construct reports — not a misleading "unterminated interpolation expression".
/// The nest is well-formed (its braces and quotes all balance); only its depth is
/// the fault, so the diagnostic must name the limit, not claim the interpolation
/// never closed.
#[test]
fn over_deep_interpolation_reports_the_nesting_limit_not_unterminated() {
    let depth = NESTING_DEPTH_LIMIT + 50;
    let mut source = String::from("const Label = ");
    for _ in 0..depth {
        source.push_str("$\"{");
    }
    source.push('x');
    for _ in 0..depth {
        source.push_str("}\"");
    }
    source.push('\n');

    let lexed = lex_source(&source);
    assert!(
        has_reason(
            &lexed.diagnostics,
            parse_reason(ParseDiagnosticReason::NestingLimit),
        ),
        "an over-deep interpolation nest must report the nesting limit: {:#?}",
        lexed.diagnostics
    );
    assert!(
        !has_reason(
            &lexed.diagnostics,
            lexer_reason(LexerDiagnosticReason::UnterminatedInterpolationExpression),
        ) && !has_reason(
            &lexed.diagnostics,
            lexer_reason(LexerDiagnosticReason::UnterminatedInterpolationString),
        ),
        "a well-formed but over-deep nest must not be reported as unterminated: {:#?}",
        lexed.diagnostics
    );
}

/// An interpolation expression with no closing `}` before the string ends is a
/// lexer error: the lexer scans for the expression terminator, reaches the end
/// of the line, and reports an unterminated interpolation expression rather than
/// silently treating the rest as text.
#[test]
fn unterminated_interpolation_expression_is_a_lexer_error() {
    let lexed = lex_source("fn main()\n    print($\"book {id\")\n");
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
    let lexed = lex_source("fn main()\n    print($\"book {a{b}}\")\n");
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

/// An empty interpolation hole inside a statement reports the missing operand at
/// the hole, not as a statement-level "expected a statement" anchored on the
/// enclosing keyword. The interpolation still recovers so the rest of the
/// statement parses.
#[test]
fn empty_interpolation_hole_in_a_statement_reports_expected_expression_at_the_hole() {
    let parsed = parse_source("fn main()\n    print($\"book {}\")\n");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression)),
        ),
        "expected an `expected an expression` diagnostic: {:#?}",
        parsed.diagnostics
    );
    assert!(
        !has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement)),
        ),
        "the empty hole must not fall through to `expected a statement`: {:#?}",
        parsed.diagnostics
    );
    let hole = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression))
        })
        .expect("expected-expression diagnostic");
    assert_eq!(
        (hole.span.line, hole.span.column),
        (2, 18),
        "the gap should anchor at the `{{` hole, not the statement keyword: {:#?}",
        hole.span
    );
}

/// A hole ending on a dangling binary operator (`{a +}`) has no right operand;
/// it reports "expected an expression" at the hole rather than the statement
/// fallback.
#[test]
fn dangling_operator_interpolation_hole_reports_expected_expression() {
    let parsed = parse_source("fn main()\n    print($\"book {a +}\")\n");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression)),
        ),
        "expected an `expected an expression` diagnostic: {:#?}",
        parsed.diagnostics
    );
    assert!(
        !has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement)),
        ),
        "the dangling-operator hole must not fall through to `expected a statement`: {:#?}",
        parsed.diagnostics
    );
}

/// A hole holding a complete operand followed by trailing garbage (`{a b}`) is
/// unclosed at the stray token; it reports "expected the end of the
/// interpolation hole" there rather than bubbling a silent `None` to the
/// statement fallback, and the rest of the statement still recovers.
#[test]
fn trailing_garbage_interpolation_hole_reports_at_the_stray_token() {
    let parsed = parse_source("fn main()\n    print($\"book {a b}\")\n");
    let hole = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::InterpolationHoleEnd,
                ))
        })
        .expect("expected an interpolation-hole-end diagnostic");
    assert_eq!(
        (hole.span.line, hole.span.column),
        (2, 21),
        "the diagnostic should anchor at the stray `b`, not the statement keyword: {:#?}",
        hole.span
    );
    assert!(
        !has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement)),
        ),
        "the trailing-garbage hole must not fall through to `expected a statement`: {:#?}",
        parsed.diagnostics
    );
}

/// A well-formed interpolation with a real operand still parses without any
/// syntax diagnostic; the missing-operand recovery does not fire on a valid hole.
#[test]
fn valid_interpolation_hole_parses_without_diagnostics() {
    let parsed = parse_source("fn main()\n    print($\"book {id} here\")\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    let lexed = lex_source("fn main()\n    print($\"book {id} more\n");
    assert!(
        lexed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == lexer_reason(LexerDiagnosticReason::UnterminatedInterpolationString)),
        "expected an unterminated-interpolation-string diagnostic: {:#?}",
        lexed.diagnostics
    );
}
