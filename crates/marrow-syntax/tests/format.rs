use marrow_syntax::{
    Block, Comment, CommentMarker, CommentPlacement, Declaration, Statement, format_expression,
    format_source, parse_source,
};

mod common;

/// Format a single-declaration `module app` source through `format_source` and
/// return just that declaration's canonical text. `format_source` wraps the file
/// as `module app\n\n<decl>\n`, so stripping that frame exercises the same
/// declaration-formatting path the public entry point uses.
fn format_decl(source: &str) -> String {
    let formatted = format_source(source);
    formatted
        .strip_prefix("module app\n\n")
        .and_then(|rest| rest.strip_suffix('\n'))
        .expect("format_source frames a single declaration as module app\\n\\n<decl>\\n")
        .to_string()
}

/// Format a single-function `module app` source through `format_source` and
/// return just the function body: the indented statements under the `fn` header.
fn format_function_body(source: &str) -> String {
    let decl = format_decl(source);
    // `format_decl` yields `fn run(...)\n<body>`; drop the header line to leave
    // the body block the test asserts on.
    decl.split_once('\n')
        .map(|(_, body)| body.to_string())
        .expect("a function declaration has a header line and a body")
}

/// Drive the production formatter over `source`, then re-parse its output and
/// hand back the `run` function's body block. Comment round-trips assert on the
/// typed `Block.comments` this yields (text, placement, marker) rather than on
/// substrings of the rendered text, so a comment that survives with the wrong
/// attachment, marker, or placement is a failure even though its text appears.
fn reparsed_run_body(source: &str) -> Block {
    let formatted = format_source(source);
    let parsed = parse_source(&formatted);
    assert!(
        parsed.diagnostics.is_empty(),
        "formatted output must re-parse cleanly:\n{formatted}\n{:#?}",
        parsed.diagnostics
    );
    parsed
        .file
        .function("run")
        .expect("formatted source defines fn run")
        .body
        .clone()
}

/// A retained comment reduced to the facts a round-trip must preserve: its
/// normalized body text, where it sits relative to statements, and whether it
/// renders as a doc (`;;`) or ordinary (`;`) marker.
fn comment_facts(comments: &[Comment]) -> Vec<(&str, CommentPlacement, CommentMarker)> {
    comments
        .iter()
        .map(|comment| (comment.text.as_str(), comment.placement, comment.marker))
        .collect()
}

/// Parse `source` as a const value and return its expression formatted back to
/// canonical source.
fn format_const_value(source: &str) -> String {
    let parsed = parse_source(&format!("const X = {source}\n"));
    assert!(
        parsed.diagnostics.is_empty(),
        "{source:?} should parse cleanly: {:#?}",
        parsed.diagnostics
    );
    let Declaration::Const(decl) = &parsed.file.declarations[0] else {
        panic!("expected const declaration for {source:?}");
    };
    format_expression(decl.value.as_ref().expect("value"))
}

#[test]
fn formats_split_store_declaration() {
    let source = "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         \x20   index byTitle(title, id)\n";

    assert_eq!(
        format_source(source),
        "module app\n\n\
         resource Book\n\
         \x20   required title: string\n\n\
         store ^books(id: int): Book\n\
         \x20   index byTitle(title, id)\n"
    );
}

#[test]
fn formats_expressions_to_canonical_source() {
    // Each input is already canonical, so formatting must reproduce it exactly.
    let canonical = [
        "5",
        "3.14",
        "\"hi\"",
        "b\"mw\"",
        "true",
        "name",
        "std::math::PI",
        "^books",
        "^books(id).title",
        "^books(id).\"old-title\"",
        "nextId(^books)",
        "shelf::make(17)",
        "save(book: draft, total: total)",
        "60 * 60 + 1",
        "a and b or c",
        "not ready",
        "-count",
        "first + last",
        "1..10",
        "1..=10",
        "a == b",
        "a != b",
        "a <= b",
        "a % b",
        // Range binds looser than `+`, so the additive operands need no parens.
        "a + b..c + d",
        "not not ready",
        "$\"book {id}: {{ready}}\"",
    ];
    for source in canonical {
        assert_eq!(format_const_value(source), source, "input {source:?}");
    }
}

