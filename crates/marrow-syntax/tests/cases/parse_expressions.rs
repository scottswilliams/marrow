//! Expression operators and values: literal kinds, operator precedence and
//! associativity, unary and grouping, the absence and `is` operators, and the
//! spans an expression carries back into source.

use crate::common;
use common::{has_reason, parse_reason};
use marrow_syntax::{
    BinaryOp, Declaration, ExpectedSyntax, Expression, LiteralKind, ParseDiagnosticReason,
    Statement, UnaryOp, parse_source,
};

#[derive(Debug)]
enum Expectation<'a> {
    Literal(LiteralKind, &'a str),
    Name(&'a [&'a str]),
}

fn parsed_return_expr(source: &str) -> Expression {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected {source:?} to parse cleanly: {:#?}",
        parsed.diagnostics
    );
    let Statement::Return {
        value: Some(value), ..
    } = &parsed.file.function("f").expect("f").body.statements[0]
    else {
        panic!("expected a return expression in {source:?}");
    };
    value.clone()
}

#[test]
fn parses_const_values_into_expression_nodes() {
    let cases: &[(&str, Expectation<'_>)] = &[
        (
            "const Max: int = 5\n",
            Expectation::Literal(LiteralKind::Integer, "5"),
        ),
        (
            "const Pi: decimal = 3.14\n",
            Expectation::Literal(LiteralKind::Decimal, "3.14"),
        ),
        (
            "const Window: duration = 2.hours\n",
            Expectation::Literal(LiteralKind::Duration, "2.hours"),
        ),
        (
            "const Greeting: string = \"hi\"\n",
            Expectation::Literal(LiteralKind::String, "\"hi\""),
        ),
        (
            "const Marker: bytes = b\"mw\"\n",
            Expectation::Literal(LiteralKind::Bytes, "b\"mw\""),
        ),
        (
            "const Active: bool = true\n",
            Expectation::Literal(LiteralKind::Bool, "true"),
        ),
        (
            "const Default = SomeName\n",
            Expectation::Name(&["SomeName"]),
        ),
        (
            "const Pi2: decimal = std::math::PI\n",
            Expectation::Name(&["std", "math", "PI"]),
        ),
    ];

    for (source, expected) in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.is_empty(),
            "expected {source:?} to parse cleanly: {:#?}",
            parsed.diagnostics
        );
        let Declaration::Const(decl) = &parsed.file.declarations[0] else {
            panic!("expected const declaration in {source:?}");
        };
        match (expected, &decl.value) {
            (
                Expectation::Literal(expected_kind, expected_text),
                Some(Expression::Literal { kind, text, .. }),
            ) => {
                assert_eq!(*kind, *expected_kind, "{source:?}");
                assert_eq!(text, expected_text, "{source:?}");
            }
            (Expectation::Name(expected_segments), Some(Expression::Name { segments, .. })) => {
                assert_eq!(segments.as_slice(), *expected_segments, "{source:?}");
            }
            (expected, actual) => panic!("expected {expected:?} for {source:?}, got {actual:?}"),
        }
    }
}

#[test]
fn parses_const_operator_expressions_with_precedence() {
    // 60 * 60 + 1 parses as (60 * 60) + 1.
    let parsed = parse_source("const Total: int = 60 * 60 + 1\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Binary {
        op, left, right, ..
    }) = &decl.value
    else {
        panic!("expected binary expression, got {:?}", decl.value);
    };
    assert_eq!(*op, BinaryOp::Add);
    assert!(
        matches!(
            left.as_ref(),
            Expression::Binary {
                op: BinaryOp::Multiply,
                ..
            }
        ),
        "left should be the multiply, got {left:?}"
    );
    assert!(
        matches!(right.as_ref(), Expression::Literal { text, .. } if text == "1"),
        "right should be literal 1, got {right:?}"
    );
}

