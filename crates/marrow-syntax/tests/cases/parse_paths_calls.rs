//! Paths, calls, field access, interpolation, and call-argument rules: how the
//! parser builds postfix chains and enforces the named/positional argument order.

use crate::common;
use crate::common::CompletePayload;
use common::{has_reason, lexer_reason, parse_reason};
use marrow_syntax::{
    BinaryOp, Declaration, Diagnose, ExpectedSyntax, Expression, InterpolationPart,
    LexerDiagnosticReason, ParseDiagnosticReason, Recovery, UnsupportedSyntax, parse_source,
};

/// The value expression of a single top-level `const Name = <value>` source. The
/// source is expected to carry a parse error.
fn const_value(source: &str) -> Expression {
    let parsed = parse_source(source);
    assert!(
        parsed.has_errors(),
        "expected {source:?} to carry a parse error: {:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected a const declaration in {source:?}");
    };
    decl.value.clone().expect("const value")
}

#[test]
fn recovery_slot_is_populated_only_by_incomplete_forms() {
    // A well-formed program never populates the recovery slot or the incomplete-type
    // leaf. Every recovery node and every `TypeExpr::Incomplete` travels with a parse
    // diagnostic, so `has_errors` is the sound proxy for their absence.
    for block in common::documented_source_blocks() {
        let parsed = parse_source(&block.source);
        assert!(
            !parsed.has_errors(),
            "documented block {} must parse clean, so it populates no recovery slot: {:#?}",
            block.path,
            parsed.diagnostics
        );
    }

    // Non-vacuous: each incomplete surface form does populate the slot / leaf.
    assert!(matches!(
        const_value("const X = book.\n"),
        Expression::Error {
            recovery: Some(Recovery::Member { .. }),
            ..
        }
    ));
    assert!(matches!(
        const_value("const X = book?.\n"),
        Expression::Error {
            recovery: Some(Recovery::OptionalMember { .. }),
            ..
        }
    ));
    assert!(matches!(
        const_value("const X = Role::\n"),
        Expression::Error {
            recovery: Some(Recovery::Path { .. }),
            ..
        }
    ));
}

#[test]
fn member_dot_eof_recovers_base() {
    // `book.` with no field name: the recovery node preserves the receiver `book` so a
    // position classifier can type it, rather than collapsing the read to a bare error.
    let value = const_value("const X = book.\n");
    let Expression::Error {
        recovery: Some(Recovery::Member { base }),
        ..
    } = &value
    else {
        panic!("expected a Member recovery node, got {value:?}");
    };
    assert!(
        matches!(base.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["book"]),
        "expected the recovered base to be the receiver `book`, got {base:?}"
    );
}

#[test]
fn member_optional_dot_eof_recovers_base() {
    // `book?.` mirrors `book.` but through the short-circuiting `?.` operator; the
    // recovery variant records the operator so the classifier need not re-read source.
    let value = const_value("const X = book?.\n");
    let Expression::Error {
        recovery: Some(Recovery::OptionalMember { base }),
        ..
    } = &value
    else {
        panic!("expected an OptionalMember recovery node, got {value:?}");
    };
    assert!(
        matches!(base.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["book"]),
        "expected the recovered base to be the receiver `book`, got {base:?}"
    );
}

#[test]
fn path_colon_colon_eof_retains_segments() {
    // `Role::` with no member: the recovery node retains the name parsed so far as its
    // base so the classifier can resolve the enum path, rather than dropping segments.
    let value = const_value("const X = Role::\n");
    let Expression::Error {
        recovery: Some(Recovery::Path { base }),
        ..
    } = &value
    else {
        panic!("expected a Path recovery node, got {value:?}");
    };
    assert!(
        matches!(base.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["Role"]),
        "expected the recovered base to retain the segment `Role`, got {base:?}"
    );
}

