//! Control-flow statements: conditionals, loops, error
//! handling, and match arms, with the body and indentation rules each enforces.

use crate::common;
use common::parse_reason;
use marrow_syntax::{
    BinaryOp, ExpectedSyntax, Expression, ParseDiagnosticReason, Statement, parse_source,
};

#[test]
fn parses_a_range_for_by_step() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   for i in 1..10 by 2\n\
         \x20       print($\"{i}\")\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let Statement::For { iterable, step, .. } = &run.body.statements[0] else {
        panic!("expected for, got {:?}", run.body.statements[0]);
    };
    assert!(
        matches!(
            iterable,
            Expression::Binary {
                op: BinaryOp::RangeExclusive,
                ..
            }
        ),
        "{iterable:?}"
    );
    let Some(Expression::Literal { text, .. }) = step.as_ref() else {
        panic!("expected an integer step literal, got {step:?}");
    };
    assert_eq!(text, "2");
}

#[test]
fn a_range_for_without_by_has_no_step() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   for i in 1..10\n\
         \x20       print($\"{i}\")\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let Statement::For { step, .. } = &run.body.statements[0] else {
        panic!("expected for, got {:?}", run.body.statements[0]);
    };
    assert_eq!(*step, None);
}

#[test]
fn parses_if_else_if_else_chain() {
    let parsed = parse_source(
        "module app\n\
         fn classify(n: int)\n\
         \x20   if n < 0\n\
         \x20       print(\"neg\")\n\
         \x20   else if n == 0\n\
         \x20       print(\"zero\")\n\
         \x20   else\n\
         \x20       print(\"pos\")\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let classify = parsed.file.function("classify").expect("classify function");
    assert_eq!(classify.body.statements.len(), 1);
    let Statement::If {
        condition,
        then_block,
        else_ifs,
        else_block,
        ..
    } = &classify.body.statements[0]
    else {
        panic!(
            "expected if statement, got {:?}",
            classify.body.statements[0]
        );
    };
    assert!(
        matches!(
            condition,
            Some(Expression::Binary {
                op: BinaryOp::Less,
                ..
            })
        ),
        "condition: {condition:?}"
    );
    assert_eq!(then_block.statements.len(), 1);
    assert_eq!(else_ifs.len(), 1);
    assert!(
        matches!(
            &else_ifs[0].condition,
            Some(Expression::Binary {
                op: BinaryOp::Equal,
                ..
            })
        ),
        "else-if condition: {:?}",
        else_ifs[0].condition
    );
    assert!(else_block.is_some(), "expected else block");
    assert_eq!(else_block.as_ref().unwrap().statements.len(), 1);
}

#[test]
fn parses_if_const_binding_guard() {
    let parsed = parse_source(
        "module app\n\
         fn title(id: Id(^books))\n\
         \x20   if const title = ^books(id).title\n\
         \x20       print(title)\n\
         \x20   else\n\
         \x20       print(\"missing\")\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let title = parsed.file.function("title").expect("title function");
    let Statement::IfConst {
        name,
        value,
        then_block,
        else_block,
        ..
    } = &title.body.statements[0]
    else {
        panic!("expected if statement, got {:?}", title.body.statements[0]);
    };
    assert_eq!(name, "title");
    assert!(
        matches!(value, Expression::Field { name, .. } if name == "title"),
        "binding value: {value:?}"
    );
    assert_eq!(then_block.statements.len(), 1);
    assert!(else_block.is_some(), "expected else block");
}

#[test]
fn parses_nested_if_inside_then_block() {
    let parsed = parse_source(
        "module app\n\
         fn check(a: bool, b: bool)\n\
         \x20   if a\n\
         \x20       if b\n\
         \x20           print(\"both\")\n\
         \x20   return\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let check = parsed.file.function("check").expect("check function");
    assert_eq!(
        check.body.statements.len(),
        2,
        "{:#?}",
        check.body.statements
    );
    let Statement::If { then_block, .. } = &check.body.statements[0] else {
        panic!("expected outer if, got {:?}", check.body.statements[0]);
    };
    assert_eq!(then_block.statements.len(), 1);
    assert!(
        matches!(&then_block.statements[0], Statement::If { .. }),
        "inner statement should be an if: {:?}",
        then_block.statements[0]
    );
    assert!(
        matches!(
            &check.body.statements[1],
            Statement::Return { value: None, .. }
        ),
        "trailing return: {:?}",
        check.body.statements[1]
    );
}

#[test]
fn parses_while_and_for_loops() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   while n < 10\n\
         \x20       n = n + 1\n\
         \x20   for id in keys(^books)\n\
         \x20       print(id)\n\
         \x20   for shelf, id in entries(^books.byShelf)\n\
         \x20       print(id)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let statements = &run.body.statements;
    assert_eq!(statements.len(), 3, "{statements:#?}");

    let Statement::While {
        condition, body, ..
    } = &statements[0]
    else {
        panic!("expected while, got {:?}", statements[0]);
    };
    assert!(matches!(
        condition,
        Some(Expression::Binary {
            op: BinaryOp::Less,
            ..
        })
    ));
    assert_eq!(body.statements.len(), 1);

    let Statement::For {
        binding, iterable, ..
    } = &statements[1]
    else {
        panic!("expected for, got {:?}", statements[1]);
    };
    assert_eq!(binding.first, "id");
    assert_eq!(binding.second, None);
    assert!(matches!(iterable, Expression::Call { .. }));

    let Statement::For { binding, .. } = &statements[2] else {
        panic!("expected paired for, got {:?}", statements[2]);
    };
    assert_eq!(binding.first, "shelf");
    assert_eq!(binding.second.as_deref(), Some("id"));
}

#[test]
fn loop_labels_are_rejected_as_removed_syntax() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   outer: for id in keys(^books)\n\
         \x20       inner: while ready\n\
         \x20           break outer\n",
    );
    assert!(parsed.has_errors(), "expected loop-label rejection");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("loop labels were removed")
            && diagnostic
                .help
                .as_deref()
                .is_some_and(|help| help.contains("extract a function"))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn labeled_break_and_continue_are_rejected_as_removed_syntax() {
    for source in [
        "module app\nfn run()\n    while ready\n        break outer\n",
        "module app\nfn run()\n    while ready\n        continue outer\n",
    ] {
        let parsed = parse_source(source);
        assert!(parsed.has_errors(), "expected labeled jump rejection");
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("loop labels were removed")
                && diagnostic
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("extract a function"))),
            "{:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn parses_try_catch() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err: Error\n\
         \x20       print(err.message)\n\
         \x20   return\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let statements = &run.body.statements;
    assert_eq!(statements.len(), 2, "{statements:#?}");
    let Statement::Try { body, catch, .. } = &statements[0] else {
        panic!("expected try statement, got {:?}", statements[0]);
    };
    assert_eq!(body.statements.len(), 1);
    let catch = catch.as_ref().expect("catch clause");
    assert_eq!(catch.name, "err");
    assert_eq!(catch.ty.as_ref().map(|ty| ty.text.as_str()), Some("Error"));
    assert_eq!(catch.block.statements.len(), 1);
    assert!(
        matches!(&statements[1], Statement::Return { value: None, .. }),
        "sibling return should still parse: {:?}",
        statements[1]
    );
}