#[test]
fn parses_const_unary_and_grouping() {
    let parsed = parse_source("const Adjusted: int = -(1 + 2)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Unary { op, operand, .. }) = &decl.value else {
        panic!("expected unary expression, got {:?}", decl.value);
    };
    assert_eq!(*op, UnaryOp::Neg);
    // Parentheses are unwrapped: the operand is the inner add expression.
    assert!(
        matches!(
            operand.as_ref(),
            Expression::Binary {
                op: BinaryOp::Add,
                ..
            }
        ),
        "operand should be the inner add, got {operand:?}"
    );
}

#[test]
fn bare_type_keyword_is_not_a_value() {
    // `int` alone is a type, not an expression, so it is a syntax error in
    // value position rather than a silently accepted value.
    let parsed = parse_source("const Bad = int\n");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::KeywordExpression)
        ),
        "expected a keyword-in-value parse error: {:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        decl.value.is_none(),
        "expected bare `int` to carry no value, got {:?}",
        decl.value
    );
}

#[test]
fn const_chained_equality_is_not_associative() {
    // Equality, inequality, comparison, and `is` each sit on their own
    // non-associative level: a second same-class operator is a grammar error
    // spanned at that operator, mirroring the `??` diagnostic, rather than a
    // generic "expected a statement" at the line start.
    // Each remedy must name a rewrite that actually compiles: comparisons of
    // boolean results can be parenthesized, but a chained `is` cannot — `(a is X)`
    // is a bool, so a second `is` over it fails the enum-operand check. The `is`
    // remedy instead points at joining the subtree tests with `and`/`or`.
    for (source, operator, remedy) in [
        ("const Bad: bool = a == b == c\n", "==", "parentheses"),
        ("const Bad: bool = a != b != c\n", "!=", "parentheses"),
        ("const Bad: bool = a < b < c\n", "<", "parentheses"),
        ("const Bad: bool = a is X is Y\n", "is", "`and`/`or`"),
    ] {
        let parsed = parse_source(source);
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::NonAssociativeOperator)
            })
            .unwrap_or_else(|| {
                panic!(
                    "expected a non-associative-operator error for {source:?}: {:#?}",
                    parsed.diagnostics
                )
            });
        // The span points at the second operator, not the statement start.
        assert_eq!(
            &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
            operator,
            "span should cover the second `{operator}`: {diagnostic:#?}"
        );
        // The remedy rides in the message so it survives the checker's
        // parse-diagnostic lowering and renders in `marrow check`.
        assert!(
            diagnostic.message.contains("does not chain") && diagnostic.message.contains(remedy),
            "expected the `{remedy}` remedy in the message: {diagnostic:#?}"
        );
        let Declaration::Const(decl) = &parsed.file.declarations[0] else {
            panic!("expected const declaration for {source:?}");
        };
        assert!(
            decl.value.is_none(),
            "expected chained `{operator}` to carry no value, got {:?}",
            decl.value
        );
    }
}

#[test]
fn const_binary_expression_span_covers_whole_expression() {
    let source = "const Total: int = 60 * 60\n";
    let parsed = parse_source(source);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let span = decl.value.as_ref().expect("value").span();
    assert_eq!(&source[span.start_byte..span.end_byte], "60 * 60");
}

#[test]
fn const_expression_span_points_into_source() {
    let source = "const Max: int = 5\n";
    let parsed = parse_source(source);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let span = decl.value.as_ref().expect("value").span();
    assert_eq!(&source[span.start_byte..span.end_byte], "5");
    assert_eq!(span.line, 1);
    assert_eq!(span.column, 18);
}

#[test]
fn empty_const_value_reports_the_single_generic_diagnostic() {
    // With no inner diagnostic drained (the value is truly empty), the generic
    // fallback is the only diagnostic: a const with `=` and nothing after it
    // reports once that it requires a value.
    let parsed = parse_source("const Bad = \n");
    assert_eq!(
        parsed.diagnostics.len(),
        1,
        "an empty const value should report exactly once: {:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed.diagnostics[0].reason == parse_reason(ParseDiagnosticReason::ConstRequiresValue),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

#[test]
fn equality_and_inequality_parse_in_expression_position() {
    // `==` is equality and `!=` is inequality; both parse as binary operators.
    let value = parsed_return_expr("fn f(a: int, b: int): bool\n    return a == b\n");
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::Equal,
                ..
            }
        ),
        "expected `==` to parse as equality: {value:?}"
    );

    let value = parsed_return_expr("fn f(x: int, y: int): bool\n    return x != y\n");
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::NotEqual,
                ..
            }
        ),
        "expected `!=` to parse as inequality: {value:?}"
    );
}