#[test]
fn incomplete_call_empty_arg_list_recovers_call() {
    // `f(` with nothing after it: a recovered `Call` node with no arguments carries the
    // enclosing-call context editor signature help needs.
    let value = const_value("const X = getOr(\n");
    let Expression::Call { callee, args, .. } = &value else {
        panic!("expected a recovered Call node, got {value:?}");
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["getOr"]),
        "expected the callee `getOr`, got {callee:?}"
    );
    assert!(args.is_empty(), "expected no arguments, got {args:?}");
}

#[test]
fn incomplete_call_trailing_comma_recovers_call() {
    // `f(a, ` with the cursor after the comma: the recovered `Call` retains the
    // arguments parsed so far, so the active-argument position stays available.
    let value = const_value("const X = getOr(reached, \n");
    let Expression::Call { callee, args, .. } = &value else {
        panic!("expected a recovered Call node, got {value:?}");
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["getOr"]),
        "expected the callee `getOr`, got {callee:?}"
    );
    assert_eq!(
        args.len(),
        1,
        "expected the one parsed argument, got {args:?}"
    );
    assert!(
        matches!(&args[0].value, Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["reached"]),
        "expected the retained argument `reached`, got {:?}",
        args[0].value
    );
}

#[test]
fn incomplete_member_recovery_still_reports_its_diagnostic() {
    // A recovery node stays honestly broken: it always travels with its
    // Error-severity parse diagnostic, so the file still fails to compile.
    let parsed = parse_source("const X = book.\n");
    assert!(parsed.has_errors());
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression))
        ),
        "expected the missing-field-name diagnostic: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_top_level_multi_line_const_value() {
    // A column-0 `const` whose value spans several physical lines inside open
    // delimiters must parse as one call, not break apart line by line.
    let source = "const id = some::call(\n  a: 1,\n  b: 2,\n)\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "multi-line const should parse cleanly: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed.file.declarations.len(),
        1,
        "expected exactly one declaration, got {:#?}",
        parsed.file.declarations
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert_eq!(decl.name, "id");
    let Some(Expression::Call { callee, args, .. }) = &decl.value else {
        panic!("expected a call value, got {:?}", decl.value);
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        panic!("expected a name callee, got {callee:?}");
    };
    assert_eq!(crate::common::segment_texts(segments), ["some", "call"]);
    assert_eq!(args.len(), 2, "expected two arguments");
}

#[test]
fn parses_interpolation_into_text_and_expression_parts() {
    let parsed = parse_source("const Label: string = $\"book {id}: {{ready}}\"\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Interpolation { parts, .. }) = &decl.value else {
        panic!("expected interpolation, got {:?}", decl.value);
    };
    assert_eq!(parts.len(), 3, "{parts:#?}");
    assert!(
        matches!(&parts[0], InterpolationPart::Text { text, .. } if &**text == "book "),
        "part 0: {:?}",
        parts[0]
    );
    assert!(
        matches!(
            &parts[1],
            InterpolationPart::Expr(Expression::Name { segments, .. }) if crate::common::segment_texts(segments) == ["id"]
        ),
        "part 1: {:?}",
        parts[1]
    );
    // `{{ready}}` stays escaped inside literal text.
    assert!(
        matches!(&parts[2], InterpolationPart::Text { text, .. } if &**text == ": {{ready}}"),
        "part 2: {:?}",
        parts[2]
    );
}

#[test]
fn parses_interpolation_with_embedded_call_path() {
    // From the reference sample: $"{id}: {^books[id].title}".
    let parsed = parse_source("const Line: string = $\"{id}: {^books[id].title}\"\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Interpolation { parts, .. }) = &decl.value else {
        panic!("expected interpolation, got {:?}", decl.value);
    };
    let exprs = parts
        .iter()
        .filter(|part| matches!(part, InterpolationPart::Expr(_)))
        .count();
    assert_eq!(exprs, 2, "expected two embedded expressions: {parts:#?}");
    assert!(
        matches!(
            parts.last(),
            Some(InterpolationPart::Expr(Expression::Field { name, .. })) if &**name == "title"
        ),
        "last embedded expr should be a field access: {parts:#?}"
    );
}

