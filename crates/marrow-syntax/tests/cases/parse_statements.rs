//! Function-body statements: bindings, assignments, saved writes, keyed `var`,
//! reserved statement surfaces, and the body diagnostics for malformed lines.

use crate::common;
use common::{has_reason, lexer_reason, parse_reason};
use marrow_syntax::{
    CheckedBind, CompoundAssignOp, Diagnose, ExpectedSyntax, Expression, LexerDiagnosticReason,
    ObsoleteOperator, ParseDiagnosticReason, ReservedSyntax, Statement, UnsupportedSyntax,
    format_source, parse_source,
};

#[test]
fn parses_simple_statements_in_function_bodies() {
    let parsed = parse_source(
        "module app\n\
         fn main()\n\
         \x20   const title: string = \"Small Gods\"\n\
         \x20   var count: int = 0\n\
         \x20   count = count + 1\n\
         \x20   print(title)\n\
         \x20   return count\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let main = parsed.file.function("main").expect("main function");
    let statements = &main.body.statements;
    assert_eq!(statements.len(), 5, "{statements:#?}");

    assert!(
        matches!(
            &statements[0],
            Statement::Const { name, ty: Some(ty), value: Expression::Literal { .. }, .. }
                if name == "title" && ty.to_string() == "string"
        ),
        "stmt 0: {:?}",
        statements[0]
    );
    assert!(
        matches!(
            &statements[1],
            Statement::Var { name, ty: Some(ty), value: Some(_), .. }
                if name == "count" && ty.to_string() == "int"
        ),
        "stmt 1: {:?}",
        statements[1]
    );
    assert!(
        matches!(
            &statements[2],
            Statement::Assign { target: Expression::Name { segments, .. }, .. }
                if segments == &["count"]
        ),
        "stmt 2: {:?}",
        statements[2]
    );
    assert!(
        matches!(
            &statements[3],
            Statement::Expr {
                value: Expression::Call { .. },
                ..
            }
        ),
        "stmt 3: {:?}",
        statements[3]
    );
    assert!(
        matches!(
            &statements[4],
            Statement::Return { value: Some(Expression::Name { segments, .. }), .. }
                if segments == &["count"]
        ),
        "stmt 4: {:?}",
        statements[4]
    );
}

#[test]
fn parses_return_absent_as_a_return_of_the_absent_value() {
    // `absent` is an ordinary primary expression, so `return absent` is a `Return`
    // carrying the `Absent` value rather than a special return form.
    let parsed = parse_source(
        "module app\n\
         fn f(): int?\n\
         \x20   return absent\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("function");
    assert!(
        matches!(
            &f.body.statements[0],
            Statement::Return {
                value: Some(Expression::Absent { .. }),
                ..
            }
        ),
        "{:#?}",
        f.body.statements[0]
    );
}

#[test]
fn if_const_accepts_a_type_annotation() {
    // `if const name: T = place` accepts the annotation the same way `const`/`var`
    // do, rather than dead-ending in a generic "expected an expression" error.
    let parsed = parse_source(
        "module app\n\
         fn title(id: Id(^books))\n\
         \x20   if const pages: int = ^books(id).pages\n\
         \x20       print(pages)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let title = parsed.file.function("title").expect("title function");
    let Statement::IfConst {
        name, ty, value, ..
    } = &title.body.statements[0]
    else {
        panic!(
            "expected an if const statement, got {:?}",
            title.body.statements[0]
        );
    };
    assert_eq!(name, "pages");
    assert!(
        matches!(ty, Some(ty) if ty.to_string() == "int"),
        "expected the `: int` annotation to be bound, got {ty:?}"
    );
    assert!(
        matches!(value, Expression::Field { name, .. } if name == "pages"),
        "binding value: {value:?}"
    );
}

#[test]
fn absent_is_a_primary_expression() {
    // The empty optional `absent` is a first-class primary value, usable wherever
    // an expression is, such as a `const` initializer.
    let parsed = parse_source("module app\nfn f()\n    const x = absent\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("function");
    assert!(
        matches!(
            &f.body.statements[0],
            Statement::Const {
                value: Expression::Absent { .. },
                ..
            }
        ),
        "{:#?}",
        f.body.statements[0]
    );
}

#[test]
fn parses_a_type_keyword_as_a_path_segment() {
    // `bytes` is a type keyword but must be valid mid-path, as in `std::bytes::length`.
    let parsed = parse_source(
        "module app\n\
         fn main()\n\
         \x20   return std::bytes::length(data)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let main = parsed.file.function("main").expect("main function");
    assert!(
        matches!(
            &main.body.statements[0],
            Statement::Return { value: Some(Expression::Call { callee, .. }), .. }
                if matches!(callee.as_ref(),
                    Expression::Name { segments, .. } if segments == &["std", "bytes", "length"])
        ),
        "{:#?}",
        main.body.statements[0]
    );
}

#[test]
fn parses_a_type_keyword_as_a_leading_path_segment() {
    // A short-form std call leads its path with a type keyword, as in `bytes::length`
    // after `use std::bytes`. The keyword must begin a path when followed by `::`,
    // exactly as it is valid mid-path — otherwise short-form `std::bytes` is unusable.
    let parsed = parse_source(
        "module app\n\
         use std::bytes\n\
         fn main()\n\
         \x20   return bytes::length(data)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let main = parsed.file.function("main").expect("main function");
    assert!(
        matches!(
            &main.body.statements[0],
            Statement::Return { value: Some(Expression::Call { callee, .. }), .. }
                if matches!(callee.as_ref(),
                    Expression::Name { segments, .. } if segments == &["bytes", "length"])
        ),
        "{:#?}",
        main.body.statements[0]
    );
}

#[test]
fn parses_keyed_var_declaration() {
    let parsed = parse_source(
        "module app\n\
         fn tally()\n\
         \x20   var counts(name: string): int\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let tally = parsed.file.function("tally").expect("tally function");
    let Statement::Var {
        name,
        keys,
        ty,
        value,
        ..
    } = &tally.body.statements[0]
    else {
        panic!("expected var, got {:?}", tally.body.statements[0]);
    };
    assert_eq!(name, "counts");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "name");
    assert_eq!(keys[0].ty.to_string(), "string");
    assert_eq!(ty.as_ref().map(|t| t.to_string()).as_deref(), Some("int"));
    assert_eq!(*value, None);
}

#[test]
fn keyed_var_preserves_key_type_spelling_for_downstream_resolution() {
    let parsed = parse_source(
        "module app\n\
         fn tally()\n\
         \x20   var counts(name: 1): int\n",
    );
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let tally = parsed.file.function("tally").expect("tally function");
    let Statement::Var { keys, ty, .. } = &tally.body.statements[0] else {
        panic!("expected var, got {:?}", tally.body.statements[0]);
    };
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "name");
    assert_eq!(keys[0].ty.to_string(), "1");
    assert_eq!(ty.as_ref().map(ToString::to_string).as_deref(), Some("int"));
}

#[test]
fn comment_lines_inside_a_multi_line_keyed_var_key_list_are_skipped() {
    let parsed = parse_source(
        "module app\n\
         fn tally()\n\
         \x20   var scores(\n\
         \x20       player: string, ; who is scoring\n\
         \x20       ; the round being recorded\n\
         \x20       round: int,\n\
         \x20   ): int\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let tally = parsed.file.function("tally").expect("tally function");
    let Statement::Var { name, keys, ty, .. } = &tally.body.statements[0] else {
        panic!("expected var, got {:?}", tally.body.statements[0]);
    };
    assert_eq!(name, "scores");
    assert_eq!(
        keys.iter()
            .map(|key| (key.name.clone(), key.ty.to_string()))
            .collect::<Vec<_>>(),
        vec![
            ("player".to_string(), "string".to_string()),
            ("round".to_string(), "int".to_string()),
        ]
    );
    assert_eq!(ty.as_ref().map(ToString::to_string).as_deref(), Some("int"));
}

#[test]
fn keyed_var_key_list_errors_keep_key_specific_reasons() {
    let source = "fn f()\n    var counts(): int\n";
    let parsed = parse_source(source);

    assert!(parsed.has_errors(), "expected error for:\n{source}");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::EmptyKeyParameters)
        ),
        "expected keyed-var diagnostic for {source}: {:#?}",
        parsed.diagnostics
    );
    assert!(
        !has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement))
        ),
        "keyed-var errors should not fall back to statement recovery for {source}: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn keyed_var_rejects_structural_equal_inside_key_type_annotations() {
    let source = "fn f()\n    var counts(name: int = 1): string\n";
    let parsed = parse_source(source);

    assert!(parsed.has_errors(), "expected error for:\n{source}");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::KeyType))
        ),
        "expected keyed-var key-type diagnostic for {source}: {:#?}",
        parsed.diagnostics
    );
    assert!(
        !has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement))
        ),
        "keyed-var errors should not fall back to statement recovery for {source}: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn bracket_collection_literal_inside_call_does_not_fall_back_to_expected_statement() {
    let source = "module app\n\
         fn main()\n\
         \x20   print(two_num([1,2,3], 3))\n\
         fn two_num(nums: List[int], target: int): List[int]\n\
         \x20   return nums\n";
    let parsed = parse_source(source);

    assert!(parsed.has_errors(), "expected error for:\n{source}");
    assert!(
        !has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement))
        ),
        "bracket literal errors should not fall back to statement recovery: {:#?}",
        parsed.diagnostics
    );
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::BracketCollectionLiterals,
                ))
        })
        .expect("expected bracket collection literal diagnostic");
    assert_eq!(
        (diagnostic.span.line, diagnostic.span.column),
        (3, 19),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn local_bindings_reject_structural_equal_inside_type_annotations() {
    let cases = [
        (
            "fn f()\n    var x: List[a = b] = 1\n",
            ExpectedSyntax::ParameterType,
        ),
        (
            "fn f()\n    const x: List[a = b] = 1\n",
            ExpectedSyntax::ConstType,
        ),
    ];

    for (source, expected) in cases {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            has_reason(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(expected))
            ),
            "expected local binding type diagnostic for {source}: {:#?}",
            parsed.diagnostics
        );
        assert!(
            !has_reason(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement))
            ),
            "local binding type errors should not fall back to statement recovery for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn parses_keyed_var_with_multiple_keys_and_trailing_comma() {
    let parsed = parse_source(
        "module app\n\
         fn grid()\n\
         \x20   var cells(x: int, y: int,): bool\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let grid = parsed.file.function("grid").expect("grid function");
    let Statement::Var { keys, ty, .. } = &grid.body.statements[0] else {
        panic!("expected var, got {:?}", grid.body.statements[0]);
    };
    assert_eq!(keys.len(), 2, "{keys:#?}");
    assert_eq!(keys[0].name, "x");
    assert_eq!(keys[1].name, "y");
    assert_eq!(ty.as_ref().map(|t| t.to_string()).as_deref(), Some("bool"));
}