#[test]
fn try_finally_is_rejected_as_removed_syntax() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   finally\n\
         \x20       cleanup()\n",
    );
    assert!(parsed.has_errors(), "expected finally rejection");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("`try` requires a `catch` clause")
            && diagnostic
                .help
                .as_deref()
                .is_some_and(|help| help.contains("catch, clean up, then rethrow"))),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn try_without_catch_is_rejected() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   return\n",
    );
    assert!(parsed.has_errors(), "expected no-catch try rejection");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("`try` requires a `catch` clause")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_try_catch_without_type_annotation() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err\n\
         \x20       print(err.message)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let Statement::Try { catch, .. } = &run.body.statements[0] else {
        panic!("expected try, got {:?}", run.body.statements[0]);
    };
    let catch = catch.as_ref().expect("catch clause");
    assert_eq!(catch.name, "err");
    assert_eq!(catch.ty, None);
}

#[test]
fn catch_rejects_structural_equal_inside_type_annotation() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       return\n\
         \x20   catch err: Error = 1\n\
         \x20       return\n",
    );

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "parse.syntax"
                && diagnostic.reason
                    == parse_reason(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::ParameterType,
                    ))
        }),
        "{:#?}",
        parsed.diagnostics
    );
}

/// Panic guard for the DEDENT-out-of-slice edge: a body that ends in nested
/// compound blocks closes every DEDENT past the body's token slice. The structure
/// asserted below is the minimum that proves no recovery swallowed the nesting,
/// not a fresh contract for `for`/`if` nesting (the focused tests above own that).
#[test]
fn nested_compound_at_end_of_body_parses_without_panic() {
    // The body ends with nested compound blocks, so every closing DEDENT lands
    // outside the body token slice. The block parser must tolerate that.
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   const ready = true\n\
         \x20   for id in keys(^books)\n\
         \x20       if ready\n\
         \x20           print(id)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let statements = &run.body.statements;
    assert_eq!(statements.len(), 2, "{statements:#?}");
    assert!(
        matches!(&statements[0], Statement::Const { name, .. } if name == "ready"),
        "stmt 0: {:?}",
        statements[0]
    );
    let Statement::For { body, .. } = &statements[1] else {
        panic!("stmt 1 should be the for-loop: {:?}", statements[1]);
    };
    assert!(
        matches!(&body.statements[0], Statement::If { .. }),
        "for body should hold the nested if: {:?}",
        body.statements[0]
    );
}

