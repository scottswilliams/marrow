use marrow_syntax::{Declaration, format_expression, parse_source};

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