#[test]
fn parses_saved_writes_and_var_without_value() {
    let parsed = parse_source(
        "module app\n\
         fn save()\n\
         \x20   var book: Book\n\
         \x20   ^books(id).title = title\n\
         \x20   delete ^books(id).subtitle\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let save = parsed.file.function("save").expect("save function");
    let statements = &save.body.statements;
    assert_eq!(statements.len(), 3, "{statements:#?}");
    assert!(
        matches!(&statements[0], Statement::Var { name, value: None, .. } if name == "book"),
        "stmt 0: {:?}",
        statements[0]
    );
    assert!(
        matches!(
            &statements[1],
            Statement::Assign { target: Expression::Field { name, .. }, .. } if name == "title"
        ),
        "stmt 1: {:?}",
        statements[1]
    );
    assert!(
        matches!(&statements[2], Statement::Delete { .. }),
        "stmt 2: {:?}",
        statements[2]
    );
}

#[test]
fn rejects_lock_as_reserved_statement_and_consumes_its_block() {
    let parsed = parse_source(
        "module app\n\
         fn commit(id: Id(^books))\n\
         \x20   lock ^books(id)\n\
         \x20       transaction\n\
         \x20           ^books(id).title = title\n",
    );
    assert!(parsed.has_errors(), "expected lock rejection");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Reserved(
                ReservedSyntax::LockStatement
            ))
        ),
        "{:#?}",
        parsed.diagnostics
    );
    assert!(
        !parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Statement))),
        "{:#?}",
        parsed.diagnostics
    );
    let commit = parsed.file.function("commit").expect("commit function");
    assert!(commit.body.statements.is_empty(), "{commit:#?}");
}

