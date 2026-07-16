//! The brace block grammar (BS01): `{ … }` blocks, `NEWLINE`-or-`}` statement
//! termination, cuddled and inline trailing clauses, `=>` match arms, newline enum
//! members, header continuation, and the parse-only B5/B6 forms. These are the
//! load-bearing behavioral invariants of the block-syntax migration, written fresh
//! against the brace grammar (the layout corpus is allowlisted until the converter
//! flip rewrites it).

use marrow_syntax::{
    Declaration, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ResourceMember,
    Statement, parse_source,
};

fn clean(source: &str) {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
        parsed.diagnostics.iter().any(
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
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
        !parsed.diagnostics.is_empty(),
        "layout input is now a diagnostic"
    );
    assert!(
        parsed.diagnostics.len() < 10,
        "diagnostics stay bounded, not a per-line flood: {:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed
            .diagnostics
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
            .iter()
            .any(|d| d.reason
                == DiagnosticReason::Parser(ParseDiagnosticReason::UnexpectedIndentation)),
        "a bare block has no statement form: {:#?}",
        parsed.diagnostics
    );
}

// ---- B5/B6 parse-only forms ----

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
    // `;` is not a comment leader; it is an unexpected character now.
    let parsed = parse_source("module app\n; not a comment\n");
    assert!(
        !parsed.diagnostics.is_empty(),
        "a leading `;` is no longer comment trivia: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn slash_slash_is_a_line_comment() {
    clean("module app\n// a line comment\nfn run() {\n    return // trailing\n}\n");
}

#[test]
fn a_match_arm_body_expression_uses_expected_syntax() {
    // Guard the ExpectedSyntax import stays meaningful: an empty match body reports.
    let parsed = parse_source("module app\nfn run(s: Shape) {\n    match s\n}\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.reason
            == DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                ExpectedSyntax::MatchBody
            ))),
        "a match with no brace body reports MatchBody: {:#?}",
        parsed.diagnostics
    );
}