#[test]
fn parses_calls_paths_and_field_access() {
    // `^books[id].title` is SavedRoot -> Keyed -> Field.
    let parsed = parse_source("const Title = ^books[id].title\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Field { base, name, .. }) = &decl.value else {
        panic!("expected field access, got {:?}", decl.value);
    };
    assert_eq!(&**name, "title");
    let Expression::Keyed { base, keys, .. } = base.as_ref() else {
        panic!("expected keyed access under field, got {base:?}");
    };
    assert_eq!(keys.len(), 1);
    assert!(
        matches!(base.as_ref(), Expression::SavedRoot { name, .. } if &**name == "books"),
        "expected saved root base, got {base:?}"
    );
    assert!(
        matches!(&keys[0], Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["id"]),
        "expected id key, got {:?}",
        keys[0]
    );
}

/// An enum path in value position carries its head as ordinary name segments, and
/// the head renders back to the spelling the type parser splits. The two positions
/// therefore address one enum by one spelling: a semantic owner joins the head with
/// `name_path_spelling` and splits it with `type_name_segments`, never by re-reading
/// source text of its own.
#[test]
fn an_enum_path_head_renders_back_to_its_type_spelling() {
    for (source, segments) in [
        ("const C = Color::red\n", &["Color", "red"][..]),
        (
            "const C = graphtext::Color::red\n",
            &["graphtext", "Color", "red"][..],
        ),
    ] {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.complete().is_empty(),
            "{:#?}",
            parsed.diagnostics
        );
        let Declaration::Const(decl) = &parsed.file.declarations[0] else {
            panic!("expected const declaration");
        };
        let Some(Expression::Name {
            segments: parsed_segments,
            ..
        }) = &decl.value
        else {
            panic!("expected a name path, got {:?}", decl.value);
        };
        assert_eq!(common::segment_texts(parsed_segments), segments);
        let (member, head) = parsed_segments.split_last().expect("a head and a member");
        assert_eq!(member.text(), *segments.last().expect("a member"));
        let head_spelling = marrow_syntax::name_path_spelling(head);
        assert_eq!(
            marrow_syntax::type_name_segments(&head_spelling).collect::<Vec<_>>(),
            &segments[..segments.len() - 1],
        );
    }
}

#[test]
fn absent_is_a_value_keyword_not_a_path_segment() {
    // `absent` is the empty-optional primary value, so it cannot also stand as a
    // `::` path segment.
    let parsed = parse_source("fn f() {\n    std::assert::absent(^books[1])\n}\n");
    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
}

#[test]
fn open_range_arguments_parse_in_calls() {
    let parsed = parse_source(
        "fn f(start: int, end: int) {\n    for id in ^posts.byDate[start.., ..end, ..=end] {\n        print(id)\n    }\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "open range key arguments should parse cleanly: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn quoted_field_segments_are_parse_errors() {
    let parsed = parse_source("const Old = ^books[id].\"old-title\"\n");
    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::Unsupported(
                UnsupportedSyntax::QuotedFieldSegments
            ))
        ),
        "{:#?}",
        parsed.diagnostics
    );

    // A plain identifier field is not quoted.
    let parsed = parse_source("const Title = book.title\n");
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        matches!(&decl.value, Some(Expression::Field { name, .. }) if &**name == "title"),
        "plain field should be unquoted: {:?}",
        decl.value
    );
}

#[test]
fn unterminated_quoted_field_segment_does_not_panic() {
    // The trailing `"` is an unterminated string (a lexer error). Parsing must
    // surface the diagnostic without panicking on the empty quoted segment.
    let parsed = parse_source("const Bad = a.\"\n");
    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            lexer_reason(LexerDiagnosticReason::UnterminatedString)
        ),
        "expected an unterminated-string diagnostic: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn keyword_field_name_reports_a_parse_error() {
    // A reserved word as a bare field name violates `field_name = identifier`, and is
    // reported rather than silently dropping the statement.
    let source = "fn touch(id: int) {\n    ^events[id].if = now\n}\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .complete()
        .iter()
        .find(|d| d.reason == parse_reason(ParseDiagnosticReason::KeywordFieldName))
        .unwrap_or_else(|| {
            panic!(
                "expected a keyword field-name diagnostic: {:#?}",
                parsed.diagnostics
            )
        });
    // The diagnostic points at the offending `.if`.
    assert_eq!(
        &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
        ".if"
    );
}