#[test]
fn rejects_merge_as_reserved_statement() {
    let parsed = parse_source(
        "module app\n\
         fn commit(id: Id(^books))\n\
         \x20   merge ^books(id) = ^books(id)\n\
         \x20   print(\"after\")\n",
    );
    assert!(parsed.has_errors(), "expected merge rejection");
    assert!(
        has_reason(
            &parsed.diagnostics,
            parse_reason(ParseDiagnosticReason::Reserved(
                ReservedSyntax::MergeStatement
            ))
        ),
        "{:#?}",
        parsed.diagnostics
    );
    // Total parsing keeps the rejected `merge` line as an error node rather than
    // dropping it, so the body reads as the reserved line followed by the print.
    let commit = parsed.file.function("commit").expect("commit function");
    assert_eq!(commit.body.statements.len(), 2, "{commit:#?}");
    assert!(
        matches!(&commit.body.statements[0], Statement::Error { .. }),
        "{:#?}",
        commit.body.statements[0]
    );
    assert!(
        matches!(&commit.body.statements[1], Statement::Expr { .. }),
        "{:#?}",
        commit.body.statements[1]
    );
}

#[test]
fn statement_spanning_open_delimiters_stays_one_statement() {
    let parsed = parse_source(
        "module app\n\
         fn make()\n\
         \x20   log(\n\
         \x20       code: \"book.absent\",\n\
         \x20       message: \"missing\",\n\
         \x20   )\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let make = parsed.file.function("make").expect("make function");
    let statements = &make.body.statements;
    assert_eq!(statements.len(), 1, "{statements:#?}");
    assert!(
        matches!(
            &statements[0],
            Statement::Expr {
                value: Expression::Call { .. },
                ..
            }
        ),
        "stmt 0: {:?}",
        statements[0]
    );
}

#[test]
fn reports_malformed_body_statements_with_a_diagnostic() {
    // A statement the body parser cannot structure must surface a parse error
    // rather than becoming a silent `Statement::Unparsed` no-op.
    let cases = [
        "module app\nfn main()\n    foo +\n",
        "module app\nfn main()\n    const x: int\n",
    ];
    for source in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.has_errors(),
            "expected a diagnostic for {source:?}: {:#?}",
            parsed.diagnostics
        );
        let syntax = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "parse.syntax" && diagnostic.span.line == 3)
            .unwrap_or_else(|| panic!("expected a line-3 parse.syntax diagnostic for {source:?}"));
        assert_eq!(syntax.kind(), "parse", "{source:?}");
    }
}

