use marrow_syntax::{
    ArgMode, BinaryOp, Declaration, Expression, InterpolationPart, LiteralKind, ResourceMember,
    Statement, UnaryOp, parse_source,
};

fn reference_sample() -> &'static str {
    r#"module shelf::sample

resource Book at ^books(id: int)
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string
    tags(pos: int): string

    notes(noteId: string)
        text: string

    versions(version: int)
        required title: string
        required shelf: string
        required changedAt: instant

    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string, changedAt: instant): Book::Id
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    book.currentVersion = 1

    let id: Book::Id = nextId(^books)

    transaction
        ^books(id) = book
        ^books(id).versions(1).title = title
        ^books(id).versions(1).shelf = shelf
        ^books(id).versions(1).changedAt = changedAt

    return id

pub fn printShelf(shelf: string)
    for id in keys(^books.byShelf(shelf))
        print($"{id}: {^books(id).title}")
"#
}

#[test]
fn parses_documented_reference_sample() {
    let sample_doc = include_str!("../../../docs/language/sample.md");
    let sample = sample_doc
        .split("```mw")
        .nth(1)
        .and_then(|tail| tail.split("```").next())
        .expect("sample.md should contain a Marrow code block");

    let parsed = parse_source(sample);

    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics from docs/language/sample.md: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed
            .file
            .module
            .as_ref()
            .map(|module| module.name.as_str()),
        Some("shelf::sample")
    );
    assert!(parsed.file.resource("Book").is_some());
    assert!(parsed.file.function("main").is_some());
}