#[test]
fn keyword_field_name_reports_once_not_also_expected_a_statement() {
    // The specific diagnostic only: the generic "expected a statement" fallback must
    // not also fire on the same line.
    let source = "fn touch(id: int) {\n    ^events[id].if = now\n}\n";
    let parsed = parse_source(source);
    let on_offending_line: Vec<_> = parsed
        .diagnostics
        .complete()
        .iter()
        .filter(|d| d.span.line == 2)
        .collect();
    assert_eq!(
        on_offending_line.len(),
        1,
        "the keyword-field line should report exactly once: {on_offending_line:#?}"
    );
    assert!(
        on_offending_line[0].reason == parse_reason(ParseDiagnosticReason::KeywordFieldName),
        "{:#?}",
        on_offending_line[0]
    );
}

#[test]
fn quoted_keyword_field_name_reports_a_parse_error() {
    let parsed = parse_source("const Bad = ^events[id].\"if\"\n");
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::Unsupported(
                UnsupportedSyntax::QuotedFieldSegments
            ))
        ),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn const_value_keyword_field_reports_once_not_also_expected_an_expression() {
    // The const value path drains the keyword-field-name diagnostic, so the generic
    // "expected an expression" fallback must not also fire: the line reports once.
    let parsed = parse_source("const Bad = a.if\n");
    assert_eq!(
        parsed.diagnostics.complete().len(),
        1,
        "the keyword-field const value should report exactly once: {:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed.diagnostics.complete()[0].reason
            == parse_reason(ParseDiagnosticReason::KeywordFieldName),
        "{:#?}",
        parsed.diagnostics.complete()[0]
    );
}

#[test]
fn if_condition_keyword_field_reports_once_not_also_expected_an_expression() {
    // The same single-report guard applies to header expressions: an `if`
    // condition that fails on a keyword field name carries only that diagnostic.
    let parsed = parse_source("fn f() {\n    if a.if {\n        return\n    }\n}\n");
    let on_offending_line: Vec<_> = parsed
        .diagnostics
        .complete()
        .iter()
        .filter(|d| d.span.line == 2)
        .collect();
    assert_eq!(
        on_offending_line.len(),
        1,
        "the keyword-field `if` condition should report exactly once: {on_offending_line:#?}"
    );
    assert!(
        on_offending_line[0].reason == parse_reason(ParseDiagnosticReason::KeywordFieldName),
        "{:#?}",
        on_offending_line[0]
    );
}

#[test]
fn parses_named_call_arguments() {
    let parsed = parse_source("const Made = save(book: draft, total: 1)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { args, .. }) = &decl.value else {
        panic!("expected call, got {:?}", decl.value);
    };
    assert_eq!(args.len(), 2);
    assert_eq!(
        args[0].name.as_ref().map(marrow_syntax::NameSegment::text),
        Some("book")
    );
    assert_eq!(
        args[1].name.as_ref().map(marrow_syntax::NameSegment::text),
        Some("total")
    );
}

#[test]
fn removed_call_argument_modes_are_rejected() {
    for source in [
        "const Made = save(book: draft, inout total)\n",
        "const Made = save(book: draft, out result)\n",
        "const Made = normalize(inout ^books[id])\n",
    ] {
        let parsed = parse_source(source);
        assert!(parsed.has_errors(), "expected removed mode rejection");
        assert!(
            has_reason(
                parsed.diagnostics.complete(),
                parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::ParameterModes
                ))
            ),
            "{:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn out_and_inout_parse_as_ordinary_names() {
    let parsed = parse_source("const Made = save(out, inout)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { args, .. }) = &decl.value else {
        panic!("expected call, got {:?}", decl.value);
    };
    assert_eq!(args.len(), 2);
    assert!(
        matches!(&args[0].value, Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["out"])
    );
    assert!(
        matches!(&args[1].value, Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["inout"])
    );
}