#[test]
fn a_doc_comment_in_statement_position_is_a_parse_error() {
    // A `;;` doc comment attaches only to a declaration, member, or parameter.
    // In a statement position it has no target, so the parser must reject it
    // rather than silently swallow it — a program that passes check and runs must
    // be formattable, and a swallowed doc comment breaks that round trip.
    let cases = [
        // own line, before a statement
        ("module app\nfn main()\n    ;; orphan doc\n    return\n", 3),
        // trailing a statement
        ("module app\nfn main()\n    return ;; orphan doc\n", 3),
        // end of body
        ("module app\nfn main()\n    return\n    ;; orphan doc\n", 4),
        // inside an unexpected over-indented block, where the only content is the
        // doc comment, so the block carries no statement token to anchor the
        // unexpected-indentation error
        (
            "module app\nfn main()\n    print(\"a\")\n        ;; orphan doc\n",
            4,
        ),
    ];
    for (source, line) in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.has_errors(),
            "a statement-position doc comment must not parse cleanly: {source:?}: {:#?}",
            parsed.diagnostics
        );
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::DocCommentWithoutTarget)
            })
            .unwrap_or_else(|| panic!("expected a doc-comment diagnostic for {source:?}"));
        assert_eq!(diagnostic.code, "parse.syntax", "{source:?}");
        assert_eq!(diagnostic.span.line, line, "{source:?}");
    }
}