#[test]
fn absence_operators_parse_in_expression_position() {
    // `??` parses as the coalesce binary operator; `?.` parses as an optional
    // field read whose base is the preceding path.
    let value = parsed_return_expr("fn f(a: int): int\n    return ^books(a)?.pages ?? 0\n");
    // `??` binds looser than `?.`, so the whole `^books(a)?.pages` is the left
    // operand of one `??`.
    let Expression::Binary {
        op: BinaryOp::Coalesce,
        left,
        ..
    } = value
    else {
        panic!("expected `??` to parse as coalesce");
    };
    assert!(
        matches!(left.as_ref(), Expression::OptionalField { name, .. } if name == "pages"),
        "expected `?.` to parse as an optional field read: {left:?}"
    );
}

#[test]
fn coalesce_binds_tighter_than_equality() {
    // `name ?? "anon" == "anon"` groups as `(name ?? "anon") == "anon"`: the `??`
    // sits one level tighter than `==`.
    let value = parsed_return_expr(
        "fn f(a: string): bool\n    return ^names(a)?.value ?? \"anon\" == \"anon\"\n",
    );
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::Equal,
                ref left,
                ..
            } if matches!(left.as_ref(), Expression::Binary { op: BinaryOp::Coalesce, .. })
        ),
        "expected `(.. ?? ..) == ..`: {value:?}"
    );
}

#[test]
fn coalesce_binds_tighter_than_comparison_and_range_but_looser_than_additive() {
    let value = parsed_return_expr("fn f(count: int): bool\n    return count ?? 0 < 5\n");
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::Less,
                ref left,
                ..
            } if matches!(left.as_ref(), Expression::Binary { op: BinaryOp::Coalesce, .. })
        ),
        "expected `(count ?? 0) < 5`: {value:?}"
    );

    let value = parsed_return_expr("fn f(start: int, n: int): int\n    return start ?? 1..n\n");
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::RangeExclusive,
                ref left,
                ..
            } if matches!(left.as_ref(), Expression::Binary { op: BinaryOp::Coalesce, .. })
        ),
        "expected `(start ?? 1)..n`: {value:?}"
    );

    let value = parsed_return_expr("fn f(x: int, y: int): int\n    return x ?? y + 1\n");
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::Coalesce,
                ref right,
                ..
            } if matches!(right.as_ref(), Expression::Binary { op: BinaryOp::Add, .. })
        ),
        "expected `x ?? (y + 1)`: {value:?}"
    );
}

#[test]
fn chained_coalesce_is_right_associative() {
    // `??` is right-associative, so `a ?? b ?? c` parses as `a ?? (b ?? c)`: the
    // top `??` keeps the inner chain as its right operand, and the chain types
    // under the coalesce rule.
    let value = parsed_return_expr("fn f(a: int): int\n    return ^books(a)?.pages ?? 0 ?? 1\n");
    let Expression::Binary {
        op: BinaryOp::Coalesce,
        left,
        right,
        ..
    } = value
    else {
        panic!("expected `??` to parse as coalesce");
    };
    assert!(
        matches!(left.as_ref(), Expression::OptionalField { name, .. } if name == "pages"),
        "expected the left operand to be the `?.` read: {left:?}"
    );
    assert!(
        matches!(
            right.as_ref(),
            Expression::Binary {
                op: BinaryOp::Coalesce,
                ..
            }
        ),
        "expected the right operand to be the inner `0 ?? 1` chain: {right:?}"
    );
}

#[test]
fn underscore_no_longer_parses_as_string_concatenation() {
    let parsed = parse_source("const Bad = \"a\" _ \"b\"\n");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression))
        ),
        "expected an expression parse error for `_` concatenation: {:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        decl.value.is_none(),
        "expected obsolete `_` concatenation to carry no value, got {:?}",
        decl.value
    );
}

