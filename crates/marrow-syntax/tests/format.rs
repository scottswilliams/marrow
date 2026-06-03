use marrow_syntax::{Declaration, format_expression, format_source, parse_source};

/// Read every `module`-starting `.mw` block from the language reference (the
/// complete library files used as parser fixtures).
fn documented_module_files() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("language");
    let mut files = Vec::new();
    let mut entries = std::fs::read_dir(&dir)
        .expect("read docs/language")
        .map(|entry| entry.expect("entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read doc");
        let (mut in_block, mut src) = (false, String::new());
        for line in text.lines() {
            if line.trim() == "```mw" {
                in_block = true;
                src.clear();
                continue;
            }
            if line.trim() == "```" && in_block {
                if src.trim_start().starts_with("module ") {
                    files.push(src.clone());
                }
                in_block = false;
                continue;
            }
            if in_block {
                src.push_str(line);
                src.push('\n');
            }
        }
    }
    files
}

/// Format a single-declaration `module app` source through `format_source` and
/// return just that declaration's canonical text. `format_source` wraps the file
/// as `module app\n\n<decl>\n`, so stripping that frame exercises the same
/// declaration-formatting path the public entry point uses.
fn format_decl(source: &str, index: usize) -> String {
    assert_eq!(index, 0, "helper only supports a single declaration");
    let formatted = format_source(source);
    formatted
        .strip_prefix("module app\n\n")
        .and_then(|rest| rest.strip_suffix('\n'))
        .expect("format_source frames a single declaration as module app\\n\\n<decl>\\n")
        .to_string()
}

