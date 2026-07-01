use crate::common;
use marrow_syntax::{
    Block, Comment, CommentMarker, CommentPlacement, Declaration, EvolveDecl, Statement,
    SurfaceDecl, format_expression, format_preserves_comments, format_source, parse_source,
};
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

fn reparsed_evolve_decl(source: &str) -> EvolveDecl {
    let formatted = format_source(source);
    let parsed = parse_source(&formatted);
    assert!(
        parsed.diagnostics.is_empty(),
        "formatted output must re-parse cleanly:\n{formatted}\n{:#?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Evolve(decl) => Some(decl.clone()),
            _ => None,
        })
        .expect("formatted source defines evolve")
}

fn reparsed_surface_decl(source: &str) -> SurfaceDecl {
    let formatted = format_source(source);
    let parsed = parse_source(&formatted);
    assert!(
        parsed.diagnostics.is_empty(),
        "formatted output must re-parse cleanly:\n{formatted}\n{:#?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Surface(decl) => Some(decl.clone()),
            _ => None,
        })
        .expect("formatted source defines surface")
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

/// A deeply nested interpolation formats back to itself: the formatter reuses the
/// expression printer at each hole, so a three-deep nest round-trips exactly.
#[test]
fn formats_deeply_nested_interpolation() {
    let value = "$\"a{$\"b{$\"c{x}d\"}e\"}f\"";
    assert_eq!(format_const_value(value), value);
}

/// An over-indented own-line comment inside a resource or store body is layout
/// trivia, exactly as in a function or enum body: it is neither a parse error nor
/// a structural indent. The lexer treats a comment whose run resolves back to the
/// block level as trivia rather than opening a spurious indented block.
#[test]
fn over_indented_own_line_comment_in_member_body_is_trivia() {
    let cases = [
        "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20       ; over-indented note\n\
         \x20   required author: string\n\
         store ^books(id: int): Book\n",
        "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         \x20   index byTitle(title)\n\
         \x20       ; over-indented note\n\
         \x20   index byAuthor(title)\n",
        "module app\n\
         fn run()\n\
         \x20   var count: int = 0\n\
         \x20       ; over-indented note\n\
         \x20   return count\n",
        "module app\n\
         enum Status\n\
         \x20   active\n\
         \x20       ; over-indented note\n\
         \x20   archived\n",
    ];
    for source in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.is_empty(),
            "an over-indented own-line comment must not be a parse error:\n{source}\n{:#?}",
            parsed.diagnostics
        );
        let formatted = format_source(source);
        let reparsed = parse_source(&formatted);
        assert!(
            reparsed.diagnostics.is_empty(),
            "formatted output must re-parse cleanly:\n{formatted}\n{:#?}",
            reparsed.diagnostics
        );
        assert!(
            formatted.contains("over-indented note"),
            "the comment must survive formatting:\n{formatted}"
        );
    }
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
fn formats_surface_declaration_with_canonical_items() {
    let source = "module app\n\
         surface Books from ^books\n\
         \x20   fields title,author, blurb\n\
         \x20   collection ^books as list\n\
         \x20   collection ^books.byAuthor as byAuthor\n\
         \x20   collection ^books.byPublished range as byPublished\n\
         \x20   create title,author\n\
         \x20   update title,blurb\n\
         \x20   action addBook\n\
         \x20   action shelf::loanBook as loan\n";
    let expected = "module app\n\
         \n\
         surface Books from ^books\n\
         \x20   fields title, author, blurb\n\
         \x20   collection ^books as list\n\
         \x20   collection ^books.byAuthor as byAuthor\n\
         \x20   collection ^books.byPublished range as byPublished\n\
         \x20   create title, author\n\
         \x20   update title, blurb\n\
         \x20   action addBook\n\
         \x20   action shelf::loanBook as loan\n";

    assert_eq!(format_source(source), expected);
}