#[test]
fn parses_simple_statements_in_function_bodies() {
    let parsed = parse_source(
        "module app\n\
         fn main()\n\
         \x20   let title: string = \"Small Gods\"\n\
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
            Statement::Let { name, ty: Some(ty), value: Expression::Literal { .. }, .. }
                if name == "title" && ty.text == "string"
        ),
        "stmt 0: {:?}",
        statements[0]
    );
    assert!(
        matches!(
            &statements[1],
            Statement::Var { name, ty: Some(ty), value: Some(_), .. }
                if name == "count" && ty.text == "int"
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
fn parses_if_else_if_else_chain() {
    let parsed = parse_source(
        "module app\n\
         fn classify(n: int)\n\
         \x20   if n < 0\n\
         \x20       print(\"neg\")\n\
         \x20   else if n = 0\n\
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
            Expression::Binary {
                op: BinaryOp::Less,
                ..
            }
        ),
        "condition: {condition:?}"
    );
    assert_eq!(then_block.statements.len(), 1);
    assert_eq!(else_ifs.len(), 1);
    assert!(
        matches!(
            &else_ifs[0].condition,
            Expression::Binary {
                op: BinaryOp::Equal,
                ..
            }
        ),
        "else-if condition: {:?}",
        else_ifs[0].condition
    );
    assert!(else_block.is_some(), "expected else block");
    assert_eq!(else_block.as_ref().unwrap().statements.len(), 1);
}

#[test]
fn parses_transaction_and_lock_blocks() {
    let parsed = parse_source(
        "module app\n\
         fn commit(id: Book::Id)\n\
         \x20   lock ^books(id)\n\
         \x20       transaction\n\
         \x20           ^books(id).title = title\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let commit = parsed.file.function("commit").expect("commit function");
    assert_eq!(commit.body.statements.len(), 1);
    let Statement::Lock { path, body, .. } = &commit.body.statements[0] else {
        panic!(
            "expected lock statement, got {:?}",
            commit.body.statements[0]
        );
    };
    assert!(
        matches!(path, Expression::Call { .. }),
        "lock path should be ^books(id): {path:?}"
    );
    assert_eq!(body.statements.len(), 1);
    let Statement::Transaction { body: txn_body, .. } = &body.statements[0] else {
        panic!(
            "expected transaction inside lock, got {:?}",
            body.statements[0]
        );
    };
    assert_eq!(txn_body.statements.len(), 1);
    assert!(
        matches!(&txn_body.statements[0], Statement::Assign { .. }),
        "transaction body should hold the assignment: {:?}",
        txn_body.statements[0]
    );
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
        label: None,
        condition,
        body,
        ..
    } = &statements[0]
    else {
        panic!("expected while, got {:?}", statements[0]);
    };
    assert!(matches!(
        condition,
        Expression::Binary {
            op: BinaryOp::Less,
            ..
        }
    ));
    assert_eq!(body.statements.len(), 1);

    let Statement::For {
        label: None,
        binding,
        iterable,
        ..
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
fn parses_labeled_loops() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   outer: for id in keys(^books)\n\
         \x20       inner: while ready\n\
         \x20           break outer\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let Statement::For {
        label: Some(outer),
        body,
        ..
    } = &run.body.statements[0]
    else {
        panic!("expected labeled for, got {:?}", run.body.statements[0]);
    };
    assert_eq!(outer, "outer");
    let Statement::While {
        label: Some(inner),
        body: while_body,
        ..
    } = &body.statements[0]
    else {
        panic!("expected labeled while, got {:?}", body.statements[0]);
    };
    assert_eq!(inner, "inner");
    assert!(
        matches!(&while_body.statements[0], Statement::Break { label: Some(target), .. } if target == "outer"),
        "expected `break outer`, got {:?}",
        while_body.statements[0]
    );
}

#[test]
fn parses_try_catch_finally() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err: Error\n\
         \x20       print(err.message)\n\
         \x20   finally\n\
         \x20       cleanup()\n\
         \x20   return\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let statements = &run.body.statements;
    assert_eq!(statements.len(), 2, "{statements:#?}");
    let Statement::Try {
        body,
        catch,
        finally,
        ..
    } = &statements[0]
    else {
        panic!("expected try statement, got {:?}", statements[0]);
    };
    assert_eq!(body.statements.len(), 1);
    let catch = catch.as_ref().expect("catch clause");
    assert_eq!(catch.name, "err");
    assert_eq!(catch.ty.as_ref().map(|ty| ty.text.as_str()), Some("Error"));
    assert_eq!(catch.block.statements.len(), 1);
    let finally = finally.as_ref().expect("finally block");
    assert_eq!(finally.statements.len(), 1);
    assert!(
        matches!(&statements[1], Statement::Return { value: None, .. }),
        "sibling return should still parse: {:?}",
        statements[1]
    );
}

#[test]
fn parses_try_with_only_finally() {
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   finally\n\
         \x20       cleanup()\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let Statement::Try { catch, finally, .. } = &run.body.statements[0] else {
        panic!("expected try, got {:?}", run.body.statements[0]);
    };
    assert!(catch.is_none(), "expected no catch clause");
    assert!(finally.is_some(), "expected finally block");
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
fn nested_compound_at_end_of_body_parses_without_panic() {
    // The body ends with nested compound blocks, so every closing DEDENT lands
    // outside the body token slice. The block parser must tolerate that.
    let parsed = parse_source(
        "module app\n\
         fn run()\n\
         \x20   let ready = true\n\
         \x20   for id in keys(^books)\n\
         \x20       if ready\n\
         \x20           print(id)\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let run = parsed.file.function("run").expect("run function");
    let statements = &run.body.statements;
    assert_eq!(statements.len(), 2, "{statements:#?}");
    assert!(
        matches!(&statements[0], Statement::Let { name, .. } if name == "ready"),
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
fn statement_spanning_open_delimiters_stays_one_statement() {
    let parsed = parse_source(
        "module app\n\
         fn make()\n\
         \x20   throw Error(\n\
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
            Statement::Throw {
                value: Expression::Call { .. },
                ..
            }
        ),
        "stmt 0: {:?}",
        statements[0]
    );
}

#[test]
fn parses_reference_sample_outline() {
    let parsed = parse_source(reference_sample());

    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        parsed.diagnostics
    );
    assert_eq!(
        parsed.file.module.as_ref().map(|m| m.name.as_str()),
        Some("shelf::sample")
    );

    let book = parsed.file.resource("Book").expect("Book resource");
    let store = book.store.as_ref().expect("saved root");
    assert_eq!(store.root, "books");
    assert_eq!(store.keys[0].name, "id");
    assert_eq!(store.keys[0].ty.text, "int");

    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Field(field)
            if field.required && field.name == "title" && field.ty.text == "string"
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Field(field)
            if !field.required
                && field.name == "tags"
                && field.keys.len() == 1
                && field.ty.text == "string"
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Group(group)
            if group.name == "versions"
                && group.keys.len() == 1
                && group.members.iter().any(|child| matches!(
                    child,
                    ResourceMember::Field(field)
                        if field.required
                            && field.name == "changedAt"
                            && field.ty.text == "instant"
                ))
    )));
    assert!(book.members.iter().any(|member| matches!(
        member,
        ResourceMember::Index(index)
            if index.name == "byShelf"
                && index.args == ["shelf", "id"]
                && !index.unique
    )));

    let add = parsed.file.function("add").expect("add function");
    assert!(add.public);
    assert_eq!(
        add.params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        ["title", "author", "shelf", "changedAt"]
    );
    assert_eq!(
        add.return_type.as_ref().map(|ty| ty.text.as_str()),
        Some("Book::Id")
    );
}

