//! The brace block grammar: `{ … }` blocks, `NEWLINE`-or-`}` statement termination,
//! cuddled and inline trailing clauses, `=>` match arms, newline enum members, and
//! header continuation.

use crate::common::CompletePayload;
use std::sync::mpsc;
use std::time::Duration;

use marrow_syntax::{
    Declaration, DiagnosticReason, ExpectedSyntax, LexerDiagnosticReason, ParseDiagnosticReason,
    ParsedSource, ResourceMember, Statement, parse_source,
};

/// Parse `source` on a worker thread and fail if it does not return promptly: a parser
/// that loops on a body missing its `}` becomes a test failure, not a stuck suite.
fn parse_bounded(source: &str) -> ParsedSource {
    let owned = source.to_string();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let parsed = parse_source(&owned);
        let _ = tx.send(parsed);
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(parsed) => {
            let _ = worker.join();
            parsed
        }
        Err(_) => panic!("parse_source did not terminate within 5s for {source:?}"),
    }
}

fn has_unclosed_block(parsed: &ParsedSource) -> bool {
    parsed.diagnostics.complete().iter().any(|d| {
        d.reason
            == DiagnosticReason::Parser(ParseDiagnosticReason::Expected(ExpectedSyntax::CloseBrace))
    })
}

fn all_spans_one_based(parsed: &ParsedSource) -> bool {
    parsed
        .diagnostics
        .complete()
        .iter()
        .all(|d| d.span.line >= 1 && d.span.column >= 1)
}

/// An unclosed declaration body must return promptly with a bounded, well-spanned
/// diagnostic naming the missing close brace.
fn assert_unclosed_block(source: &str) {
    let parsed = parse_bounded(source);
    assert!(
        parsed.diagnostics.complete().len() < 16,
        "diagnostics stay bounded for {source:?}: {:#?}",
        parsed.diagnostics
    );
    assert!(
        all_spans_one_based(&parsed),
        "every diagnostic keeps a valid 1-based span for {source:?}: {:#?}",
        parsed.diagnostics
    );
    assert!(
        has_unclosed_block(&parsed),
        "an unclosed block reports a CloseBrace diagnostic for {source:?}: {:#?}",
        parsed.diagnostics
    );
}

fn clean(source: &str) {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "expected a clean parse of {source:?}: {:#?}",
        parsed.diagnostics
    );
}

// ---- happy paths ----

#[test]
fn a_braced_function_body_parses_clean() {
    clean("module app\nfn run() {\n    return\n}\n");
}

