use marrow_syntax::{
    Declaration, format_block, format_declaration, format_expression, format_source, parse_source,
};

/// Read every `module`-starting `.mw` block from `docs/language` (the complete
/// library files used as parser fixtures).
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

/// Parse a module and format the declaration at `index`.
fn format_decl(source: &str, index: usize) -> String {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse cleanly: {:#?}",
        parsed.diagnostics
    );
    format_declaration(source, &parsed.file.declarations[index])
}

/// Parse a single-function module and format its body block at indent level 1.
fn format_function_body(source: &str) -> String {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse cleanly: {:#?}",
        parsed.diagnostics
    );
    let Declaration::Function(function) = &parsed.file.declarations[0] else {
        panic!("expected a function declaration");
    };
    format_block(source, &function.body, 1)
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
    format_expression(&decl.value)
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
        "Book::Id(17)",
        "save(book: draft, out result, inout total)",
        "60 * 60 + 1",
        "a and b or c",
        "not ready",
        "-count",
        "first _ last",
        "1..10",
        "1..=10",
        "a = b",
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
         \x20   let total: int = 0\n\
         \x20   var seen(id: int): bool\n\
         \x20   if n < 0\n\
         \x20       print(\"neg\")\n\
         \x20   else if n = 0\n\
         \x20       print(\"zero\")\n\
         \x20   else\n\
         \x20       total = total + n\n\
         \x20   for id in keys(^books)\n\
         \x20       delete ^books(id)\n\
         \x20   return total\n";
    let expected = "\
         \x20   let total: int = 0\n\
         \x20   var seen(id: int): bool\n\
         \x20   if n < 0\n\
         \x20       print(\"neg\")\n\
         \x20   else if n = 0\n\
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
fn formats_transaction_lock_and_try_blocks() {
    let source = "module app\n\
         fn commit(id: Book::Id)\n\
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
fn formats_const_declaration_with_docs() {
    let source = "module app\n\
         ;; The maximum number of loans.\n\
         const MaxLoans: int = 5\n";
    let expected = ";; The maximum number of loans.\n\
         const MaxLoans: int = 5";
    assert_eq!(format_decl(source, 0), expected);
}

#[test]
fn formats_resource_declaration_with_members() {
    let source = "module app\n\
         resource Book at ^books(id: int)\n\
         \x20   ;; Display title.\n\
         \x20   @id(\"book.title\")\n\
         \x20   required title: string\n\
         \x20   tags(pos: int): string\n\
         \x20   notes(noteId: string)\n\
         \x20       text: string\n\
         \x20   index byShelf(shelf, id) unique\n";
    let expected = "resource Book at ^books(id: int)\n\
         \x20   ;; Display title.\n\
         \x20   @id(\"book.title\")\n\
         \x20   required title: string\n\
         \x20   tags(pos: int): string\n\
         \x20   notes(noteId: string)\n\
         \x20       text: string\n\
         \x20   index byShelf(shelf, id) unique";
    assert_eq!(format_decl(source, 0), expected);
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
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \n\
         pub fn add(title: string): int\n\
         \x20   return 1\n";
    assert_eq!(format_source(source), expected);
}

#[test]
fn format_source_is_idempotent_and_reparses_cleanly() {
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
    }
}

#[test]
fn formatting_round_trips_through_the_parser() {
    // Formatting then re-parsing yields the same canonical text.
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