#[test]
fn preserves_surface_comments_and_index_named_as() {
    let source = "module app\n\
         surface Books from ^books\n\
         \x20   ; public collections\n\
         \x20   collection ^books.as as byAs ; index named as\n";
    let expected = "module app\n\
         \n\
         surface Books from ^books\n\
         \x20   ; public collections\n\
         \x20   collection ^books.as as byAs ; index named as\n";

    assert_eq!(format_source(source), expected);

    let surface = reparsed_surface_decl(source);
    assert_eq!(
        comment_facts(&surface.comments),
        vec![
            (
                "public collections",
                CommentPlacement::OwnLine,
                CommentMarker::Line,
            ),
            (
                "index named as",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
        ]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
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
        // `??` is right-associative, so the right operand of a chain stays bare
        // while a left grouping keeps its parentheses.
        ("a ?? b ?? c", "a ?? b ?? c"),
        ("a ?? (b ?? c)", "a ?? b ?? c"),
        ("(a ?? b) ?? c", "(a ?? b) ?? c"),
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
fn formats_compound_assignment_canonically() {
    let source = "module app\n\
         fn run()\n\
         \x20   count*=3\n\
         \x20   total + = count\n";
    let expected = "\
         \x20   count *= 3\n\
         \x20   total += count";
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

/// A single blank line between sibling match arms groups them exactly as it
/// groups statements, members, and sibling if-blocks: one blank in the source is
/// preserved, arms with no blank stay tight, and the result is idempotent.
#[test]
fn preserves_grouping_blank_between_match_arms() {
    let source = "module app\n\
         fn label(s: Status)\n\
         \x20   match s\n\
         \x20       active\n\
         \x20           print(\"a\")\n\
         \n\
         \x20       archived\n\
         \x20           print(\"b\")\n\
         \x20       deleted\n\
         \x20           print(\"c\")\n";
    let expected = "\
         \x20   match s\n\
         \x20       active\n\
         \x20           print(\"a\")\n\
         \n\
         \x20       archived\n\
         \x20           print(\"b\")\n\
         \x20       deleted\n\
         \x20           print(\"c\")";
    assert_eq!(format_function_body(source), expected);
    let once = format_source(source);
    assert_eq!(
        format_source(&once),
        once,
        "match-arm grouping blank is not idempotent"
    );
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
fn formats_optional_function_return_and_absent_value() {
    let source = "module app\n\
         fn f(): int?\n\
         \x20   return absent\n";

    assert_eq!(
        format_source(source),
        "module app\n\nfn f(): int?\n    return absent\n"
    );
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

/// The canonical runnable sample is the conformance oracle, and a formatter that
/// cannot format its own canonical sample is the defect: `format_source` of the
/// verbatim sample must return it unchanged.
#[test]
fn canonical_sample_is_already_fmt_canonical() {
    let source = common::reference_sample();
    assert_eq!(
        format_source(&source),
        source,
        "the canonical sample.md is not in fmt-canonical form"
    );
}

/// A trailing comma forces the inner call multiline, and a call that wraps a
/// multiline argument must expand too: the parser reads any call whose
/// parentheses span more than one line as multiline, so an inline parent would
/// not survive a re-parse. A single pass must already be a fixed point, and a
/// comment-free program must never trip a false comment-loss.
#[test]
fn single_line_call_wrapping_a_trailing_comma_call_is_idempotent() {
    let source = "module app\n\npub fn run()\n    print(h(g(a: 1, b: 2,)))\n";
    let once = format_source(source);
    assert_eq!(
        format_source(&once),
        once,
        "a single-line call wrapping a trailing-comma call is not idempotent:\n{once}"
    );
    assert!(
        format_preserves_comments(source, &once),
        "a comment-free program must never trip comment-loss:\n{once}"
    );
}

/// A string interpolation is lexed within one source line, so an embedded call
/// can never expand across lines no matter what trailing comma or wrapped child
/// it carries. The formatter keeps it inline, so a single pass is a fixed point
/// and a comment-free program never trips a false comment-loss.
#[test]
fn trailing_comma_call_inside_interpolation_is_idempotent() {
    let cases = [
        // Bare interpolation: no outer call wraps it.
        (
            "module app\n\npub fn run()\n    x = $\"a{g(a: 1,)}b\"\n",
            "$\"a{g(a: 1)}b\"",
        ),
        // Interpolation wrapped by an outer call argument.
        (
            "module app\n\npub fn run()\n    print($\"a{g(a: 1, b: 2,)}b\")\n",
            "$\"a{g(a: 1, b: 2)}b\"",
        ),
    ];
    for (source, inline_interp) in cases {
        let once = format_source(source);
        assert!(
            once.contains(inline_interp),
            "the embedded call must render inline, leaving the interpolation on one line:\n{once}"
        );
        assert_eq!(
            format_source(&once),
            once,
            "a trailing-comma call inside an interpolation is not idempotent:\n{once}"
        );
        assert!(
            format_preserves_comments(source, &once),
            "a comment-free program must never trip comment-loss:\n{once}"
        );
        let parsed = parse_source(&once);
        assert!(
            parsed.diagnostics.is_empty(),
            "formatted interpolation must re-parse cleanly:\n{once}\n{:#?}",
            parsed.diagnostics
        );
    }
}

/// A single blank line between statements or members is preserved, two or more
/// consecutive blank lines collapse to one, and a leading or trailing blank line
/// inside a body is dropped.
#[test]
fn preserves_single_intra_body_blank_line() {
    let source = "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         \n\
         \n\
         \x20   loanedTo: string\n\
         pub fn run()\n\
         \n\
         \x20   const a = 1\n\
         \n\
         \x20   const b = 2\n\
         \n";
    let expected = "module app\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \n\
         \x20   loanedTo: string\n\
         \n\
         pub fn run()\n\
         \x20   const a = 1\n\
         \n\
         \x20   const b = 2\n";
    assert_eq!(format_source(source), expected);
    assert_eq!(
        format_source(&format_source(source)),
        expected,
        "blank-line normalization is not idempotent"
    );
}

/// A `;;` doc comment attached to a member carries the member's grouping blank
/// line: the blank above the doc comment is preserved exactly as it is for a
/// plain member or a `;`-commented one, and the result is idempotent.
#[test]
fn preserves_blank_above_doc_commented_member() {
    let source = "module app\n\
         resource Book\n\
         \x20   required title: string\n\
         \n\
         \x20   ;; Who currently holds the book.\n\
         \x20   loanedTo: string\n";
    let expected = "module app\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \n\
         \x20   ;; Who currently holds the book.\n\
         \x20   loanedTo: string\n";
    assert_eq!(format_source(source), expected);
    assert_eq!(
        format_source(&format_source(source)),
        expected,
        "blank above a doc-commented member is not idempotent"
    );
}

/// A comment that sits after a blank line stays its own line and is not pulled up
/// into the preceding statement's line.
#[test]
fn comment_after_blank_line_stays_attached_to_following_item() {
    let source = "module app\n\
         pub fn run()\n\
         \x20   const a = 1\n\
         \n\
         \x20   ; about b\n\
         \x20   const b = 2\n";
    let expected = "module app\n\
         \n\
         pub fn run()\n\
         \x20   const a = 1\n\
         \n\
         \x20   ; about b\n\
         \x20   const b = 2\n";
    assert_eq!(format_source(source), expected);
}

/// A top-level `;` comment that follows a blank line stays glued to the
/// declaration below it, with the blank line above the comment, regardless of
/// whether the preceding declaration is block-bearing. The comment must never be
/// re-grouped upward onto the predecessor.
#[test]
fn top_level_comment_after_blank_stays_with_following_decl_across_block_bearing_predecessors() {
    let predecessors = [
        "resource Item\n    name: text",
        "enum Color\n    red\n    green",
        "store ^items(id: text): Item\n    index by_name(name)",
        "evolve\n    retire ^old",
        "pub fn one()\n    const a = 1",
    ];
    for predecessor in predecessors {
        let source =
            format!("module app\n\n{predecessor}\n\n; about two\npub fn two()\n    const b = 2\n");
        let once = format_source(&source);
        assert!(
            once.contains("\n\n; about two\npub fn two()"),
            "comment detached from following decl after predecessor `{predecessor}`:\n{once}"
        );
        assert_eq!(
            format_source(&once),
            once,
            "format is not idempotent after predecessor `{predecessor}`:\n{once}"
        );
    }
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
fn rejects_body_doc_comments_at_parse() {
    // A `;;` documents the next declaration or member; inside a function body
    // there is nothing to document. Such a doc comment is a parse error, not
    // silently retained trivia: a swallowed doc comment is one the formatter
    // cannot place, which would break the check-run-format round trip. Both the
    // own-line and the trailing position are rejected.
    for source in [
        "module app\nfn run()\n    ;; orphan doc\n    print(\"a\")\n",
        "module app\nfn run()\n    const x: int = 1 ;; trailing doc\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "parse.syntax"),
            "a body doc comment must be a parse error: {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
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

/// An indented top-level own-line comment must round-trip exactly like the
/// column-1 form: parse cleanly, be retained, and re-render at column 1 so
/// formatting is a fixed point and never refuses with comment loss. Covers the
/// before-first-decl and between-decls positions for both `;` and `;;`, plus an
/// indented top-level comment at end of file.
#[test]
fn preserves_indented_top_level_own_line_comments() {
    let source = "module app\n\
         \x20   ; indented before first decl\n\
         const Max:int=5\n\
         \x20   ;; indented between decls\n\
         const Min:int=0\n\
         \x20   ; indented at end of file\n";
    let expected = "module app\n\
         \n\
         ; indented before first decl\n\
         const Max: int = 5\n\
         \n\
         ;; indented between decls\n\
         const Min: int = 0\n\
         \n\
         ; indented at end of file\n";

    assert_eq!(format_source(source), expected);
    assert!(
        format_preserves_comments(source, expected),
        "indented top-level comment must round-trip without comment loss"
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

/// An indented `;;` doc comment with no declaration to document is a parse
/// error at the top level, exactly like the column-1 form. Retaining indented
/// top-level comments must not silently swallow a dangling doc comment.
#[test]
fn rejects_indented_top_level_doc_comment_without_target() {
    let parsed = parse_source("module app\n    ;; dangling doc at eof\n");
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "parse.syntax"),
        "an indented dangling doc comment must be a parse error: {:#?}",
        parsed.diagnostics
    );
}

#[test]
fn preserves_top_level_header_trailing_comments() {
    let source = "module app ; module rationale\n\
         use common ; use rationale\n\
         const Max:int=5 ; const rationale\n\
         ;; Stored books.\n\
         resource Book ; resource rationale\n\
         \x20   title: string\n\
         store ^books: Book ; store rationale\n\
         evolve ; evolve rationale\n\
         \x20   rename Book.title -> Book.name\n\
         enum Status ; enum rationale\n\
         \x20   active\n\
         fn run() ; function rationale\n\
         \x20   return\n";
    let expected = "module app ; module rationale\n\
         \n\
         use common ; use rationale\n\
         \n\
         const Max: int = 5 ; const rationale\n\
         \n\
         ;; Stored books.\n\
         resource Book ; resource rationale\n\
         \x20   title: string\n\
         \n\
         store ^books: Book ; store rationale\n\
         \n\
         evolve ; evolve rationale\n\
         \x20   rename Book.title -> Book.name\n\
         \n\
         enum Status ; enum rationale\n\
         \x20   active\n\
         \n\
         fn run() ; function rationale\n\
         \x20   return\n";

    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_member_header_trailing_comments() {
    let source = "module app\n\
         resource Book\n\
         \x20   ;; Shared details.\n\
         \x20   details ; group rationale\n\
         \x20       ;; Display title.\n\
         \x20       required title: string ; field rationale\n\
         store ^books: Book\n\
         \x20   ;; Lookup by title.\n\
         \x20   index byTitle(title) ; index rationale\n\
         enum Status\n\
         \x20   ;; Live states.\n\
         \x20   category live ; category rationale\n\
         \x20       ;; Selectable state.\n\
         \x20       active ; member rationale\n";
    let expected = "module app\n\
         \n\
         resource Book\n\
         \x20   ;; Shared details.\n\
         \x20   details ; group rationale\n\
         \x20       ;; Display title.\n\
         \x20       required title: string ; field rationale\n\
         \n\
         store ^books: Book\n\
         \x20   ;; Lookup by title.\n\
         \x20   index byTitle(title) ; index rationale\n\
         \n\
         enum Status\n\
         \x20   ;; Live states.\n\
         \x20   category live ; category rationale\n\
         \x20       ;; Selectable state.\n\
         \x20       active ; member rationale\n";

    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_multiline_top_level_headers() {
    let source = "module app\n\
         const Info = save(\n\
         \x20   title: \"x\",\n\
         ) ; const rationale\n\
         fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         ) ; function rationale\n\
         \x20   return\n";
    let expected = "module app\n\
         \n\
         const Info = save(\n\
         \x20   title: \"x\",\n\
         ) ; const rationale\n\
         \n\
         fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         ) ; function rationale\n\
         \x20   return\n";

    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
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
fn preserves_trailing_comments_on_multiline_statements() {
    let source = "module app\n\
         fn run()\n\
         \x20   throw Error(\n\
         \x20       code: \"book.absent\",\n\
         \x20       message: \"missing book\",\n\
         \x20   ) ; retained rationale\n";
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   throw Error(\n\
         \x20       code: \"book.absent\",\n\
         \x20       message: \"missing book\",\n\
         \x20   ) ; retained rationale\n";

    assert_eq!(format_source(source), expected);

    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![(
            "retained rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line,
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_compound_statement_headers() {
    let source = "module app\n\
         fn run()\n\
         \x20   if isReady(\n\
         \x20       value: 1,\n\
         \x20   ) ; header rationale\n\
         \x20       return\n";
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   if isReady(\n\
         \x20       value: 1,\n\
         \x20   ) ; header rationale\n\
         \x20       return\n";

    assert_eq!(format_source(source), expected);

    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![(
            "header rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line,
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_if_clauses() {
    let source = "module app\n\
         fn run()\n\
         \x20   if ready ; if rationale\n\
         \x20       return\n\
         \x20   else if fallback ; elseif rationale\n\
         \x20       return\n\
         \x20   else ; else rationale\n\
         \x20       return\n";
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   if ready ; if rationale\n\
         \x20       return\n\
         \x20   else if fallback ; elseif rationale\n\
         \x20       return\n\
         \x20   else ; else rationale\n\
         \x20       return\n";

    assert_eq!(format_source(source), expected);

    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![
            (
                "if rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line
            ),
            (
                "elseif rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
            (
                "else rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
        ]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_try_catch_headers() {
    let source = "module app\n\
         fn run()\n\
         \x20   try ; try rationale\n\
         \x20       return\n\
         \x20   catch err: Error ; catch rationale\n\
         \x20       return\n";
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   try ; try rationale\n\
         \x20       return\n\
         \x20   catch err: Error ; catch rationale\n\
         \x20       return\n";

    assert_eq!(format_source(source), expected);

    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![
            (
                "try rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
            (
                "catch rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
        ]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_match_arm_headers() {
    let source = "module app\n\
         fn run()\n\
         \x20   match status\n\
         \x20       active ; active rationale\n\
         \x20           return\n\
         \x20       inactive\n\
         \x20           return\n";
    let expected = "module app\n\
         \n\
         fn run()\n\
         \x20   match status\n\
         \x20       active ; active rationale\n\
         \x20           return\n\
         \x20       inactive\n\
         \x20           return\n";

    assert_eq!(format_source(source), expected);

    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![(
            "active rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line,
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
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
fn preserves_evolve_step_comments() {
    let source = "module app\n\
         evolve\n\
         \x20   ; choose a durable rename\n\
         \x20   rename Book.title -> Book.subtitle ; keep rename rationale\n\
         \x20   transform Book.shelf ; transform rationale\n\
         \x20       ; body rationale\n\
         \x20       return ^books(1).shelf\n";
    let expected = "module app\n\
         \n\
         evolve\n\
         \x20   ; choose a durable rename\n\
         \x20   rename Book.title -> Book.subtitle ; keep rename rationale\n\
         \x20   transform Book.shelf ; transform rationale\n\
         \x20       ; body rationale\n\
         \x20       return ^books(1).shelf\n";

    assert_eq!(format_source(source), expected);

    let evolve = reparsed_evolve_decl(source);
    assert_eq!(
        comment_facts(&evolve.comments),
        vec![
            (
                "choose a durable rename",
                CommentPlacement::OwnLine,
                CommentMarker::Line,
            ),
            (
                "keep rename rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
            (
                "transform rationale",
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ),
        ]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_multiline_evolve_defaults() {
    let source = "module app\n\
         evolve\n\
         \x20   default Book.info = save(\n\
         \x20       title: \"x\",\n\
         \x20   ) ; default rationale\n";
    let expected = "module app\n\
         \n\
         evolve\n\
         \x20   default Book.info = save(\n\
         \x20       title: \"x\",\n\
         \x20   ) ; default rationale\n";

    assert_eq!(format_source(source), expected);

    let evolve = reparsed_evolve_decl(source);
    assert_eq!(
        comment_facts(&evolve.comments),
        vec![(
            "default rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line,
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_multiline_evolve_transform_targets() {
    let source = "module app\n\
         evolve\n\
         \x20   transform choose(\n\
         \x20       value: 1,\n\
         \x20   ) ; transform rationale\n\
         \x20       return\n";
    let expected = "module app\n\
         \n\
         evolve\n\
         \x20   transform choose(\n\
         \x20       value: 1,\n\
         \x20   ) ; transform rationale\n\
         \x20       return\n";

    assert_eq!(format_source(source), expected);

    let evolve = reparsed_evolve_decl(source);
    assert_eq!(
        comment_facts(&evolve.comments),
        vec![(
            "transform rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line,
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn comment_preservation_guard_rejects_unstable_rewrites() {
    let source = "module app\n\
         evolve\n\
         \x20   default Book.info = save(\n\
         \x20       title: \"x\",\n\
         \x20   ) ; default rationale\n";
    let unstable_rewrite = "module app\n\
         \n\
         evolve\n\
         \x20   default Book.info = save( ; default rationale\n\
         \x20   title: \"x\",\n\
         )\n";

    assert!(!format_preserves_comments(source, unstable_rewrite));
}

#[test]
fn evolve_block_format_is_idempotent() {
    let source = "module app\n\
         evolve\n\
         \x20   rename ^books -> ^archive\n";
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}