#[test]
fn out_and_inout_can_head_ordinary_call_argument_expressions() {
    let parsed = parse_source("const Made = save(out(1), inout - 1)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { args, .. }) = &decl.value else {
        panic!("expected call, got {:?}", decl.value);
    };
    assert_eq!(args.len(), 2);
    assert!(matches!(&args[0].value, Expression::Call { .. }));
    assert!(matches!(
        &args[1].value,
        Expression::Binary {
            op: BinaryOp::Subtract,
            ..
        }
    ));
}

#[test]
fn positional_argument_after_named_is_rejected() {
    // After the first named argument, every remaining argument must be named.
    let source = "const Made = sub(b: 1, 2)\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .complete()
        .iter()
        .find(|d| d.reason == parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed))
        .unwrap_or_else(|| {
            panic!(
                "expected a positional-after-named diagnostic: {:#?}",
                parsed.diagnostics
            )
        });
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.kind(), "parse");
    // The diagnostic points at the offending positional argument, not the call.
    assert_eq!(
        &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
        "2"
    );
    // The rule is non-fatal: the call still parses with both arguments, so later checks
    // see the whole tree.
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { args, .. }) = &decl.value else {
        panic!("expected call, got {:?}", decl.value);
    };
    assert_eq!(args.len(), 2);
    assert_eq!(
        parsed
            .diagnostics
            .complete()
            .iter()
            .filter(|d| {
                d.reason == parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed)
            })
            .count(),
        1
    );
}

#[test]
fn positional_then_named_arguments_are_accepted() {
    // Positional arguments may precede named ones; only the reverse is rejected.
    let parsed = parse_source("const Made = sub(1, b: 2)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn all_named_arguments_are_accepted() {
    let parsed = parse_source("const Made = sub(a: 1, b: 2)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn positional_after_named_is_rejected_inside_function_bodies() {
    // A call statement in a body reaches the parser through a different path than a
    // `const` value, so the rule is checked over the whole tree.
    let parsed = parse_source("fn run() {\n    log(level: 1, 2)\n}\n");
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed)
        ),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn positional_after_named_is_rejected_in_nested_calls() {
    // The walk descends into argument values, so an offending inner call is
    // reported even when the surrounding call is well-formed.
    let parsed = parse_source("const Made = outer(inner(b: 1, 2))\n");
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed)
        ),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_conversion_and_constructor_calls() {
    // Conversion call on a type keyword.
    let parsed = parse_source("const Count: int = int(raw)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { callee, .. }) = &decl.value else {
        panic!("expected conversion call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["int"]),
        "expected int callee, got {callee:?}"
    );

    let parsed = parse_source("const Loaded = Id(^books, \"book-17\")\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { callee, .. }) = &decl.value else {
        panic!("expected identity constructor call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["Id"]),
        "expected Id callee, got {callee:?}"
    );

    // Qualified calls keep their path segments.
    let parsed = parse_source("const First = shelf::make(17)\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { callee, args, .. }) = &decl.value else {
        panic!("expected constructor call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if crate::common::segment_texts(segments) == ["shelf", "make"]),
        "expected shelf::make callee, got {callee:?}"
    );
    assert_eq!(args.len(), 1);
}

#[test]
fn keyword_head_and_keyword_path_segment_are_rejected() {
    // There is no dedicated qualified-`Id` rule: a keyword head (`Id::fromKey`) cannot
    // begin an expression, and a keyword path segment (`Author::Id`) breaks the
    // qualified-name continuation.
    let parsed = parse_source("const Bad = Author::Id(7)\n");
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression))
        ),
        "{:#?}",
        parsed.diagnostics
    );

    let parsed = parse_source("const Bad = Id::fromKey(7)\n");
    assert!(
        has_reason(
            parsed.diagnostics.complete(),
            parse_reason(ParseDiagnosticReason::KeywordExpression)
        ),
        "{:#?}",
        parsed.diagnostics
    );
}
