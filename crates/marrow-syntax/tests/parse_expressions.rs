//! Expression operators and values: literal kinds, operator precedence and
//! associativity, unary and grouping, the absence and `is` operators, and the
//! spans an expression carries back into source.

use marrow_syntax::{
    BinaryOp, Declaration, Expression, LiteralKind, ParseDiagnosticReason, Statement, UnaryOp,
    parse_source,
};

mod common;

use common::parse_reason;

#[derive(Debug)]
enum Expectation<'a> {
    Literal(LiteralKind, &'a str),
    Name(&'a [&'a str]),
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
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error: {:#?}",
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
    // Grammar: equality is non-associative, so `a = b = c` is not a valid
    // expression. The parser consumes `a = b` then leaves `= c`, which does not
    // fully parse and so is a syntax error rather than silently nesting.
    let parsed = parse_source("const Bad: bool = a = b = c\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error: {:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        decl.value.is_none(),
        "expected chained equality to carry no value, got {:?}",
        decl.value
    );
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
    let eq = parse_source("fn f(a: int, b: int): bool\n    return a == b\n");
    assert!(eq.diagnostics.is_empty(), "{:#?}", eq.diagnostics);
    let Statement::Return {
        value: Some(value), ..
    } = &eq.file.function("f").expect("f").body.statements[0]
    else {
        panic!("expected a return statement");
    };
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

    let ne = parse_source("fn f(x: int, y: int): bool\n    return x != y\n");
    assert!(ne.diagnostics.is_empty(), "{:#?}", ne.diagnostics);
    let Statement::Return {
        value: Some(value), ..
    } = &ne.file.function("f").expect("f").body.statements[0]
    else {
        panic!("expected a return statement");
    };
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
    let parsed = parse_source("fn f(a: int): int\n    return ^books(a)?.pages ?? 0\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Statement::Return {
        value: Some(value), ..
    } = &parsed.file.function("f").expect("f").body.statements[0]
    else {
        panic!("expected a return statement");
    };
    // `??` binds looser than `?.`, so the whole `^books(a)?.pages` is the left
    // operand of one `??`.
    let Expression::Binary {
        op: BinaryOp::Coalesce,
        left,
        ..
    } = value
    else {
        panic!("expected `??` to parse as coalesce: {value:?}");
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
    let parsed = parse_source(
        "fn f(a: string): bool\n    return ^names(a)?.value ?? \"anon\" == \"anon\"\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Statement::Return {
        value: Some(value), ..
    } = &parsed.file.function("f").expect("f").body.statements[0]
    else {
        panic!("expected a return statement");
    };
    assert!(
        matches!(
            value,
            Expression::Binary {
                op: BinaryOp::Equal,
                left,
                ..
            } if matches!(left.as_ref(), Expression::Binary { op: BinaryOp::Coalesce, .. })
        ),
        "expected `(.. ?? ..) == ..`: {value:?}"
    );
}

#[test]
fn chained_coalesce_is_not_associative() {
    // `??` is non-associative, so `a ?? b ?? c` does not parse.
    let parsed = parse_source("fn f(a: int): int\n    return ^books(a)?.pages ?? 0 ?? 1\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error for chained `??`: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn bare_equals_in_expression_position_is_a_parse_error() {
    // `=` is assignment only; a `=` left over in expression position (here nested
    // in a condition where it cannot be the statement assignment) does not parse.
    let parsed = parse_source("fn f(a: int, b: int)\n    if (a = b)\n        return\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error for a bare `=` in expression position: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_the_is_operator() {
    let parsed = parse_source("module app\nfn f(pet: Cat): bool\n    return pet is Cat::tiger\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("f");
    let Statement::Return {
        value: Some(Expression::Binary { op, right, .. }),
        ..
    } = &f.body.statements[0]
    else {
        panic!("expected a binary return, got {:?}", f.body.statements[0]);
    };
    assert_eq!(*op, BinaryOp::Is);
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
    let parsed = parse_source("module app\nfn f(): Cat\n    return Cat::tiger::bengal\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("f");
    let Statement::Return {
        value: Some(Expression::Name { segments, .. }),
        ..
    } = &f.body.statements[0]
    else {
        panic!("expected a name return, got {:?}", f.body.statements[0]);
    };
    assert_eq!(segments, &["Cat", "tiger", "bengal"]);
}