#[test]
fn malformed_while_condition_reports_a_parse_error() {
    // A `while` header that does not parse as a complete expression is a parse
    // error: the grammar requires `while_stmt = "while" expression NEWLINE block`.
    let parsed = parse_source("fn f()\n    while a == b == c\n        return\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error for the malformed `while` condition: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_a_match_statement_with_bare_member_arms() {
    let parsed = parse_source(
        "module app\n\
         fn f(s: Status)\n    \
         match s\n        active\n            print(\"a\")\n        \
         archived\n            print(\"b\")\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("f");
    let Statement::Match {
        scrutinee, arms, ..
    } = &f.body.statements[0]
    else {
        panic!("expected a match, got {:?}", f.body.statements[0]);
    };
    assert!(matches!(scrutinee, Some(Expression::Name { .. })));
    let paths: Vec<Vec<&str>> = arms
        .iter()
        .map(|arm| arm.path.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(paths, [vec!["active"], vec!["archived"]]);
    // Each arm carries its own block.
    assert_eq!(arms[0].block.statements.len(), 1);
}

#[test]
fn parses_a_match_arm_that_is_a_qualified_member_path() {
    // A qualified arm `tiger::bengal` and a category arm `lion` parse into their
    // relative `::`-separated segments; the scrutinee supplies the enum.
    let parsed = parse_source(
        "module app\n\
         fn f(c: Cat)\n    \
         match c\n        tiger::bengal\n            print(\"a\")\n        \
         lion\n            print(\"b\")\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let f = parsed.file.function("f").expect("f");
    let Statement::Match { arms, .. } = &f.body.statements[0] else {
        panic!("expected a match, got {:?}", f.body.statements[0]);
    };
    let paths: Vec<Vec<&str>> = arms
        .iter()
        .map(|arm| arm.path.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(paths, [vec!["tiger", "bengal"], vec!["lion"]]);
}

#[test]
fn rejects_a_match_arm_that_is_not_a_member_path() {
    let parsed = parse_source(
        "module app\n\
         fn f(s: Status)\n    \
         match s\n        active: int\n            print(\"a\")\n",
    );
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.reason == parse_reason(ParseDiagnosticReason::MatchArmMemberPath)),
        "{:#?}",
        parsed.diagnostics
    );
}