#[test]
fn bare_equals_in_expression_position_is_a_parse_error() {
    // `=` is assignment only; a `=` left over in expression position is reported
    // as the `=`-vs-`==` mistake at the `=` token, with a hint to use `==`.
    let source = "fn f(a: int, b: int)\n    if a = 2\n        return\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason == parse_reason(ParseDiagnosticReason::EqualsInExpression)
        })
        .unwrap_or_else(|| {
            panic!(
                "expected an `=`-in-expression parse error: {:#?}",
                parsed.diagnostics
            )
        });
    // The span points at the `=` token itself, on the condition line.
    assert_eq!(diagnostic.span.line, 2, "{diagnostic:#?}");
    assert_eq!(
        &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
        "="
    );
    // The `==` hint rides in the message so it survives the checker's
    // parse-diagnostic lowering and renders in `marrow check`.
    assert!(
        diagnostic.message.contains("`==` for equality"),
        "expected an `==` hint in the message: {diagnostic:#?}"
    );
}

#[test]
fn chained_compound_assignment_is_reported_at_the_second_operator() {
    // Assignment does not chain and is not an expression: a second compound-assign
    // operator reached in expression position is reported at that operator, the
    // same class of located parse error as the stray `=` recovery, rather than a
    // generic "expected a statement" mislocated at the statement keyword.
    for (line, second_operator) in [
        ("    a += b += c\n", "+="),
        ("    a += b -= c\n", "-="),
        ("    a *= b /= c\n", "/="),
    ] {
        let source = format!("module app\nfn f(a: int, b: int, c: int)\n{line}");
        let parsed = parse_source(&source);
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::CompoundAssignInExpression)
            })
            .unwrap_or_else(|| {
                panic!(
                    "expected a compound-assign-in-expression error for {source:?}: {:#?}",
                    parsed.diagnostics
                )
            });
        // The span covers the second compound operator, not the statement start.
        let start = source.rfind(second_operator).expect("second operator");
        assert_eq!(
            diagnostic.span.start_byte, start,
            "span should cover the second `{second_operator}`: {diagnostic:#?}"
        );
        assert_eq!(
            &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
            second_operator,
            "{diagnostic:#?}"
        );
        assert!(
            diagnostic.message.contains("does not chain"),
            "expected a does-not-chain message: {diagnostic:#?}"
        );
        // The generic statement fallback must not also fire.
        assert!(
            !parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.reason
                    == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement))
            }),
            "expected no `expected a statement` fallback: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn a_single_compound_assignment_still_parses_cleanly() {
    let parsed = parse_source("module app\nfn f(a: int, b: int)\n    a += b\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("function");
    assert!(
        matches!(&f.body.statements[0], Statement::CompoundAssign { .. }),
        "{:#?}",
        f.body.statements[0]
    );
}

#[test]
fn parses_the_is_operator() {
    let value =
        parsed_return_expr("module app\nfn f(pet: Cat): bool\n    return pet is Cat::tiger\n");
    let Expression::Binary { op, right, .. } = value else {
        panic!("expected a binary return, got {value:?}");
    };
    assert_eq!(op, BinaryOp::Is);
    // The right operand is the member-path `Cat::tiger`.
    let Expression::Name { segments, .. } = right.as_ref() else {
        panic!("expected a name on the right, got {right:?}");
    };
    assert_eq!(segments, &["Cat", "tiger"]);
}

#[test]
fn rejects_a_chained_is() {
    let parsed = parse_source(
        "module app\nfn f(pet: Cat): bool\n    return pet is Cat::tiger is Cat::housecat\n",
    );
    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
}

#[test]
fn a_three_segment_member_path_parses_as_one_name() {
    let value = parsed_return_expr("module app\nfn f(): Cat\n    return Cat::tiger::bengal\n");
    let Expression::Name { segments, .. } = value else {
        panic!("expected a name return, got {value:?}");
    };
    assert_eq!(segments, &["Cat", "tiger", "bengal"]);
}