#[test]
fn a_single_statement_body_still_needs_braces_and_parses() {
    let parsed = parse_source("module app\nfn run(): int {\n    return 1\n}\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let run = parsed.file.function("run").expect("run");
    assert!(matches!(run.body.statements[0], Statement::Return { .. }));
}

#[test]
fn compound_statements_take_braced_blocks() {
    clean(
        "module app\nfn run(n: int) {\n    if n < 0 {\n        return\n    }\n    \
         while n < 10 {\n        n = n\n    }\n    for i in 1..10 {\n        log(i)\n    }\n    \
         transaction {\n        ^c[1].v = n\n    }\n}\n",
    );
}

#[test]
fn a_resource_store_and_group_use_braces() {
    let parsed = parse_source(
        "module app\nresource Book {\n    required title: string\n    \
         notes[noteId: string] {\n        text: string\n    }\n}\n\
         store ^books[id: int]: Book {\n    index byTitle[title]\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let book = parsed.file.resource("Book").expect("Book");
    assert!(
        book.members
            .iter()
            .any(|member| matches!(member, ResourceMember::Group(_))),
        "the notes group parses as a nested member: {book:#?}"
    );
}

#[test]
fn a_store_without_members_needs_no_braces() {
    clean("module app\nresource B {\n    t: string\n}\nstore ^books: B\n");
}

// ---- cuddled vs non-cuddled trailing clauses ----

#[test]
fn a_cuddled_else_parses() {
    clean(
        "module app\nfn run(n: int) {\n    if n < 0 {\n        return\n    } else {\n        return\n    }\n}\n",
    );
}

#[test]
fn a_non_cuddled_else_on_its_own_line_parses() {
    clean(
        "module app\nfn run(n: int) {\n    if n < 0 {\n        return\n    }\n    else {\n        return\n    }\n}\n",
    );
}

#[test]
fn an_else_if_chain_parses() {
    let parsed = parse_source(
        "module app\nfn run(n: int) {\n    if n < 0 {\n        return\n    } else if n > 0 {\n        return\n    } else {\n        return\n    }\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Statement::If {
        else_ifs,
        else_block,
        ..
    } = &parsed.file.function("run").unwrap().body.statements[0]
    else {
        panic!("expected if");
    };
    assert_eq!(else_ifs.len(), 1);
    assert!(else_block.is_some());
}

#[test]
fn an_inline_diverging_else_needs_no_braces() {
    let parsed = parse_source(
        "module app\nfn run(n: int): int {\n    if n < 0 {\n        return 0\n    } else return 1\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Statement::If { else_block, .. } = &parsed.file.function("run").unwrap().body.statements[0]
    else {
        panic!("expected if");
    };
    let block = else_block.as_ref().expect("inline else block");
    assert_eq!(
        block.statements.len(),
        1,
        "one inline statement: {block:#?}"
    );
}

#[test]
fn an_inline_on_more_clause_cuddles_the_loop_brace() {
    let parsed = parse_source(
        "module app\nfn run(): int {\n    for k in ^c.items at most 5 {\n        log(k)\n    } on more return -1\n    return 0\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Statement::For { bound, .. } = &parsed.file.function("run").unwrap().body.statements[0]
    else {
        panic!("expected for");
    };
    let on_more = bound
        .as_ref()
        .and_then(|bound| bound.on_more.as_ref())
        .expect("on more block");
    assert_eq!(on_more.statements.len(), 1);
}

// ---- match arms with => ----

#[test]
fn match_arms_use_fat_arrows_with_inline_and_braced_bodies() {
    let parsed = parse_source(
        "module app\nfn run(s: Shape): int {\n    match s {\n        dot => return 0\n        circle(r) => {\n            return r\n        }\n    }\n    return -1\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Statement::Match { arms, .. } = &parsed.file.function("run").unwrap().body.statements[0]
    else {
        panic!("expected match");
    };
    assert_eq!(arms.len(), 2, "{arms:#?}");
    assert_eq!(arms[1].bindings.len(), 1, "circle binds r");
}

#[test]
fn a_match_arm_without_a_fat_arrow_reports_once() {
    let parsed =
        parse_source("module app\nfn run(s: Shape) {\n    match s {\n        dot\n    }\n}\n");
    assert!(
        parsed.diagnostics.complete().iter().any(
            |d| d.reason == DiagnosticReason::Parser(ParseDiagnosticReason::MatchArmMemberPath)
        ),
        "a `=>`-less arm reports the arm error: {:#?}",
        parsed.diagnostics
    );
}

// ---- enum members are newline-separated, categories nest with braces ----

#[test]
fn enum_members_are_newline_separated_with_braced_categories() {
    let parsed = parse_source(
        "module app\nenum Cat {\n    lion\n    tiger {\n        bengal\n        siberian\n    }\n}\n",
    );
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
    let Some(Declaration::Enum(decl)) = parsed
        .file
        .declarations
        .iter()
        .find(|d| matches!(d, Declaration::Enum(_)))
    else {
        panic!("expected enum");
    };
    assert_eq!(decl.members.len(), 2, "lion and tiger: {decl:#?}");
    let tiger = decl
        .members
        .iter()
        .find(|member| member.name == "tiger")
        .expect("tiger");
    assert_eq!(tiger.members.len(), 2, "bengal and siberian");
}

// ---- header continuation ----

#[test]
fn a_header_continues_after_a_trailing_and() {
    clean(
        "module app\nfn run(a: bool, b: bool) {\n    if a and\n       b {\n        return\n    }\n}\n",
    );
}

#[test]
fn a_value_continues_after_a_trailing_equals() {
    let parsed = parse_source("module app\nconst Total: int =\n    2 * 3\n");
    assert!(
        parsed.diagnostics.complete().is_empty(),
        "{:#?}",
        parsed.diagnostics
    );
}

// ---- hostile / recovery ----

#[test]
fn an_unbalanced_open_brace_does_not_panic_and_reports() {
    let parsed = parse_source("module app\nfn run() {\n    return\n");
    // No panic (reaching here is the assertion) and the following file still parses;
    // a missing close is tolerated as end-of-input.
    let _ = parsed;
}

#[test]
fn a_stray_close_brace_syncs_recovery_and_the_next_decl_parses() {
    let parsed = parse_source("module app\nfn a() {\n    }\n}\nfn b() {\n    return\n}\n");
    // The stray `}` at top level is reported, and `b` still parses past it.
    assert!(
        parsed.file.function("b").is_some(),
        "b parses past the stray brace: {:#?}",
        parsed.file
    );
}

#[test]
fn old_layout_input_yields_bounded_diagnostics_without_panic() {
    // Layout source (no braces) must fail closed with diagnostics, not a panic or a
    // flood: the body is not a brace block, so the function body is empty/erroring.
    let parsed = parse_source("module app\nfn run()\n    return\n");
    assert!(
        !parsed.diagnostics.complete().is_empty(),
        "layout input is now a diagnostic"
    );
    assert!(
        parsed.diagnostics.complete().len() < 10,
        "diagnostics stay bounded, not a per-line flood: {:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed
            .diagnostics
            .complete()
            .iter()
            .all(|d| d.span.line >= 1 && d.span.column >= 1),
        "every diagnostic keeps a valid 1-based span"
    );
}

#[test]
fn a_bare_block_in_statement_position_is_rejected() {
    let parsed = parse_source("module app\nfn run() {\n    {\n        return\n    }\n}\n");
    assert!(
        parsed
            .diagnostics
            .complete()
            .iter()
            .any(|d| d.reason == DiagnosticReason::Parser(ParseDiagnosticReason::UnexpectedBlock)),
        "a bare block has no statement form: {:#?}",
        parsed.diagnostics
    );
}

// ---- if-const chain and let-else parse-only forms ----

#[test]
fn an_if_const_chain_parses_to_the_chain_node() {
    let parsed = parse_source(
        "module app\nfn run(): int {\n    if const a = ^c[1].v and const b = ^c[2].v and a < b {\n        return 1\n    }\n    return 0\n}\n",
    );
    let Statement::IfConstChain {
        bindings,
        condition,
        ..
    } = &parsed.file.function("run").unwrap().body.statements[0]
    else {
        panic!(
            "expected an if-const chain: {:#?}",
            parsed.file.function("run").unwrap().body.statements[0]
        );
    };
    assert_eq!(bindings.len(), 2, "two const bindings");
    assert!(condition.is_some(), "a trailing condition");
}

#[test]
fn a_let_else_parses_to_the_let_else_node() {
    let parsed = parse_source(
        "module app\nfn run(): int {\n    const x = ^c[1].v else return -1\n    return x\n}\n",
    );
    let Statement::LetElse {
        is_var, else_block, ..
    } = &parsed.file.function("run").unwrap().body.statements[0]
    else {
        panic!(
            "expected a let-else: {:#?}",
            parsed.file.function("run").unwrap().body.statements[0]
        );
    };
    assert!(!is_var, "const let-else");
    assert_eq!(else_block.statements.len(), 1, "one inline diverging stmt");
}

#[test]
fn a_semicolon_comment_is_no_longer_a_comment() {
    // `;` is not a comment leader; the lexer reports it as an unexpected character.
    let parsed = parse_source("module app\n; not a comment\n");
    assert!(
        parsed.diagnostics.complete().iter().any(|d| d.reason
            == DiagnosticReason::Lexer(LexerDiagnosticReason::UnexpectedCharacter(';'))),
        "a leading `;` is an unexpected character, not comment trivia: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn slash_slash_is_a_line_comment() {
    clean("module app\n// a line comment\nfn run() {\n    return // trailing\n}\n");
}

// ---- unclosed declaration bodies terminate with a bounded diagnostic ----

#[test]
fn a_resource_body_missing_its_close_brace_reports_and_terminates() {
    assert_unclosed_block("module app\nresource B {\n    t: string\n");
}

#[test]
fn an_enum_body_missing_its_close_brace_reports_and_terminates() {
    assert_unclosed_block("module app\nenum E {\n    a\n    b\n");
}

#[test]
fn a_store_body_missing_its_close_brace_reports_and_terminates() {
    assert_unclosed_block(
        "module app\nresource B {\n    t: string\n}\nstore ^books[id: int]: B {\n    index byT[t]\n",
    );
}

#[test]
fn a_nested_group_stealing_the_outer_brace_reports_and_terminates() {
    // The inner group `a { ... }` consumes the only `}`, leaving the outer resource
    // unclosed: parsing still terminates and reports the outer block.
    let parsed = parse_bounded("module app\nresource B {\n    a{b\n}\n");
    assert!(
        parsed.diagnostics.complete().len() < 16,
        "bounded diagnostics: {:#?}",
        parsed.diagnostics
    );
    assert!(
        all_spans_one_based(&parsed),
        "valid 1-based spans: {:#?}",
        parsed.diagnostics
    );
    assert!(
        has_unclosed_block(&parsed),
        "the stolen outer brace reports unclosed: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn a_bare_resource_open_brace_at_eof_reports_and_terminates() {
    assert_unclosed_block("module app\nresource B {\n");
}

// ---- no diagnostic ever anchors at the invalid 0,0 default span ----

#[test]
fn an_if_const_chain_with_a_trailing_and_never_anchors_at_zero() {
    // A trailing `and` leaves the final condition slice empty; the empty-slice
    // fallback must anchor on the `and`/line, never the invalid line-0/column-0
    // default span.
    let parsed =
        parse_source("module app\nfn run() {\n    if const a = x and{\n        return\n    }\n}\n");
    assert!(
        !parsed.diagnostics.complete().is_empty(),
        "the empty trailing condition is reported"
    );
    assert!(
        all_spans_one_based(&parsed),
        "a trailing `and` must not anchor a diagnostic at line 0 / column 0: {:#?}",
        parsed.diagnostics
    );
}

/// Every diagnostic of `source` as its typed reason and 1-based position, each checked
/// to name the same point by byte and by line/column.
fn located_reasons(source: &str) -> Vec<(DiagnosticReason, u32, u32)> {
    parse_bounded(source)
        .diagnostics
        .complete()
        .iter()
        .map(|d| {
            assert_eq!(
                d.span.start_byte,
                offset_of(source, d.span.line, d.span.column),
                "{source:?}: {d:#?} names different points by byte and by line/column"
            );
            (d.reason.clone(), d.span.line, d.span.column)
        })
        .collect()
}

/// The exact parse of each block-grammar shape the reference names: the typed reasons
/// and positions it reports, and the statements that stand as siblings afterwards.
#[test]
fn the_block_grammar_parse_matrix() {
    let block = |line, column| {
        (
            DiagnosticReason::Parser(ParseDiagnosticReason::Expected(ExpectedSyntax::Block)),
            line,
            column,
        )
    };
    let cases: [(
        &str,
        Vec<(DiagnosticReason, u32, u32)>,
        fn(&[Statement]) -> bool,
    ); 7] = [
        (
            "module app\nfn f() {\n    if a\n    return\n}\n",
            vec![block(4, 5)],
            |s| {
                matches!(s, [Statement::If { then_block, else_ifs, else_block: None, .. }, Statement::Return { .. }]
                    if then_block.statements.is_empty() && else_ifs.is_empty())
            },
        ),
        (
            "module app\nfn f() {\n    if a {\n    }\n}\n",
            vec![],
            |s| matches!(s, [Statement::If { then_block, .. }] if then_block.statements.is_empty()),
        ),
        (
            "module app\nfn f() {\n    if a {\n    } else\n    return\n}\n",
            vec![],
            |s| {
                matches!(s, [Statement::If { else_block: Some(block), .. }]
                    if matches!(block.statements.as_slice(), [Statement::Return { .. }]))
            },
        ),
        (
            "module app\nfn f() {\n    while x\n    x = 1\n}\n",
            vec![block(4, 5)],
            |s| {
                matches!(s, [Statement::While { body, .. }, Statement::Assign { .. }]
                    if body.statements.is_empty())
            },
        ),
        (
            "module app\nfn f() {\n    for i in xs at most 3\n    n = n + 1\n}\n",
            vec![block(4, 5)],
            |s| {
                matches!(s, [Statement::For { bound: Some(_), body, .. }, Statement::Assign { .. }]
                    if body.statements.is_empty())
            },
        ),
        (
            "module app\nfn f() {\n    match v {\n        a => return\n    }\n}\n",
            vec![],
            |s| {
                matches!(s, [Statement::Match { arms, .. }]
                    if matches!(arms.as_slice(), [arm] if matches!(arm.block.statements.as_slice(), [Statement::Return { .. }])))
            },
        ),
        (
            "module app\nfn f() {\n    if a return\n}\n",
            vec![
                (
                    DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::Expression,
                    )),
                    3,
                    10,
                ),
                block(3, 16),
            ],
            |s| matches!(s, [Statement::If { .. }]),
        ),
    ];
    for (source, expected, siblings) in cases {
        assert_eq!(located_reasons(source), expected, "{source:?}");
        let parsed = parse_bounded(source);
        let Some(Declaration::Function(function)) = parsed.file.declarations.first() else {
            panic!("{source:?}: the function still parses");
        };
        assert!(
            siblings(function.body.statements.as_slice()),
            "{source:?}: {:#?}",
            function.body.statements
        );
    }

    // A bare `fn` or `test` header reports the missing body at the gap on the same
    // terms, and the declaration stands with an empty body.
    for (source, line, column) in [
        ("module app\nfn f()\n", 3, 1),
        ("module app\ntest \"t\"\n", 3, 1),
        ("module app\nfn f()", 2, 7),
    ] {
        assert_eq!(
            located_reasons(source),
            vec![block(line, column)],
            "{source:?}"
        );
        let parsed = parse_bounded(source);
        assert!(
            matches!(
                parsed.file.declarations.as_slice(),
                [Declaration::Function(_)] | [Declaration::Test(_)]
            ),
            "{source:?}: {:#?}",
            parsed.file.declarations
        );
    }

    // A header that ends its body's token slice on a token other than a line break: the
    // gap sits just past the header, byte and line/column agreeing.
    let source = "module app\nfn f() { if a }\n";
    assert_eq!(located_reasons(source), vec![block(2, 14)], "{source:?}");
}

/// The byte offset of a 1-based `(line, column)` in `source`.
fn offset_of(source: &str, line: u32, column: u32) -> usize {
    let line_start: usize = source
        .split_inclusive('\n')
        .take(line as usize - 1)
        .map(str::len)
        .sum();
    line_start + column as usize - 1
}

/// A compound-statement header, `match` head, or trailing clause with neither a block
/// nor an inline statement is a syntax error at the zero-width gap where the block
/// would open: the next token's start, or the end of the clause's line when the body
/// ends there. An empty block stands in for recovery so the statement after it still
/// parses as a sibling.
#[test]
fn a_header_without_a_block_reports_the_gap_and_stands_an_empty_block() {
    for (source, line, column) in [
        ("module app\nfn f() {\n    if a\n    return\n}\n", 4, 5),
        ("module app\nfn f() {\n    while a\n    return\n}\n", 4, 5),
        (
            "module app\nfn f() {\n    for i in xs\n    return\n}\n",
            4,
            5,
        ),
        (
            "module app\nfn f() {\n    transaction\n    return\n}\n",
            4,
            5,
        ),
        ("module app\nfn f() {\n    match s\n    return\n}\n", 4, 5),
        ("module app\nfn f() {\n    match s\n}\n", 3, 12),
        (
            "module app\nfn f() {\n    if a {\n    } else if b\n    return\n}\n",
            5,
            5,
        ),
        ("module app\nfn f() {\n    if a {\n    } else\n}\n", 4, 11),
        (
            "module app\nfn f() {\n    match s {\n        dot =>\n    }\n}\n",
            5,
            5,
        ),
        ("module app\nfn f() {\n    const x = y else\n}\n", 3, 21),
        (
            "module app\nfn f() {\n    for a in b at most 8 {\n    } on more\n}\n",
            4,
            14,
        ),
        (
            "module app\nfn f() {\n    const p: int = checked a * b\n        on out_of_range\n}\n",
            4,
            24,
        ),
    ] {
        let parsed = parse_bounded(source);
        let diagnostics = parsed.diagnostics.complete();
        assert_eq!(diagnostics.len(), 1, "{source:?}: {diagnostics:#?}");
        let gap = &diagnostics[0];
        assert_eq!(
            gap.reason,
            DiagnosticReason::Parser(ParseDiagnosticReason::Expected(ExpectedSyntax::Block)),
            "{source:?}"
        );
        assert_eq!(
            gap.span.start_byte, gap.span.end_byte,
            "{source:?}: {gap:#?}"
        );
        assert_eq!(
            (gap.span.line, gap.span.column),
            (line, column),
            "{source:?}"
        );
        assert_eq!(
            gap.span.start_byte,
            offset_of(source, line, column),
            "{source:?}: the gap's byte and its line/column name the same point"
        );
    }

    let parsed = parse_bounded("module app\nfn f() {\n    if a\n    return\n}\n");
    let Some(Declaration::Function(function)) = parsed.file.declarations.first() else {
        panic!("the function still parses");
    };
    assert!(
        matches!(
            function.body.statements.as_slice(),
            [
                Statement::If { then_block, .. },
                Statement::Return { .. }
            ] if then_block.statements.is_empty()
        ),
        "{:#?}",
        function.body.statements
    );
}

/// The whole brace-grammar hostile corpus must never surface a diagnostic at the
/// invalid line-0/column-0 default span.
#[test]
fn no_parser_diagnostic_anchors_at_line_or_column_zero() {
    let corpus = [
        "module app\nresource B {\n    t: string\n",
        "module app\nenum E {\n    a\n    b\n",
        "module app\nresource B {\n    a{b\n}\n",
        "module app\nresource B {\n",
        "module app\nfn run() {\n    if const a = x and{\n        return\n    }\n}\n",
        "module app\nfn run() {\n    if const a = x and const b = y and{\n        return\n    }\n}\n",
        "module app\nfn run() {\n    match s {\n        dot =>\n    }\n}\n",
        "module app\nfn run() {\n    for i in {\n        return\n    }\n}\n",
        "module app\nfn run() {\n    return\n",
        "module app\nfn a() {\n    }\n}\nfn b() {\n    return\n}\n",
        "module app\nstore ^books[id: int]: B {\n    index byT[t]\n",
        "module app\nfn run(): Map<string, int> {\n    return\n}\n",
    ];
    for source in corpus {
        let parsed = parse_bounded(source);
        for d in parsed.diagnostics.complete() {
            assert!(
                d.span.line >= 1 && d.span.column >= 1,
                "diagnostic at line {} column {} (positions are 1-based) for {source:?}: {d:#?}",
                d.span.line,
                d.span.column
            );
        }
    }
}