#[test]
fn reinserts_minimal_parentheses_for_precedence() {
    // The syntax tree drops parentheses; the formatter restores only those
    // required to preserve the parsed grouping.
    let cases = [
        ("(1 + 2) * 3", "(1 + 2) * 3"),
        ("3 * (1 + 2)", "3 * (1 + 2)"),
        // Redundant parentheses are dropped when precedence already implies them.
        ("1 + (2 * 3)", "1 + 2 * 3"),
        ("(a)", "a"),
        ("-(a + b)", "-(a + b)"),
        ("not (a and b)", "not (a and b)"),
        ("(a or b) and c", "(a or b) and c"),
        ("(count ?? 0) < 5", "count ?? 0 < 5"),
        ("(start ?? 1)..n", "start ?? 1..n"),
    ];
    for (input, expected) in cases {
        assert_eq!(format_const_value(input), expected, "input {input:?}");
    }
}

#[test]
fn formats_statement_blocks_with_indentation() {
    let source = "module app\n\
         fn run(n: int)\n\
         \x20   const total: int = 0\n\
         \x20   var seen(id: int): bool\n\
         \x20   if n < 0\n\
         \x20       print(\"neg\")\n\
         \x20   else if n == 0\n\
         \x20       print(\"zero\")\n\
         \x20   else\n\
         \x20       total = total + n\n\
         \x20   for id in keys(^books)\n\
         \x20       delete ^books(id)\n\
         \x20   return total\n";
    let expected = "\
         \x20   const total: int = 0\n\
         \x20   var seen(id: int): bool\n\
         \x20   if n < 0\n\
         \x20       print(\"neg\")\n\
         \x20   else if n == 0\n\
         \x20       print(\"zero\")\n\
         \x20   else\n\
         \x20       total = total + n\n\
         \x20   for id in keys(^books)\n\
         \x20       delete ^books(id)\n\
         \x20   return total";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_loops_and_unlabeled_break() {
    let source = "module app\n\
         fn run()\n\
         \x20   for id in keys(^books)\n\
         \x20       while ready\n\
         \x20           break\n";
    let expected = "\
         \x20   for id in keys(^books)\n\
         \x20       while ready\n\
         \x20           break";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_a_range_for_with_a_by_step() {
    // The `by` step round-trips: header endpoints and the step are re-emitted.
    let source = "module app\n\
         fn run()\n\
         \x20   for i in 10..=1 by -2\n\
         \x20       print($\"{i}\")\n";
    let expected = "\
         \x20   for i in 10..=1 by -2\n\
         \x20       print($\"{i}\")";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_transaction_and_try_blocks() {
    let source = "module app\n\
         fn commit(id: Id(^books))\n\
         \x20   transaction\n\
         \x20       ^books(id).title = title\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err: Error\n\
         \x20       print(err.message)\n";
    let expected = "\
         \x20   transaction\n\
         \x20       ^books(id).title = title\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err: Error\n\
         \x20       print(err.message)";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_a_match_with_bare_member_arms() {
    let source = "module app\n\
         fn label(s: Status)\n\
         \x20   match s\n\
         \x20       active\n\
         \x20           print(\"a\")\n\
         \x20       archived\n\
         \x20           print(\"b\")\n";
    let expected = "\
         \x20   match s\n\
         \x20       active\n\
         \x20           print(\"a\")\n\
         \x20       archived\n\
         \x20           print(\"b\")";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_a_match_with_qualified_member_path_arms() {
    // A qualified arm `tiger::bengal` renders as its path; a category arm `lion`
    // renders as the bare member. The formatter re-emits the relative path exactly.
    let source = "module app\n\
         fn label(c: Cat)\n\
         \x20   match c\n\
         \x20       tiger::bengal\n\
         \x20           print(\"a\")\n\
         \x20       lion\n\
         \x20           print(\"b\")\n";
    let expected = "\
         \x20   match c\n\
         \x20       tiger::bengal\n\
         \x20           print(\"a\")\n\
         \x20       lion\n\
         \x20           print(\"b\")";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_const_declaration_with_docs() {
    let source = "module app\n\
         ;; The maximum number of loans.\n\
         const MaxLoans: int = 5\n";
    let expected = ";; The maximum number of loans.\n\
         const MaxLoans: int = 5";
    assert_eq!(format_decl(source), expected);
}

#[test]
fn formats_empty_doc_comment_lines_without_trailing_whitespace() {
    // Render contract: a blank line between doc paragraphs renders as a bare `;;`
    // with no trailing space. The golden pins that exact text, and the structural
    // checks below pin the two facts behind it — no line ends in whitespace, and
    // the empty paragraph break survives as an empty entry in the typed docs.
    let source = "module app\n\
         ;; First paragraph.\n\
         ;;\n\
         ;; Second paragraph.\n\
         const MaxLoans: int = 5\n";
    let expected = ";; First paragraph.\n\
         ;;\n\
         ;; Second paragraph.\n\
         const MaxLoans: int = 5";
    let formatted = format_decl(source);
    assert_eq!(formatted, expected);
    assert!(
        formatted.lines().all(|line| !line.ends_with(' ')),
        "formatter output contains trailing whitespace:\n{formatted:?}"
    );

    let reparsed = parse_source(&format_source(source));
    let Some(Declaration::Const(decl)) = reparsed.file.declarations.first() else {
        panic!("expected a const declaration: {:#?}", reparsed.file);
    };
    assert_eq!(decl.docs, ["First paragraph.", "", "Second paragraph."]);
}

#[test]
fn formats_resource_declaration_with_members() {
    let source = "module app\n\
         resource Book\n\
         \x20   ;; Display title.\n\
         \x20   required title: string\n\
         \x20   tags(pos: int): string\n\
         \x20   notes(noteId: string)\n\
         \x20       text: string\n\
         store ^books(id: int): Book\n\
         \x20   index byShelf(shelf, id) unique\n";
    let expected = "module app\n\n\
         resource Book\n\
         \x20   ;; Display title.\n\
         \x20   required title: string\n\
         \x20   tags(pos: int): string\n\
         \x20   notes(noteId: string)\n\
         \x20       text: string\n\
         \n\
         store ^books(id: int): Book\n\
         \x20   index byShelf(shelf, id) unique";
    assert_eq!(format_source(source).trim_end(), expected);
}

#[test]
fn formats_function_declaration_with_params() {
    let source = "module app\n\
         pub fn add(title: string, total: int): int\n\
         \x20   return total\n";
    let expected = "pub fn add(title: string, total: int): int\n\
         \x20   return total";
    assert_eq!(format_decl(source), expected);
}

#[test]
fn formats_whole_file_with_blank_line_policy() {
    let source = "module shelf::books\n\
         use std::clock\n\
         use shelf::books\n\
         const MaxLoans: int = 5\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): int\n\
         \x20   return 1\n";
    // Module, the use block, and each declaration are separated by one blank line.
    let expected = "module shelf::books\n\
         \n\
         use std::clock\n\
         use shelf::books\n\
         \n\
         const MaxLoans: int = 5\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \n\
         store ^books(id: int): Book\n\
         \n\
         pub fn add(title: string): int\n\
         \x20   return 1\n";
    assert_eq!(format_source(source), expected);
}

/// A span-independent structural fingerprint of a parsed file: the `Debug`
/// rendering with every `SourceSpan { ... }` region removed. Two files compare
/// equal exactly when their declarations match structurally (names, statements,
/// nesting, retained comments), ignoring byte positions that formatting shifts.
fn structural_fingerprint(source: &str) -> String {
    let debug = format!("{:#?}", parse_source(source).file);
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug.as_str();
    while let Some(at) = rest.find("SourceSpan {") {
        out.push_str(&rest[..at]);
        // Skip past the matching closing brace of this `SourceSpan { ... }`.
        let after = &rest[at + "SourceSpan {".len()..];
        let close = after
            .find('}')
            .expect("SourceSpan debug has a closing brace");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Corpus contract for the whole formatter: over every documented module file,
/// `format_source` is a fixed point, its output re-parses cleanly, and it
/// preserves the declaration tree (span-stripped AST equality). The per-construct
/// formatting goldens above own the exact rendered text; this owns the
/// structure-preservation and stability invariants across the real corpus.
#[test]
fn format_source_preserves_structure_and_reparses_cleanly() {
    let blocks = common::documented_module_blocks();
    assert!(blocks.len() >= 5, "expected several module files");
    for block in blocks {
        let source = block.source;
        let once = format_source(&source);
        let twice = format_source(&once);
        assert_eq!(
            once, twice,
            "format_source is not a fixed point for:\n{source}"
        );
        // The formatted output must itself be valid Marrow.
        let reparsed = parse_source(&once);
        assert!(
            reparsed.diagnostics.is_empty(),
            "formatted output should re-parse cleanly:\n{once}\n{:#?}",
            reparsed.diagnostics
        );
        // Formatting must not drop, reorder, or otherwise alter a declaration:
        // the original and the reformatted source must parse to the same tree
        // (modulo the byte positions that formatting necessarily shifts).
        assert_eq!(
            structural_fingerprint(&source),
            structural_fingerprint(&once),
            "formatting changed the declaration tree for:\n{source}\n--- formatted ---\n{once}"
        );
    }
}

#[test]
fn formatting_is_a_stable_fixed_point() {
    // Formatting then re-parsing yields the same canonical text. This only
    // checks stability (idempotency), not that structure is preserved.
    let inputs = [
        "60 * 60 + 1",
        "(1 + 2) * 3",
        "f(a, b: 2)",
        "^books(id).title",
        "not a or b",
    ];
    for input in inputs {
        let once = format_const_value(input);
        let twice = format_const_value(&once);
        assert_eq!(once, twice, "formatting not stable for {input:?}");
    }
}

#[test]
fn round_trips_ordinary_line_comments_by_placement() {
    // A leading own-line comment, a trailing comment after code, and a final
    // standalone own-line comment survive parse -> format as block trivia with
    // their normalized text, placement, and ordinary `;` marker intact. The
    // attachment facts are checked directly on the re-parsed body block, so a
    // comment that re-renders but loses its placement is a failure.
    let source = "module app\n\
         fn run()\n\
         \x20   ; set up the total\n\
         \x20   const total: int = 0\n\
         \x20   print(total) ; show it\n\
         \x20   ; nothing left to do\n";
    let expected = [
        (
            "set up the total",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ),
        ("show it", CommentPlacement::Trailing, CommentMarker::Line),
        (
            "nothing left to do",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ),
    ];

    let body = reparsed_run_body(source);
    assert_eq!(comment_facts(&body.comments), expected);

    // Formatting is a fixed point for these comments: re-rendering the canonical
    // form yields the same trivia, so no round-trip drops or duplicates them.
    let recanonicalized = format_source(&format_source(source));
    let again = reparsed_run_body(&recanonicalized);
    assert_eq!(comment_facts(&again.comments), expected);
}

#[test]
fn preserves_overindented_body_comments_at_block_indent() {
    let source = "module app\n\
         fn run()\n\
         \x20   print(\"before\")\n\
         \x20       ; keep this comment\n\
         \x20   print(\"after\")\n";
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   print(\"before\")\n\
         \x20   ; keep this comment\n\
         \x20   print(\"after\")\n";

    assert_eq!(format_source(source), expected);
    let body = reparsed_run_body(&format_source(source));
    assert_eq!(
        comment_facts(&body.comments),
        [(
            "keep this comment",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        )]
    );
}

#[test]
fn drops_overindented_comments_that_belong_to_invalid_statement_blocks() {
    let source = "module app\n\
         fn run()\n\
         \x20   print(\"before\")\n\
         \x20       ; invalid block comment\n\
         \x20       print(\"bad\")\n\
         \x20   print(\"after\")\n";
    let formatted = format_source(source);
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   print(\"before\")\n\
         \x20   print(\"after\")\n";

    assert_eq!(formatted, expected);
    let body = reparsed_run_body(&formatted);
    assert!(
        body.comments.is_empty(),
        "invalid-block comment should not survive recovery: {:#?}",
        body.comments
    );
}

#[test]
fn normalizes_body_doc_comments_to_ordinary_comments() {
    // A `;;` documents the next declaration or member; inside a function body
    // there is nothing to document, so both the own-line and the trailing path
    // normalize it to an ordinary `;` comment. After formatting, the re-parsed
    // trivia therefore carries the `Line` marker, never `Doc`.
    let source = "module app\n\
         fn run()\n\
         \x20   ;; first comment\n\
         \x20   print(\"a\")\n\
         \x20   const x: int = 1 ;; trailing doc\n";
    let expected = [
        (
            "first comment",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ),
        (
            "trailing doc",
            CommentPlacement::Trailing,
            CommentMarker::Line,
        ),
    ];

    let body = reparsed_run_body(source);
    assert_eq!(comment_facts(&body.comments), expected);

    let recanonicalized = format_source(&format_source(source));
    let again = reparsed_run_body(&recanonicalized);
    assert_eq!(comment_facts(&again.comments), expected);
}

#[test]
fn round_trips_comments_attached_inside_nested_blocks() {
    // Comments inside a nested block stay block trivia of that inner block, not
    // the function body: an own-line and a trailing comment inside the `if`, and
    // the comment trailing the `if`, all belong to the then-block. Asserting the
    // attachment (rather than a flattened substring scan) pins which block owns
    // each comment, which a textual round-trip cannot distinguish.
    let source = "module app\n\
         fn run(n: int)\n\
         \x20   if n < 0\n\
         \x20       ; negative branch\n\
         \x20       print(\"neg\") ; report\n\
         \x20   ; after the if\n\
         \x20   return\n";
    let expected = [
        (
            "negative branch",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ),
        ("report", CommentPlacement::Trailing, CommentMarker::Line),
        (
            "after the if",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ),
    ];

    let then_comments = |source: &str| {
        let body = reparsed_run_body(source);
        assert!(
            body.comments.is_empty(),
            "no comment attaches to the outer body: {:#?}",
            body.comments
        );
        let Statement::If { then_block, .. } = &body.statements[0] else {
            panic!("first statement is the if: {:?}", body.statements[0]);
        };
        comment_facts(&then_block.comments)
            .into_iter()
            .map(|(text, placement, marker)| (text.to_string(), placement, marker))
            .collect::<Vec<_>>()
    };
    let expected = expected
        .into_iter()
        .map(|(text, placement, marker)| (text.to_string(), placement, marker))
        .collect::<Vec<_>>();

    assert_eq!(then_comments(source), expected);

    let recanonicalized = format_source(&format_source(source));
    assert_eq!(then_comments(&recanonicalized), expected);
}

#[test]
fn documented_parameters_format_one_per_line() {
    // Render contract: a parameter list carrying docs is laid out one parameter
    // per line, each under its `;;` doc lines, with a trailing comma. This golden
    // pins that exact layout, the one place the formatter's output text is the
    // contract; it is regenerated only on an intentional layout change.
    let source = "module app\n\
         fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         \x20   ;; shelf it is filed under\n\
         \x20   shelf: string,\n\
         )\n\
         \x20   return\n";
    let decl = format_decl(source);
    let expected = "fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         \x20   ;; shelf it is filed under\n\
         \x20   shelf: string,\n\
         )\n\
         \x20   return";
    assert_eq!(decl, expected);
}

#[test]
fn documented_parameter_signature_round_trips() {
    // A multi-line `;;` doc block attaches to its parameter and survives
    // parse -> format. Asserting the re-parsed `ParamDecl.docs` pins each doc to
    // the right parameter with each line intact, which a substring scan over the
    // rendered text cannot, and a second format is a fixed point on those facts.
    let source = "module app\n\
         fn f(\n\
         \x20   ;; first line\n\
         \x20   ;; second line\n\
         \x20   book: int,\n\
         \x20   shelf: string,\n\
         )\n\
         \x20   return\n";

    let param_docs = |source: &str| {
        let parsed = parse_source(source);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        parsed
            .file
            .function("f")
            .expect("formatted source defines fn f")
            .params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    param.ty.text.clone(),
                    param.docs.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = vec![
        (
            "book".to_string(),
            "int".to_string(),
            vec!["first line".to_string(), "second line".to_string()],
        ),
        ("shelf".to_string(), "string".to_string(), Vec::new()),
    ];

    let once = format_source(source);
    let twice = format_source(&once);
    assert_eq!(
        once, twice,
        "documented signature formatting is not a fixed point"
    );
    assert_eq!(param_docs(&once), expected);
}

#[test]
fn preserves_top_level_and_member_line_comments() {
    let source = "module app\n\
         ; shared constants\n\
         const Max:int=5\n\
         ; stored records\n\
         resource Book\n\
         \x20   ; visible label\n\
         \x20   title: string\n";
    let expected = "module app\n\
         \n\
         ; shared constants\n\
         const Max: int = 5\n\
         \n\
         ; stored records\n\
         resource Book\n\
         \x20   ; visible label\n\
         \x20   title: string\n";

    assert_eq!(format_source(source), expected);
}

#[test]
fn keeps_standalone_doc_paragraph_separate_from_following_declaration_docs() {
    let source = "module app\n\
         ;; Module overview.\n\
         ;;\n\
         \n\
         ;; Stored books.\n\
         resource Book\n\
         \x20   title: string\n";
    let expected = "module app\n\
         \n\
         ;; Module overview.\n\
         ;;\n\
         \n\
         ;; Stored books.\n\
         resource Book\n\
         \x20   title: string\n";

    assert_eq!(format_source(source), expected);
}

#[test]
fn preserves_multiline_trailing_comma_calls() {
    let source = "module app\n\
         fn fail()\n\
         \x20   throw Error(\n\
         \x20       code: \"book.absent\",\n\
         \x20       message: \"missing book\",\n\
         \x20   )\n";
    let expected = "module app\n\
         \n\
         fn fail()\n\
         \x20   throw Error(\n\
         \x20       code: \"book.absent\",\n\
         \x20       message: \"missing book\",\n\
         \x20   )\n";

    assert_eq!(format_source(source), expected);
}

#[test]
fn formats_evolve_block_consistently_with_resource_and_store() {
    let source = "module app\n\
         evolve\n\
         \x20   rename Book.title    ->   Book.subtitle\n\
         \x20   default Book.author = \"unknown\"\n\
         \x20   retire ^books.byTitle\n\
         \x20   transform Book.shelf\n\
         \x20       return ^books(1).shelf\n";
    let expected = "module app\n\
         \n\
         evolve\n\
         \x20   rename Book.title -> Book.subtitle\n\
         \x20   default Book.author = \"unknown\"\n\
         \x20   retire ^books.byTitle\n\
         \x20   transform Book.shelf\n\
         \x20       return ^books(1).shelf\n";

    assert_eq!(format_source(source), expected);
}

#[test]
fn evolve_block_format_is_idempotent() {
    let source = "module app\n\
         evolve\n\
         \x20   rename ^books -> ^archive\n";
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}
