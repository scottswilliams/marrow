use marrow_syntax::{
    ArgMode, BinaryOp, Declaration, Expression, InterpolationPart, LiteralKind, ResourceMember,
    Statement, UnaryOp, format_expression, parse_source,
};

fn member_names(decl: &marrow_syntax::EnumDecl) -> Vec<&str> {
    decl.members.iter().map(|m| m.name.as_str()).collect()
}

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

pub fn add(title: string, author: string, shelf: string, changedAt: instant): Id(^books)
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    book.currentVersion = 1

    const id: Id(^books) = nextId(^books)

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
fn parses_split_store_declaration() {
    let parsed = parse_source(
        "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.resource("Book").is_some());
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(store.root.root, "books");
    assert_eq!(store.root.keys.len(), 1);
    assert_eq!(store.root.keys[0].name, "id");
    assert_eq!(store.root.keys[0].ty.text, "int");
    assert_eq!(store.resource, "Book");
}

#[test]
fn concise_resource_at_desugars_to_resource_and_store() {
    let parsed = parse_source(
        "module app\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   shelf: string\n\
         \x20   index byShelf(shelf, id)\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(parsed.file.declarations.len(), 2);
    let resource = parsed.file.resource("Book").expect("Book resource");
    assert_eq!(resource.members.len(), 2);
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(store.resource, "Book");
    assert_eq!(store.indexes.len(), 1);
    assert_eq!(store.indexes[0].name, "byShelf");
}

#[test]
fn split_resource_body_rejects_index_members() {
    let parsed = parse_source(
        "module app\n\
         resource Book\n\
         \x20   title: string\n\
         \x20   index byTitle(title)\n\
         store ^books(id: int): Book\n",
    );

    assert!(parsed.has_errors(), "expected parse rejection");
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("store body")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_adr_0209_tilde_roots() {
    for source in [
        "module app\ncache ~books(id: int): Book\n",
        "module app\nensure ~books(id: int): Book\n",
        "module app\nresource Book\n    author: Id(~authors)\n",
        "module app\n~scratch(id: int): Book\n",
    ] {
        let parsed = parse_source(source);
        assert!(parsed.has_errors(), "expected rejection for:\n{source}");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "parse.syntax"),
            "{:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn parses_all_documented_module_files() {
    // Every fenced `mw` block that opens with `module` is a complete library
    // file and must parse without diagnostics. Signature-only and fragment
    // examples (bare statements, body-less functions) are illustrative and not
    // included here; the lexer fixture covers all blocks.
    let blocks = documented_module_blocks();
    assert!(
        blocks.len() >= 5,
        "expected several documented module files, found {}",
        blocks.len()
    );
    for block in blocks {
        let parsed = parse_source(&block.source);
        assert!(
            parsed.diagnostics.is_empty(),
            "{}#{} should parse cleanly, got:\n{:#?}\n--- source ---\n{}",
            block.path,
            block.index,
            parsed.diagnostics,
            block.source
        );
    }
}

struct MwBlock {
    path: String,
    index: usize,
    source: String,
}

fn documented_module_blocks() -> Vec<MwBlock> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language");
    let mut blocks = Vec::new();
    let mut entries = std::fs::read_dir(&dir)
        .expect("read docs/language")
        .map(|entry| entry.expect("language doc entry").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read language doc");
        let mut in_block = false;
        let mut index = 0usize;
        let mut source = String::new();
        for line in text.lines() {
            if line.trim() == "```mw" {
                in_block = true;
                index += 1;
                source.clear();
                continue;
            }
            if line.trim() == "```" && in_block {
                if source.trim_start().starts_with("module ") {
                    blocks.push(MwBlock {
                        path: path.file_name().unwrap().to_string_lossy().into_owned(),
                        index,
                        source: source.clone(),
                    });
                }
                in_block = false;
                continue;
            }
            if in_block {
                source.push_str(line);
                source.push('\n');
            }
        }
    }
    blocks
}

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
    assert_eq!(keys[0].ty.text, "string");
    assert_eq!(ty.as_ref().map(|t| t.text.as_str()), Some("int"));
    assert_eq!(*value, None);
}

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
    assert_eq!(ty.as_ref().map(|t| t.text.as_str()), Some("bool"));
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
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "parse.syntax" && diagnostic.message.contains("`lock` is reserved")
        }),
        "{:#?}",
        parsed.diagnostics
    );
    assert!(
        !parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("expected a statement")),
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
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "parse.syntax" && diagnostic.message.contains("`merge` is reserved")
        }),
        "{:#?}",
        parsed.diagnostics
    );
    let commit = parsed.file.function("commit").expect("commit function");
    assert_eq!(commit.body.statements.len(), 1, "{commit:#?}");
    assert!(
        matches!(&commit.body.statements[0], Statement::Expr { .. }),
        "{:#?}",
        commit.body.statements[0]
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
        Some(Expression::Binary {
            op: BinaryOp::Less,
            ..
        })
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
fn parses_reference_sample_structure() {
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
    let store = parsed.file.store("books").expect("books store");
    assert_eq!(store.root.root, "books");
    assert_eq!(store.root.keys[0].name, "id");
    assert_eq!(store.root.keys[0].ty.text, "int");
    assert_eq!(store.resource, "Book");

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
    assert!(
        store
            .indexes
            .iter()
            .any(|index| index.name == "byShelf" && index.args == ["shelf", "id"] && !index.unique)
    );

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
        Some("Id(^books)")
    );
}