#[test]
fn a_dangling_doc_comment_with_no_following_target_is_a_parse_error() {
    // A `;;` doc comment attaches to the next declaration, member, or parameter.
    // With nothing to attach to — at end of file, at the end of a resource or
    // store body, or separated from the next declaration by a blank line — it has
    // no target and must be rejected everywhere, just like the statement-position
    // case, so it can never pass check and then brick the formatter.
    let cases = [
        // top-level, dangling at end of file
        ("module app\n;; just docs\n", 2),
        // top-level, separated from the next declaration by a blank line
        ("module app\n;; orphan\n\nfn main()\n    return\n", 2),
        // end of a resource body, after the last member
        (
            "module app\nresource Book\n    required title: string\n    ;; orphan\n",
            4,
        ),
        // end of a store body, after the last index
        (
            "module app\nresource Book\n    required title: string\nstore ^books(id: int): Book\n    index byTitle(title, id)\n    ;; orphan\n",
            6,
        ),
    ];
    for (source, line) in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.has_errors(),
            "a dangling doc comment must not parse cleanly: {source:?}: {:#?}",
            parsed.diagnostics
        );
        let diagnostic = parsed
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.reason == parse_reason(ParseDiagnosticReason::DocCommentWithoutTarget)
            })
            .unwrap_or_else(|| panic!("expected a doc-comment diagnostic for {source:?}"));
        assert_eq!(diagnostic.code, "parse.syntax", "{source:?}");
        assert_eq!(diagnostic.span.line, line, "{source:?}");
    }
}