#[test]
fn attaches_doc_comments_and_stable_ids_to_resource_members() {
    let parsed = parse_source(
        r#"module shelf::books

resource Book at ^books(id: int)
    ;; Display title.
    @id("book.title")
    required title: string
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let book = parsed.file.resource("Book").expect("Book resource");
    let ResourceMember::Field(title) = &book.members[0] else {
        panic!("expected field, got {:?}", book.members[0]);
    };
    assert_eq!(title.docs, ["Display title."]);
    assert_eq!(title.stable_id.as_deref(), Some("book.title"));
}

#[test]
fn rejects_tabs_because_marrow_blocks_are_space_indented() {
    let parsed = parse_source("module app\n\tpub fn main()\n");

    assert!(parsed.has_errors());
    assert_eq!(parsed.diagnostics[0].code, "parse.syntax");
    assert_eq!(parsed.diagnostics[0].line, 2);
    assert_eq!(parsed.diagnostics[0].column, 1);
    assert!(parsed.diagnostics[0].message.contains("tabs"));
    let tab_reports = parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("tabs"))
        .count();
    assert_eq!(tab_reports, 1, "{:#?}", parsed.diagnostics);
}

#[test]
fn surfaces_lexer_diagnostics_for_function_body_tokens() {
    let parsed = parse_source("module app\nfn main()\n    return a == b\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let obsolete = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`==`"))
        .expect("expected obsolete operator diagnostic");
    assert_eq!(obsolete.code, "parse.syntax");
    assert_eq!(obsolete.kind, "parse");
    assert_eq!(obsolete.line, 3);
    assert_eq!(
        obsolete.help.as_deref(),
        Some("Use `=` for equality."),
        "{:#?}",
        obsolete.help
    );
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
        (
            // Quoted field segments are not decomposed by the parser yet.
            "const Old = ^books(id).\"old-title\"\n",
            Expectation::Unparsed("^books(id).\"old-title\""),
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
                Expression::Literal { kind, text, .. },
            ) => {
                assert_eq!(*kind, *expected_kind, "{source:?}");
                assert_eq!(text, expected_text, "{source:?}");
            }
            (Expectation::Name(expected_segments), Expression::Name { segments, .. }) => {
                assert_eq!(segments.as_slice(), *expected_segments, "{source:?}");
            }
            (Expectation::Unparsed(expected_text), Expression::Unparsed { text, .. }) => {
                assert_eq!(text, expected_text, "{source:?}");
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
    let Expression::Binary {
        op, left, right, ..
    } = &decl.value
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
    let Expression::Unary { op, operand, .. } = &decl.value else {
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
fn parses_interpolation_into_text_and_expression_parts() {
    let parsed = parse_source("const Label: string = $\"book {id}: {{ready}}\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Expression::Interpolation { parts, .. } = &decl.value else {
        panic!("expected interpolation, got {:?}", decl.value);
    };
    assert_eq!(parts.len(), 3, "{parts:#?}");
    assert!(
        matches!(&parts[0], InterpolationPart::Text { text, .. } if text == "book "),
        "part 0: {:?}",
        parts[0]
    );
    assert!(
        matches!(
            &parts[1],
            InterpolationPart::Expr(Expression::Name { segments, .. }) if segments == &["id"]
        ),
        "part 1: {:?}",
        parts[1]
    );
    // `{{ready}}` stays escaped inside literal text.
    assert!(
        matches!(&parts[2], InterpolationPart::Text { text, .. } if text == ": {{ready}}"),
        "part 2: {:?}",
        parts[2]
    );
}

#[test]
fn parses_interpolation_with_embedded_call_path() {
    // From the reference sample: $"{id}: {^books(id).title}".
    let parsed = parse_source("const Line: string = $\"{id}: {^books(id).title}\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Expression::Interpolation { parts, .. } = &decl.value else {
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
            Some(InterpolationPart::Expr(Expression::Field { name, .. })) if name == "title"
        ),
        "last embedded expr should be a field access: {parts:#?}"
    );
}

#[test]
fn parses_calls_paths_and_field_access() {
    // `^books(id).title` is SavedRoot -> Call -> Field.
    let parsed = parse_source("const Title = ^books(id).title\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Expression::Field { base, name, .. } = &decl.value else {
        panic!("expected field access, got {:?}", decl.value);
    };
    assert_eq!(name, "title");
    let Expression::Call { callee, args, .. } = base.as_ref() else {
        panic!("expected call under field, got {base:?}");
    };
    assert_eq!(args.len(), 1);
    assert!(
        matches!(callee.as_ref(), Expression::SavedRoot { name, .. } if name == "books"),
        "expected saved root callee, got {callee:?}"
    );
    assert!(
        matches!(&args[0].value, Expression::Name { segments, .. } if segments == &["id"]),
        "expected id argument, got {:?}",
        args[0].value
    );
}

#[test]
fn parses_named_and_moded_call_arguments() {
    let parsed = parse_source("const Made = save(book: draft, out result, inout total)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Expression::Call { args, .. } = &decl.value else {
        panic!("expected call, got {:?}", decl.value);
    };
    assert_eq!(args.len(), 3);
    assert_eq!(args[0].name.as_deref(), Some("book"));
    assert_eq!(args[0].mode, None);
    assert_eq!(args[1].mode, Some(ArgMode::Out));
    assert_eq!(args[1].name, None);
    assert_eq!(args[2].mode, Some(ArgMode::InOut));
}

#[test]
fn parses_conversion_and_constructor_calls() {
    // Conversion call on a type keyword.
    let parsed = parse_source("const Count: int = int(raw)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Expression::Call { callee, .. } = &decl.value else {
        panic!("expected conversion call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if segments == &["int"]),
        "expected int callee, got {callee:?}"
    );

    // Generated identity constructor `Book::Id(17)`.
    let parsed = parse_source("const First = Book::Id(17)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Expression::Call { callee, args, .. } = &decl.value else {
        panic!("expected constructor call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if segments == &["Book", "Id"]),
        "expected Book::Id callee, got {callee:?}"
    );
    assert_eq!(args.len(), 1);
}

#[test]
fn bare_type_keyword_is_not_a_value() {
    // `int` alone is a type, not an expression, so it does not parse as a value.
    let parsed = parse_source("const Bad = int\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        matches!(decl.value, Expression::Unparsed { .. }),
        "expected bare `int` to be Unparsed, got {:?}",
        decl.value
    );
}

#[test]
fn const_chained_equality_is_not_associative() {
    // Grammar: equality is non-associative, so `a = b = c` is not a valid
    // expression. The parser consumes `a = b` then leaves `= c`, so the value
    // falls back to Unparsed rather than silently nesting.
    let parsed = parse_source("const Bad: bool = a = b = c\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        matches!(decl.value, Expression::Unparsed { .. }),
        "expected chained equality to be Unparsed, got {:?}",
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
    let span = decl.value.span();
    assert_eq!(&source[span.start_byte..span.end_byte], "60 * 60");
}

#[test]
fn const_expression_span_points_into_source() {
    let source = "const Max: int = 5\n";
    let parsed = parse_source(source);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let span = decl.value.span();
    assert_eq!(&source[span.start_byte..span.end_byte], "5");
    assert_eq!(span.line, 1);
    assert_eq!(span.column, 18);
}

#[derive(Debug)]
enum Expectation<'a> {
    Literal(LiteralKind, &'a str),
    Name(&'a [&'a str]),
    Unparsed(&'a str),
}

#[test]
fn rejects_parameter_defaults() {
    let parsed = parse_source("module app\nfn f(x: int = 5)\n    return\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("parameter defaults"))
        .expect("expected parameter-defaults diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.kind, "parse");
    assert_eq!(diagnostic.line, 2);
    assert!(
        !diagnostic.message.contains("expected"),
        "diagnostic should not fall back to a generic message, got {:?}",
        diagnostic.message
    );
}

#[test]
fn rejects_user_defined_generics_on_functions() {
    let parsed = parse_source("module app\nfn f<T>(x: T)\n    return\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("user-defined generics"))
        .expect("expected user-defined-generics diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.line, 2);
}

#[test]
fn rejects_top_level_type_aliases() {
    let parsed = parse_source("module app\ntype Title = string\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("type aliases"))
        .expect("expected type-aliases diagnostic");
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.line, 2);
}

#[test]
fn merges_lexer_and_parser_diagnostics_in_source_order() {
    let parsed = parse_source(concat!(
        "module ;-bad-name\n",
        "fn main()\n",
        "    return ~~~\n",
    ));

    assert!(parsed.has_errors());
    let mut lines = parsed
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.line)
        .collect::<Vec<_>>();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted, "diagnostics not in source order: {lines:?}");
    lines.dedup();
    assert!(
        lines.contains(&1) && lines.contains(&3),
        "expected diagnostics on lines 1 and 3, saw {lines:?}"
    );
}

#[test]
fn rejects_internal_and_private_visibility() {
    for visibility in ["internal", "private"] {
        let parsed = parse_source(&format!("module app\n{visibility} fn main()\n    return\n"));

        assert!(parsed.has_errors(), "expected error for {visibility}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("pub")
                    && diagnostic.message.contains("module-private")),
            "diagnostics for {visibility}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn requires_indented_resource_and_function_bodies() {
    let parsed = parse_source(
        r#"module app
resource Book at ^books(id: int)
pub fn main()
"#,
    );

    assert_eq!(parsed.diagnostics.len(), 2, "{:#?}", parsed.diagnostics);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("resource body"))
    );
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body"))
    );
}

#[test]
fn rejects_resource_members_nested_under_fields() {
    let parsed = parse_source(
        r#"module app
resource Book
    title: string
        nested: string
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unexpected indentation")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_empty_saved_root_key_lists() {
    let parsed = parse_source(
        r#"module app
resource Book at ^books()
    title: string
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("key")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_empty_index_argument_lists() {
    let parsed = parse_source(
        r#"module app
resource Book at ^books(id: int)
    title: string
    index empty()
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("index argument")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_const_without_value() {
    let parsed = parse_source(
        r#"module app
const MaxLoans: int
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("const")
                && diagnostic.message.contains("=")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_module_names() {
    let parsed = parse_source("module 123\n");

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("module name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_import_names() {
    let parsed = parse_source(
        r#"module app
use *
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("import name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_invalid_const_names() {
    let parsed = parse_source(
        r#"module app
const : int = 1
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("const name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_malformed_type_annotations() {
    for source in [
        "module app\nconst Max: = 1\n",
        "module app\nfn main(value:)\n    return\n",
        "module app\nresource Book at ^books(id:)\n    title: string\n",
        "module app\nresource Book\n    title: sequence[]\n",
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("type")),
            "diagnostics for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_malformed_index_field_paths() {
    for source in [
        "module app\nresource Book at ^books(id: int)\n    title: string\n    index bad(title.)\n",
        "module app\nresource Book at ^books(id: int)\n    title: string\n    index bad(.title)\n",
        "module app\nresource Book at ^books(id: int)\n    title: string\n    index bad(title.*)\n",
    ] {
        let parsed = parse_source(source);

        assert!(parsed.has_errors(), "expected error for:\n{source}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("index field path")),
            "diagnostics for {source}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_late_or_duplicate_module_declarations() {
    let parsed = parse_source(
        r#"module app
fn main()
    return
module later
"#,
    );

    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("module declaration")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn keeps_top_level_declarations_in_source_order() {
    let parsed = parse_source(
        r#"module app
const MaxLoans: int = 5
resource Book
    title: string
fn normalize(title: string): string
    return title
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let names = parsed
        .file
        .declarations
        .iter()
        .map(|decl| match decl {
            Declaration::Const(decl) => decl.name.as_str(),
            Declaration::Resource(decl) => decl.name.as_str(),
            Declaration::Function(decl) => decl.name.as_str(),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["MaxLoans", "Book", "normalize"]);
}