#[test]
fn attaches_doc_comments_to_resource_members() {
    let parsed = parse_source(
        r#"module shelf::books

resource Book at ^books(id: int)
    ;; Display title.
    required title: string
"#,
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let book = parsed.file.resource("Book").expect("Book resource");
    let ResourceMember::Field(title) = &book.members[0] else {
        panic!("expected field, got {:?}", book.members[0]);
    };
    assert_eq!(title.docs, ["Display title."]);
}

#[test]
fn rejects_tabs_because_marrow_blocks_are_space_indented() {
    let parsed = parse_source("module app\n\tpub fn main()\n");

    assert!(parsed.has_errors());
    assert_eq!(parsed.diagnostics[0].code, "parse.syntax");
    assert_eq!(parsed.diagnostics[0].span.line, 2);
    assert_eq!(parsed.diagnostics[0].span.column, 1);
    assert!(parsed.diagnostics[0].message.contains("tabs"));
    let tab_reports = parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("tabs"))
        .count();
    assert_eq!(tab_reports, 1, "{:#?}", parsed.diagnostics);
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
        assert_eq!(syntax.kind, "parse", "{source:?}");
    }
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
                && diagnostic.message.contains("unexpected indentation")),
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
fn parses_trailing_comments_on_declaration_lines() {
    let parsed = parse_source(
        "module app ; module comment\n\
         use std::math ; use comment\n\
         const Max: int = 5 ; const comment\n\
         resource Book at ^books(id: int) ; resource comment\n\
         \x20   title: string ; field comment\n\
         \x20   notes(noteId: string) ; group comment\n\
         \x20       text: string ; nested field comment\n\
         \x20   index byTitle(title) ; index comment\n\
         enum Status ; enum comment\n\
         \x20   active ; member comment\n\
         fn main() ; function comment\n\
         \x20   return ; statement comment\n",
    );

    assert!(
        parsed.diagnostics.is_empty(),
        "declaration trailing comments should be trivia: {:#?}",
        parsed.diagnostics
    );
    assert!(parsed.file.resource("Book").is_some());
    assert!(parsed.file.enum_decl("Status").is_some());
    assert!(parsed.file.function("main").is_some());
}