#[test]
fn a_doc_comment_that_precedes_a_declaration_or_member_attaches_cleanly() {
    // The attachment cases must stay clean: a doc comment immediately before a
    // declaration, a resource member, a store index, or a parameter documents it
    // and is not a dangling error.
    for source in [
        "module app\n;; documents the const\nconst Limit: int = 10\n",
        "module app\nresource Book\n    ;; the title\n    required title: string\n",
        "module app\nresource Book\n    required title: string\nstore ^books(id: int): Book\n    ;; lookup by title\n    index byTitle(title, id)\n",
        "module app\n;; documents main\nfn main()\n    return\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            !parsed.has_errors(),
            "an attaching doc comment must parse cleanly: {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn an_ordinary_comment_in_statement_position_parses_cleanly() {
    // A single-`;` line comment in statement position is fine; only `;;` doc
    // comments require an attachment target.
    for source in [
        "module app\nfn main()\n    ; ordinary\n    return\n",
        "module app\nfn main()\n    return ; ordinary\n",
        "module app\nfn main()\n    return\n    ; ordinary\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            !parsed.has_errors(),
            "an ordinary statement comment must parse cleanly: {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn a_doc_comment_on_a_declaration_still_attaches() {
    // The doc-comment rejection is scoped to statement position; a `;;` doc
    // comment on a declaration attaches as before.
    let parsed = parse_source("module app\n;; documents the function\nfn main()\n    return\n");
    assert!(
        !parsed.has_errors(),
        "a doc comment on a declaration must still attach: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn reports_unexpected_indentation_after_simple_statements() {
    let parsed = parse_source(
        "module app\n\
         fn main()\n\
         \x20   print(\"kept\")\n\
         \x20       print(\"over-indented\")\n",
    );

    assert!(
        parsed.has_errors(),
        "an unexpected nested line must not parse cleanly: {:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.span.line == 4
                && diagnostic.reason == parse_reason(ParseDiagnosticReason::UnexpectedIndentation)),
        "expected a line-4 indentation diagnostic: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_final_block_statement_without_trailing_newline() {
    let parsed = parse_source("module app\nfn main()\n    if ready\n        return");

    assert!(
        parsed.diagnostics.is_empty(),
        "EOF should close the final newline/dedent sequence: {:#?}",
        parsed.diagnostics
    );
    let main = parsed.file.function("main").expect("main function");
    assert!(matches!(main.body.statements[0], Statement::If { .. }));
}

#[test]
fn surfaces_lexer_diagnostics_for_function_body_tokens() {
    let parsed = parse_source("module app\nfn main()\n    return a && b\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let obsolete = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.reason
                == lexer_reason(LexerDiagnosticReason::ObsoleteOperator(
                    ObsoleteOperator::AndAnd,
                ))
        })
        .expect("expected obsolete operator diagnostic");
    assert_eq!(obsolete.code, "parse.syntax");
    assert_eq!(obsolete.kind(), "parse");
    assert_eq!(obsolete.span.line, 3);
    assert_eq!(
        obsolete.help.as_deref(),
        Some("Use `and` for boolean and."),
        "{:#?}",
        obsolete.help
    );
}

#[test]
fn out_is_an_ordinary_variable_name() {
    let parsed = parse_source("module app\nfn f(): int\n    var out: int = 0\n    return out\n");

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
}

#[test]
fn finally_is_an_ordinary_variable_name() {
    let parsed = parse_source(
        "module app\nfn f(): string\n    var finally: string = \"done\"\n    return finally\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
}

#[test]
fn parses_compound_assignment_from_single_operator_token() {
    for (source, expected_op) in [
        ("module app\nfn f()\n    i+=3\n", CompoundAssignOp::Add),
        (
            "module app\nfn f()\n    i -= 3\n",
            CompoundAssignOp::Subtract,
        ),
        ("module app\nfn f()\n    i*=3\n", CompoundAssignOp::Multiply),
        ("module app\nfn f()\n    i /= 3\n", CompoundAssignOp::Divide),
        (
            "module app\nfn f()\n    i%=3\n",
            CompoundAssignOp::Remainder,
        ),
    ] {
        let parsed = parse_source(source);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let f = parsed.file.function("f").expect("function");
        assert!(
            matches!(
                &f.body.statements[0],
                Statement::CompoundAssign {
                    target: Expression::Name { segments, .. },
                    op,
                    value: Expression::Literal { .. },
                    ..
                } if segments == &["i"] && *op == expected_op
            ),
            "{:#?}",
            f.body.statements[0]
        );
    }
}

#[test]
fn split_compound_assignment_is_rejected_with_a_recovery_node() {
    // Each compound operator is a single token, so a space before the `=`
    // (`i * = 3`) is not a compound assignment: it reports and leaves an error
    // node so the body still parses.
    let parsed = parse_source("module app\nfn f()\n    i * = 3\n");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic.reason
            == parse_reason(ParseDiagnosticReason::SplitCompoundAssign)),
        "{:#?}",
        parsed.diagnostics
    );
    let f = parsed.file.function("f").expect("function");
    assert!(
        matches!(&f.body.statements[0], Statement::Error { .. }),
        "{:#?}",
        f.body.statements[0]
    );
}

#[test]
fn spaced_compound_assignment_does_not_generalize_to_comparisons() {
    let parsed = parse_source("module app\nfn f()\n    i <= 3\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("function");
    assert!(
        matches!(&f.body.statements[0], Statement::Expr { .. }),
        "{:#?}",
        f.body.statements[0]
    );

    let spaced = parse_source("module app\nfn f()\n    i < = 3\n");
    assert!(spaced.has_errors(), "{:#?}", spaced.diagnostics);
}

/// The checked-arithmetic form parses in all three binding positions with both
/// diverging arms, captured by fault kind regardless of source order.
#[test]
fn parses_checked_arithmetic_forms() {
    let parsed = parse_source(
        "module app\n\
         fn main(a: int, b: int)\n\
         \x20   const q: int = checked a / b\n\
         \x20       on out_of_range\n\
         \x20           return\n\
         \x20       on zero_divisor\n\
         \x20           return\n\
         \x20   var r = checked a + b\n\
         \x20       on out_of_range\n\
         \x20           r = 0\n\
         \x20   return\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let main = parsed.file.function("main").expect("main function");
    let statements = &main.body.statements;

    match &statements[0] {
        Statement::Checked {
            bind,
            out_of_range,
            zero_divisor,
            ..
        } => {
            assert!(
                matches!(bind, CheckedBind::Const { name, ty: Some(_) } if name == "q"),
                "{bind:#?}"
            );
            assert!(out_of_range.is_some() && zero_divisor.is_some());
        }
        other => panic!("expected a checked const, got {other:#?}"),
    }
    match &statements[1] {
        Statement::Checked {
            bind,
            out_of_range,
            zero_divisor,
            ..
        } => {
            assert!(
                matches!(bind, CheckedBind::Var { name, ty: None } if name == "r"),
                "{bind:#?}"
            );
            assert!(out_of_range.is_some() && zero_divisor.is_none());
        }
        other => panic!("expected a checked var, got {other:#?}"),
    }
}

/// `return checked ...` binds through a return.
#[test]
fn parses_checked_return() {
    let parsed = parse_source(
        "module app\n\
         fn main(a: int, b: int): int\n\
         \x20   return checked a * b\n\
         \x20       on out_of_range\n\
         \x20           return 0\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let main = parsed.file.function("main").expect("main function");
    assert!(matches!(
        &main.body.statements[0],
        Statement::Checked {
            bind: CheckedBind::Return,
            ..
        }
    ));
}

/// A checked form with no indented arms reports one `CheckedBody` diagnostic and
/// still yields a `Statement::Checked` node (total parsing).
#[test]
fn checked_form_missing_arms_reports_checked_body() {
    let parsed = parse_source(
        "module app\n\
         fn main(a: int, b: int)\n\
         \x20   const q = checked a + b\n\
         \x20   return\n",
    );
    assert!(has_reason(
        &parsed.diagnostics,
        parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::CheckedBody)),
    ));
    let main = parsed.file.function("main").expect("main function");
    assert!(matches!(
        &main.body.statements[0],
        Statement::Checked { .. }
    ));
}

/// A malformed arm header reports one `CheckedArm` diagnostic and its block does
/// not leak into the surrounding form.
#[test]
fn checked_form_bad_arm_reports_checked_arm() {
    let parsed = parse_source(
        "module app\n\
         fn main(a: int, b: int)\n\
         \x20   const q = checked a + b\n\
         \x20       on wat\n\
         \x20           return\n\
         \x20   return\n",
    );
    assert!(has_reason(
        &parsed.diagnostics,
        parse_reason(ParseDiagnosticReason::CheckedArm),
    ));
}

/// The checked form formats idempotently: arms render `on out_of_range` before
/// `on zero_divisor`, and formatting a formatted form is a fixed point.
#[test]
fn checked_form_formats_idempotently() {
    let source = "module app\n\
         fn main(a: int, b: int): int\n\
         \x20   const q: int = checked a / b\n\
         \x20       on zero_divisor\n\
         \x20           return 0\n\
         \x20       on out_of_range\n\
         \x20           return 1\n\
         \x20   return q\n";
    let once = format_source(source);
    let twice = format_source(&once);
    assert_eq!(once, twice, "formatting is a fixed point:\n{once}");
    // The fixed-order render puts out_of_range first even though source had it second.
    let oor = once.find("on out_of_range").expect("out_of_range arm");
    let zd = once.find("on zero_divisor").expect("zero_divisor arm");
    assert!(
        oor < zd,
        "out_of_range renders before zero_divisor:\n{once}"
    );
    // The formatted output re-parses cleanly.
    assert!(parse_source(&once).diagnostics.is_empty());
}

/// `place name = ^root(key)` parses to a `PlaceBinding` naming the entry-address
/// expression; the compiler owns the durable checks, the parser only structures it.
#[test]
fn parses_a_place_binding() {
    let parsed = parse_source(
        "module app\n\
         fn main(id: int)\n\
         \x20   place book = ^books(id)\n\
         \x20   book.title = \"x\"\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let main = parsed.file.function("main").expect("main function");
    assert!(
        matches!(
            &main.body.statements[0],
            Statement::PlaceBinding { name, place: Expression::Call { .. }, .. }
                if name == "book"
        ),
        "stmt 0: {:?}",
        main.body.statements[0]
    );
}

/// `place` in name position is a keyword, so a missing name or missing `=` is a
/// single bounded parse error rather than a dropped or cascading line.
#[test]
fn a_malformed_place_binding_is_one_parse_error() {
    let missing_name = parse_source(
        "module app\n\
         fn main()\n\
         \x20   place = 1\n",
    );
    assert!(!missing_name.diagnostics.is_empty());

    let missing_equals = parse_source(
        "module app\n\
         fn main(id: int)\n\
         \x20   place book ^books(id)\n",
    );
    assert!(!missing_equals.diagnostics.is_empty());
}

/// A `place` binding formats idempotently and re-parses cleanly.
#[test]
fn place_binding_formats_idempotently() {
    let source = "module app\n\
         fn main(id: int)\n\
         \x20   place book = ^books(id)\n\
         \x20   book.title = \"x\"\n\
         \x20   delete book\n";
    let once = format_source(source);
    let twice = format_source(&once);
    assert_eq!(once, twice, "formatting is a fixed point:\n{once}");
    assert!(once.contains("place book = ^books(id)"), "{once}");
    assert!(parse_source(&once).diagnostics.is_empty());
}