/// Format a single-function `module app` source through `format_source` and
/// return just the function body (the indented statements under the `fn` header),
/// matching what the old block-level helper produced.
fn format_function_body(source: &str) -> String {
    let decl = format_decl(source, 0);
    // `format_decl` yields `fn run(...)\n<body>`; drop the header line to leave
    // the body block the test asserts on.
    decl.split_once('\n')
        .map(|(_, body)| body.to_string())
        .expect("a function declaration has a header line and a body")
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
fn formats_concise_resource_at_as_split_resource_and_store() {
    let source = "module app\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
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
        "save(book: draft, out result, inout total)",
        "60 * 60 + 1",
        "a and b or c",
        "not ready",
        "-count",
        "first _ last",
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
fn formats_labeled_loops_and_break_label() {
    let source = "module app\n\
         fn run()\n\
         \x20   outer: for id in keys(^books)\n\
         \x20       inner: while ready\n\
         \x20           break outer\n";
    let expected = "\
         \x20   outer: for id in keys(^books)\n\
         \x20       inner: while ready\n\
         \x20           break outer";
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
fn formats_transaction_lock_and_try_blocks() {
    let source = "module app\n\
         fn commit(id: Id(^books))\n\
         \x20   lock ^books(id)\n\
         \x20       transaction\n\
         \x20           ^books(id).title = title\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err: Error\n\
         \x20       print(err.message)\n\
         \x20   finally\n\
         \x20       cleanup()\n";
    let expected = "\
         \x20   lock ^books(id)\n\
         \x20       transaction\n\
         \x20           ^books(id).title = title\n\
         \x20   try\n\
         \x20       risky()\n\
         \x20   catch err: Error\n\
         \x20       print(err.message)\n\
         \x20   finally\n\
         \x20       cleanup()";
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
    assert_eq!(format_decl(source, 0), expected);
}

#[test]
fn formats_empty_doc_comment_lines_without_trailing_whitespace() {
    let source = "module app\n\
         ;; First paragraph.\n\
         ;;\n\
         ;; Second paragraph.\n\
         const MaxLoans: int = 5\n";
    let expected = ";; First paragraph.\n\
         ;;\n\
         ;; Second paragraph.\n\
         const MaxLoans: int = 5";
    let formatted = format_decl(source, 0);
    assert_eq!(formatted, expected);
    assert!(
        formatted.lines().all(|line| !line.ends_with(' ')),
        "formatter output contains trailing whitespace:\n{formatted:?}"
    );
}

#[test]
fn formats_resource_declaration_with_members() {
    let source = "module app\n\
         resource Book at ^books(id: int)\n\
         \x20   ;; Display title.\n\
         \x20   required title: string\n\
         \x20   tags(pos: int): string\n\
         \x20   notes(noteId: string)\n\
         \x20       text: string\n\
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
         pub fn add(title: string, out result: int): int\n\
         \x20   result = 1\n\
         \x20   return result\n";
    let expected = "pub fn add(title: string, out result: int): int\n\
         \x20   result = 1\n\
         \x20   return result";
    assert_eq!(format_decl(source, 0), expected);
}

#[test]
fn formats_whole_file_with_blank_line_policy() {
    let source = "module shelf::books\n\
         use std::clock\n\
         use shelf::books\n\
         const MaxLoans: int = 5\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
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

#[test]
fn format_source_preserves_structure_and_reparses_cleanly() {
    let files = documented_module_files();
    assert!(files.len() >= 5, "expected several module files");
    for source in files {
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
fn preserves_leading_standalone_and_trailing_comments() {
    // A function body with a leading comment (own line before a statement), a
    // trailing comment (after code on a statement line), and a standalone
    // comment (own line with no following statement) must round-trip.
    let source = "module app\n\
         fn run()\n\
         \x20   ; set up the total\n\
         \x20   const total: int = 0\n\
         \x20   print(total) ; show it\n\
         \x20   ; nothing left to do\n";
    let expected = "\
         \x20   ; set up the total\n\
         \x20   const total: int = 0\n\
         \x20   print(total) ; show it\n\
         \x20   ; nothing left to do";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn preserves_body_doc_comments_as_ordinary_comments() {
    let source = "module app\n\
         fn run()\n\
         \x20   ;; first comment\n\
         \x20   print(\"a\")\n\
         \x20   ;; second comment\n\
         \x20   print(\"b\")\n";
    let expected = "\
         \x20   ; first comment\n\
         \x20   print(\"a\")\n\
         \x20   ; second comment\n\
         \x20   print(\"b\")";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn preserves_comments_in_nested_blocks() {
    let source = "module app\n\
         fn run(n: int)\n\
         \x20   if n < 0\n\
         \x20       ; negative branch\n\
         \x20       print(\"neg\") ; report\n\
         \x20   ; after the if\n\
         \x20   return\n";
    let expected = "\
         \x20   if n < 0\n\
         \x20       ; negative branch\n\
         \x20       print(\"neg\") ; report\n\
         \x20   ; after the if\n\
         \x20   return";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn comment_preservation_round_trips_and_is_idempotent() {
    let source = "module app\n\
         fn run()\n\
         \x20   ; leading\n\
         \x20   const x: int = 1 ; trailing\n\
         \x20   ; standalone\n";
    let body = format_function_body(source);
    // Reparsing the formatted body and reformatting yields identical text, and
    // the comments are still present.
    let reformatted = format_function_body(&format!("module app\nfn run()\n{body}\n"));
    assert_eq!(body, reformatted, "comment formatting is not a fixed point");
    assert!(body.contains("; leading"));
    assert!(body.contains("; trailing"));
    assert!(body.contains("; standalone"));
}

#[test]
fn documented_parameters_format_one_per_line() {
    let source = "module app\n\
         fn f(\n\
         \x20   ;; the book to file\n\
         \x20   book: int,\n\
         \x20   ;; shelf it is filed under\n\
         \x20   shelf: string,\n\
         )\n\
         \x20   return\n";
    let decl = format_decl(source, 0);
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
    // A signature with parameter docs survives parse -> format and is a fixed
    // point: reformatting the output yields identical text and the docs persist.
    let source = "module app\n\
         fn f(\n\
         \x20   ;; first line\n\
         \x20   ;; second line\n\
         \x20   book: int,\n\
         \x20   shelf: string,\n\
         )\n\
         \x20   return\n";
    let once = format_source(source);
    let twice = format_source(&once);
    assert_eq!(
        once, twice,
        "documented signature formatting is not a fixed point"
    );
    assert!(once.contains(";; first line"));
    assert!(once.contains(";; second line"));
    assert!(once.contains("book: int,"));
    assert!(once.contains("shelf: string,"));
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