#[test]
fn surfaces_lexer_diagnostics_for_function_body_tokens() {
    let parsed = parse_source("module app\nfn main()\n    return a && b\n");

    assert!(parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let obsolete = parsed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`&&`"))
        .expect("expected obsolete operator diagnostic");
    assert_eq!(obsolete.code, "parse.syntax");
    assert_eq!(obsolete.kind, "parse");
    assert_eq!(obsolete.span.line, 3);
    assert_eq!(
        obsolete.help.as_deref(),
        Some("Use `and` for boolean and."),
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
fn parses_top_level_multi_line_const_value() {
    // A column-0 `const` whose value spans several physical lines inside open
    // delimiters must parse as one call, not break apart line by line.
    let source = "const id = some::call(\n  a: 1,\n  b: 2,\n)\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
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
    assert_eq!(segments.as_slice(), &["some", "call"]);
    assert_eq!(args.len(), 2, "expected two arguments");
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
fn parses_interpolation_into_text_and_expression_parts() {
    let parsed = parse_source("const Label: string = $\"book {id}: {{ready}}\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Interpolation { parts, .. }) = &decl.value else {
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
    let Some(Expression::Field { base, name, .. }) = &decl.value else {
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
fn parses_quoted_field_segments() {
    let parsed = parse_source("const Old = ^books(id).\"old-title\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Field {
        name, quoted, base, ..
    }) = &decl.value
    else {
        panic!("expected field access, got {:?}", decl.value);
    };
    assert_eq!(name, "old-title");
    assert!(*quoted, "segment should be marked quoted");
    assert!(
        matches!(base.as_ref(), Expression::Call { .. }),
        "base should be ^books(id): {base:?}"
    );

    // A plain identifier field is not quoted.
    let parsed = parse_source("const Title = book.title\n");
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        matches!(&decl.value, Some(Expression::Field { name, quoted: false, .. }) if name == "title"),
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
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unterminated string")),
        "expected an unterminated-string diagnostic: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn keyword_field_name_reports_a_parse_error() {
    // `at` is a reserved word (`resource X at ^root`). Used as a bare field
    // name it violates `field_name = identifier | string_lit`, so the parser
    // must report it rather than silently dropping the statement.
    let source = "fn touch(id: int)\n    ^events(id).at = now\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|d| d.message.contains("keyword") && d.message.contains("field name"))
        .unwrap_or_else(|| {
            panic!(
                "expected a keyword field-name diagnostic: {:#?}",
                parsed.diagnostics
            )
        });
    // The diagnostic points at the offending `.at`.
    assert_eq!(
        &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
        ".at"
    );
}

#[test]
fn keyword_field_name_reports_once_not_also_expected_a_statement() {
    // A line that fails because of a keyword field name carries the specific
    // diagnostic only: the generic "expected a statement" fallback must not also
    // fire on the same line.
    let source = "fn touch(id: int)\n    ^events(id).at = now\n";
    let parsed = parse_source(source);
    let on_offending_line: Vec<_> = parsed
        .diagnostics
        .iter()
        .filter(|d| d.span.line == 2)
        .collect();
    assert_eq!(
        on_offending_line.len(),
        1,
        "the keyword-field line should report exactly once: {on_offending_line:#?}"
    );
    assert!(
        on_offending_line[0].message.contains("field name"),
        "{:#?}",
        on_offending_line[0]
    );
}

#[test]
fn quoted_keyword_field_name_parses() {
    // A reserved word can name data by quoting it after the dot. `."at"` is a
    // string-literal field segment, so it parses cleanly.
    let parsed = parse_source("const At = ^events(id).\"at\"\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    assert!(
        matches!(&decl.value, Some(Expression::Field { name, quoted: true, .. }) if name == "at"),
        "expected quoted field `at`, got {:?}",
        decl.value
    );
}

#[test]
fn parses_named_and_inout_call_arguments() {
    let parsed = parse_source("const Made = save(book: draft, inout total)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { args, .. }) = &decl.value else {
        panic!("expected call, got {:?}", decl.value);
    };
    assert_eq!(args.len(), 2);
    assert_eq!(args[0].name.as_deref(), Some("book"));
    assert_eq!(args[0].mode, None);
    assert_eq!(args[1].mode, Some(ArgMode::InOut));
    assert_eq!(args[1].name, None);
}

#[test]
fn rejects_out_call_argument_as_reserved_surface() {
    let parsed = parse_source("const Made = save(book: draft, out result)\n");
    assert!(parsed.has_errors(), "expected out call-argument rejection");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "parse.syntax" && diagnostic.message.contains("`out` is reserved")
        }),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn positional_argument_after_named_is_rejected() {
    // After the first named argument, every remaining argument must be named.
    // A plain positional argument after a named one is a parse error that points
    // at the offending argument.
    let source = "const Made = sub(b: 1, 2)\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|d| {
            d.message
                .contains("positional argument cannot follow a named argument")
        })
        .unwrap_or_else(|| {
            panic!(
                "expected a positional-after-named diagnostic: {:#?}",
                parsed.diagnostics
            )
        });
    assert_eq!(diagnostic.code, "parse.syntax");
    assert_eq!(diagnostic.kind, "parse");
    // The diagnostic points at the offending positional argument, not the call.
    assert_eq!(
        &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
        "2"
    );
    // The rule is non-fatal: the call still parses with both arguments so later
    // checks see the whole tree, and the violation reports exactly once.
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
            .iter()
            .filter(|d| d
                .message
                .contains("positional argument cannot follow a named argument"))
            .count(),
        1
    );
}

#[test]
fn positional_then_named_arguments_are_accepted() {
    // Positional arguments may precede named ones; only the reverse is rejected.
    let parsed = parse_source("const Made = sub(1, b: 2)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
}

#[test]
fn all_named_arguments_are_accepted() {
    let parsed = parse_source("const Made = sub(a: 1, b: 2)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
}

#[test]
fn positional_after_named_is_rejected_inside_function_bodies() {
    // A call statement in a function body reaches the parser through a different
    // path than a `const` value, so it confirms the rule is checked over the
    // whole tree, not just top-level values.
    let parsed = parse_source("fn run()\n    log(level: 1, 2)\n");
    assert!(
        parsed.diagnostics.iter().any(|d| {
            d.message
                .contains("positional argument cannot follow a named argument")
        }),
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
        parsed.diagnostics.iter().any(|d| {
            d.message
                .contains("positional argument cannot follow a named argument")
        }),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_conversion_and_constructor_calls() {
    // Conversion call on a type keyword.
    let parsed = parse_source("const Count: int = int(raw)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { callee, .. }) = &decl.value else {
        panic!("expected conversion call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if segments == &["int"]),
        "expected int callee, got {callee:?}"
    );

    // Qualified calls keep their path segments.
    let parsed = parse_source("const First = shelf::make(17)\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration");
    };
    let Some(Expression::Call { callee, args, .. }) = &decl.value else {
        panic!("expected constructor call, got {:?}", decl.value);
    };
    assert!(
        matches!(callee.as_ref(), Expression::Name { segments, .. } if segments == &["shelf", "make"]),
        "expected shelf::make callee, got {callee:?}"
    );
    assert_eq!(args.len(), 1);
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

#[derive(Debug)]
enum Expectation<'a> {
    Literal(LiteralKind, &'a str),
    Name(&'a [&'a str]),
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
    assert_eq!(diagnostic.span.line, 2);
    assert!(
        !diagnostic.message.contains("expected"),
        "diagnostic should not fall back to a generic message, got {:?}",
        diagnostic.message
    );
}

#[test]
fn rejects_out_parameter_as_reserved_surface() {
    let parsed = parse_source(
        "module app\n\
         fn parseInt(text: string, out value: int): bool\n\
         \x20   return true\n",
    );

    assert!(parsed.has_errors(), "expected out parameter rejection");
    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "parse.syntax" && diagnostic.message.contains("`out` is reserved")
        }),
        "{:#?}",
        parsed.diagnostics
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
    assert_eq!(diagnostic.span.line, 2);
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
    assert_eq!(diagnostic.span.line, 2);
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
        .map(|diagnostic| diagnostic.span.line)
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
fn reserved_word_as_const_name_is_rejected() {
    // syntax.md: "Reserved words are not identifiers." A const name is an
    // `identifier`, so a reserved word (`at`) there is a parse error, matching
    // the param/member/key name positions.
    let parsed = parse_source("module app\nconst at = 5\n");
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("`at` is a keyword")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn reserved_word_as_var_name_reports_variable_name_diagnostic() {
    // `out` is reserved as a parameter-mode keyword. In a binding position the
    // parser should diagnose the binding name itself, not drop the statement and
    // cascade through the rest of the body.
    let parsed = parse_source("module app\nfn f(): int\n    var out: int = 0\n    return out\n");

    assert_eq!(parsed.diagnostics.len(), 2, "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics[0]
            .message
            .contains("expected variable name"),
        "{:#?}",
        parsed.diagnostics[0]
    );
    assert_eq!(parsed.diagnostics[0].span.line, 3);
    assert!(
        parsed.diagnostics[1]
            .message
            .contains("cannot be used as an expression"),
        "{:#?}",
        parsed.diagnostics[1]
    );
    assert!(
        parsed
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("expected a statement")),
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
store ^books(id: int): Book
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
            Declaration::Store(decl) => decl.root.root.as_str(),
            Declaration::Function(decl) => decl.name.as_str(),
            Declaration::Enum(decl) => decl.name.as_str(),
            Declaration::Evolve(_) => "evolve",
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["MaxLoans", "Book", "books", "normalize"]);
}

// --- Wave 6 findings: single-report guard and grammar tightenings (A21/A23) ---

#[test]
fn const_value_keyword_field_reports_once_not_also_expected_an_expression() {
    // `a.at` fails because `at` is a keyword used as a field name. The const
    // value path drains that specific diagnostic, so the generic "expected an
    // expression" fallback must not also fire: the line reports exactly once.
    let parsed = parse_source("const Bad = a.at\n");
    assert_eq!(
        parsed.diagnostics.len(),
        1,
        "the keyword-field const value should report exactly once: {:#?}",
        parsed.diagnostics
    );
    assert!(
        parsed.diagnostics[0].message.contains("field name"),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

#[test]
fn if_condition_keyword_field_reports_once_not_also_expected_an_expression() {
    // The same single-report guard applies to header expressions: an `if`
    // condition that fails on a keyword field name carries only that diagnostic.
    let parsed = parse_source("fn f()\n    if a.at\n        return\n");
    let on_offending_line: Vec<_> = parsed
        .diagnostics
        .iter()
        .filter(|d| d.span.line == 2)
        .collect();
    assert_eq!(
        on_offending_line.len(),
        1,
        "the keyword-field `if` condition should report exactly once: {on_offending_line:#?}"
    );
    assert!(
        on_offending_line[0].message.contains("field name"),
        "{:#?}",
        on_offending_line[0]
    );
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
        parsed.diagnostics[0].message.contains("require a value"),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

#[test]
fn reserved_word_as_parameter_name_is_rejected() {
    // syntax.md: "Reserved words are not identifiers." A parameter name is an
    // `identifier`, so a reserved word in that position is a parse error.
    let parsed = parse_source("fn f(at: int)\n    return\n");
    assert_eq!(parsed.diagnostics.len(), 1, "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics[0]
            .message
            .contains("expected parameter name"),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

#[test]
fn reserved_word_as_resource_member_name_is_rejected() {
    // A resource member name is an `identifier`; a reserved word (`at`) there is
    // a parse error rather than a silently accepted member.
    let parsed = parse_source("resource R at ^r\n    at: int\n");
    assert_eq!(parsed.diagnostics.len(), 1, "{:#?}", parsed.diagnostics);
    assert!(
        parsed.diagnostics[0]
            .message
            .contains("expected resource member name"),
        "{:#?}",
        parsed.diagnostics[0]
    );
}

#[test]
fn reserved_word_as_key_parameter_name_is_rejected() {
    // A keyed member's key name is an `identifier`; a reserved word (`at`) in a
    // key parameter list is a parse error.
    let parsed = parse_source("resource R at ^r\n    e(at: string): int\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error for the reserved-word key name: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn malformed_while_condition_reports_a_parse_error() {
    // A `while` header that does not parse as a complete expression is a parse
    // error (the A21 hole-close generalized): the grammar requires
    // `while_stmt = "while" expression NEWLINE block`.
    let parsed = parse_source("fn f()\n    while a == b == c\n        return\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "expected a parse error for the malformed `while` condition: {:#?}",
        parsed.diagnostics
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
fn parses_a_flat_enum_declaration() {
    let parsed = parse_source("module app\nenum Status\n    active\n    archived\n    banned\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let status = parsed.file.enum_decl("Status").expect("Status enum");
    assert!(!status.public);
    assert_eq!(member_names(status), ["active", "archived", "banned"]);
}

#[test]
fn parses_pub_enum() {
    let parsed = parse_source("module app\npub enum Status\n    active\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let status = parsed.file.enum_decl("Status").expect("Status enum");
    assert!(status.public);
    assert_eq!(member_names(status), ["active"]);
}

#[test]
fn attaches_doc_comments_to_enum_members() {
    let parsed = parse_source("module app\nenum Status\n    ;; Currently live.\n    active\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let status = parsed.file.enum_decl("Status").expect("Status enum");
    assert_eq!(status.members[0].docs, ["Currently live."]);
}

#[test]
fn rejects_an_enum_with_no_members() {
    let parsed = parse_source("module app\nenum Status\nfn main()\n    return\n");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("at least one member")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_an_enum_member_with_a_type_annotation() {
    let parsed = parse_source("module app\nenum Status\n    active: int\n");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("bare name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn rejects_an_enum_member_with_parameters() {
    let parsed = parse_source("module app\nenum Status\n    active(x: int)\n");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("bare name")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn parses_nested_enum_members_into_a_tree() {
    let parsed = parse_source(
        "module app\nenum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n",
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let cat = parsed.file.enum_decl("Cat").expect("Cat enum");
    assert_eq!(member_names(cat), ["tiger", "housecat"]);
    let tiger = &cat.members[0];
    assert!(tiger.category, "tiger should be a category");
    let nested: Vec<&str> = tiger.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(nested, ["bengal", "siberian"]);
    assert!(
        cat.members[1].members.is_empty(),
        "housecat has no children"
    );
}

#[test]
fn the_category_modifier_sets_the_flag_and_a_bare_member_does_not() {
    let parsed =
        parse_source("module app\nenum Cat\n    category tiger\n        bengal\n    housecat\n");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let cat = parsed.file.enum_decl("Cat").expect("Cat enum");
    assert!(cat.members[0].category, "category tiger");
    assert!(!cat.members[1].category, "bare housecat");
    // The nested member is a plain member, not a category.
    assert!(!cat.members[0].members[0].category, "bengal");
}

#[test]
fn round_trips_an_enum_through_the_formatter() {
    let source = "enum Status\n    active\n    archived\n    banned";
    let parsed = parse_source(source);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    // The canonical form of a single declaration is the declaration followed by a
    // trailing newline, so a clean enum round-trips unchanged.
    assert_eq!(marrow_syntax::format_source(source), format!("{source}\n"));
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
            .any(|d| d.message.contains("member path")),
        "{:#?}",
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

/// `(name, ty, docs)` triples for every parameter of a single function, for
/// comparing parameter lists across the comma, newline, and mixed surfaces.
fn param_shape(source: &str) -> Vec<(String, String, Vec<String>)> {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    parsed
        .file
        .function("f")
        .expect("function f")
        .params
        .iter()
        .map(|param| {
            (
                param.name.clone(),
                param.ty.text.clone(),
                param.docs.clone(),
            )
        })
        .collect()
}

#[test]
fn single_line_parameter_list_parses_unchanged() {
    assert_eq!(
        param_shape("module app\nfn f(a: int, b: string)\n    return\n"),
        vec![
            ("a".to_string(), "int".to_string(), Vec::new()),
            ("b".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn multi_line_parameter_list_without_commas_matches_single_line() {
    let newline_separated = "module app\nfn f(\n    a: int\n    b: string\n)\n    return\n";
    assert_eq!(
        param_shape(newline_separated),
        param_shape("module app\nfn f(a: int, b: string)\n    return\n")
    );
}

#[test]
fn multi_line_parameter_list_with_trailing_commas_matches_single_line() {
    let comma_separated = "module app\nfn f(\n    a: int,\n    b: string,\n)\n    return\n";
    assert_eq!(
        param_shape(comma_separated),
        param_shape("module app\nfn f(a: int, b: string)\n    return\n")
    );
}

#[test]
fn mixed_comma_and_newline_separators_parse_identically() {
    let mixed = "module app\nfn f(\n    a: int,\n    b: string\n    c: bool,\n)\n    return\n";
    assert_eq!(
        param_shape(mixed),
        vec![
            ("a".to_string(), "int".to_string(), Vec::new()),
            ("b".to_string(), "string".to_string(), Vec::new()),
            ("c".to_string(), "bool".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn single_doc_line_above_a_parameter_is_captured() {
    let source = "module app\nfn f(\n    ;; the book to file\n    book: int,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![(
            "book".to_string(),
            "int".to_string(),
            vec!["the book to file".to_string()],
        )]
    );
}

#[test]
fn stacked_doc_lines_are_captured_in_order() {
    let source =
        "module app\nfn f(\n    ;; first line\n    ;; second line\n    book: int,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![(
            "book".to_string(),
            "int".to_string(),
            vec!["first line".to_string(), "second line".to_string()],
        )]
    );
}

#[test]
fn a_parameter_without_a_doc_has_empty_docs() {
    let source =
        "module app\nfn f(\n    ;; documented\n    a: int,\n    b: string,\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![
            (
                "a".to_string(),
                "int".to_string(),
                vec!["documented".to_string()]
            ),
            ("b".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn multi_line_call_arguments_still_parse() {
    // A multi-line call-argument list is governed by the same delimiter-newline
    // suppression; documenting parameters must not regress it.
    let parsed =
        parse_source("module app\nfn f()\n    print(\n        1,\n        2,\n        3,\n    )\n");
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
}

#[test]
fn parameter_type_wrapped_inside_brackets_stays_one_parameter() {
    // A type may span physical lines inside its brackets; the line break sits at
    // a depth above the parameter list, so it must not split the parameter.
    let source = "module app\nfn f(\n    rows: sequence[\n        Book\n    ]\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![("rows".to_string(), "sequence[Book]".to_string(), Vec::new(),)]
    );
}

#[test]
fn parameter_with_wrapped_bracketed_type_and_a_following_parameter_parses_both() {
    let source = "module app\nfn f(\n    rows: sequence[\n        Book\n    ]\n    shelf: string\n)\n    return\n";
    assert_eq!(
        param_shape(source),
        vec![
            ("rows".to_string(), "sequence[Book]".to_string(), Vec::new(),),
            ("shelf".to_string(), "string".to_string(), Vec::new()),
        ]
    );
}

#[test]
fn trailing_doc_with_no_following_parameter_is_reported() {
    // A dangling `;;` run after the last parameter documents nothing; it must be
    // reported rather than silently dropped.
    let source = "module app\nfn f(\n    a: int,\n    ;; orphaned doc\n)\n    return\n";
    let parsed = parse_source(source);
    let diagnostic = parsed
        .diagnostics
        .iter()
        .find(|d| d.message.contains("doc comment must precede a parameter"))
        .expect("a diagnostic for the orphaned doc comment");
    assert_eq!(diagnostic.code, "parse.syntax");
}

fn evolve_decl(parsed: &marrow_syntax::ParsedSource) -> &marrow_syntax::EvolveDecl {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Evolve(decl) => Some(decl),
            _ => None,
        })
        .expect("an evolve declaration")
}

#[test]
fn evolve_block_parses_each_step_to_the_ast() {
    use marrow_syntax::EvolveStep;
    let source = "module app\n\
        evolve\n\
        \x20   rename Book.title -> Book.subtitle\n\
        \x20   default Book.author = \"unknown\"\n\
        \x20   retire ^books.byTitle\n\
        \x20   transform Book.shelf\n\
        \x20       return ^books(1).shelf\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let decl = evolve_decl(&parsed);
    assert_eq!(decl.steps.len(), 4);

    match &decl.steps[0] {
        EvolveStep::Rename { from, to, .. } => {
            assert_eq!(format_expression(from), "Book.title");
            assert_eq!(format_expression(to), "Book.subtitle");
        }
        other => panic!("expected rename, got {other:#?}"),
    }
    match &decl.steps[1] {
        EvolveStep::Default { target, value, .. } => {
            assert_eq!(format_expression(target), "Book.author");
            assert_eq!(format_expression(value), "\"unknown\"");
        }
        other => panic!("expected default, got {other:#?}"),
    }
    match &decl.steps[2] {
        EvolveStep::Retire { target, .. } => {
            assert_eq!(format_expression(target), "^books.byTitle");
        }
        other => panic!("expected retire, got {other:#?}"),
    }
    match &decl.steps[3] {
        EvolveStep::Transform { target, body, .. } => {
            assert_eq!(format_expression(target), "Book.shelf");
            assert_eq!(body.statements.len(), 1);
        }
        other => panic!("expected transform, got {other:#?}"),
    }
}

#[test]
fn evolve_rename_renames_a_saved_root() {
    use marrow_syntax::EvolveStep;
    let source = "module app\nevolve\n    rename ^books -> ^archive\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let decl = evolve_decl(&parsed);
    match &decl.steps[0] {
        EvolveStep::Rename { from, to, .. } => {
            assert_eq!(format_expression(from), "^books");
            assert_eq!(format_expression(to), "^archive");
        }
        other => panic!("expected rename, got {other:#?}"),
    }
}

#[test]
fn evolve_rename_without_arrow_is_reported() {
    let source = "module app\nevolve\n    rename Book.title Book.subtitle\n";
    let parsed = parse_source(source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.code == "parse.syntax" && d.message.contains("->")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_default_without_value_is_reported() {
    let source = "module app\nevolve\n    default Book.title\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_unknown_step_keyword_is_reported() {
    let source = "module app\nevolve\n    rebrand Book.title\n";
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_contextual_words_remain_identifiers_outside_the_block() {
    // `rename`, `default`, `retire`, and `transform` are contextual, so they stay
    // usable as ordinary identifiers (here, function names) outside an evolve block.
    let source = "module app\nfn rename(): int\n    return 1\nfn retire(): int\n    return 2\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.function("rename").is_some());
    assert!(parsed.file.function("retire").is_some());
}

#[test]
fn evolve_indented_block_under_a_non_transform_step_is_reported() {
    // Only a transform carries an indented body; an indented block under rename,
    // default, or retire is a mistake the parser must flag rather than silently
    // consume.
    let source = "module app\n\
        evolve\n\
        \x20   retire Book.title\n\
        \x20       stray body line\n";
    let parsed = parse_source(source);
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.code == "parse.syntax" && d.message.contains("indented block")),
        "{:#?}",
        parsed.diagnostics
    );
}

#[test]
fn evolve_transform_with_a_multi_statement_body_and_a_following_declaration_parse() {
    use marrow_syntax::EvolveStep;
    let source = "module app\n\
        evolve\n\
        \x20   transform Book.shelf\n\
        \x20       const old: string = ^books(1).shelf\n\
        \x20       return old\n\
        fn after(): int\n\
        \x20   return 1\n";
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:#?}", parsed.diagnostics);
    let decl = evolve_decl(&parsed);
    match &decl.steps[0] {
        EvolveStep::Transform { body, .. } => assert_eq!(body.statements.len(), 2),
        other => panic!("expected transform, got {other:#?}"),
    }
    // The declaration after the evolve block still parses.
    assert!(parsed.file.function("after").is_some());
}
