//! Formatter goldens over the brace grammar. These exercise the public
//! `format_source`/`format_expression` path and own the exact canonical rendering of
//! each construct: `{ … }` blocks, cuddled and inline clauses, `=>` match arms,
//! bracket keys, angle generics, `//`/`///` comments, and the empty-body rule. The
//! comment-ownership invariants (MSY01) are pinned in `comment_ownership.rs`; here the
//! rendered text is the contract.

use crate::common;
use marrow_syntax::{
    Block, Comment, CommentMarker, CommentPlacement, Declaration, Statement, format_expression,
    format_preserves_comments, format_source, parse_source,
};

/// Format a single-declaration `module app` source and return just that
/// declaration's canonical text. `format_source` frames the file as
/// `module app\n\n<decl>\n`, so stripping that frame exercises the same declaration
/// path the public entry point uses.
fn format_decl(source: &str) -> String {
    let formatted = format_source(source);
    formatted
        .strip_prefix("module app\n\n")
        .and_then(|rest| rest.strip_suffix('\n'))
        .expect("format_source frames a single declaration as module app\\n\\n<decl>\\n")
        .to_string()
}

/// Format a single-function `module app` source and return just the function body:
/// the indented statements between the `fn … {` header and its closing `}`.
fn format_function_body(source: &str) -> String {
    let decl = format_decl(source);
    // `format_decl` yields `fn run(...) {\n<body>\n}`; drop the header line and the
    // closing brace line to leave the body block the test asserts on.
    let inner = decl
        .split_once('\n')
        .map(|(_, rest)| rest)
        .expect("a function declaration has a header line and a body");
    inner
        .strip_suffix("\n}")
        .expect("a braced function body ends with a closing brace line")
        .to_string()
}

/// Re-parse the formatter's output and hand back the `run` function's body block, so
/// comment round-trips assert on the typed `Block.comments` (text, placement, marker)
/// rather than substrings of the rendered text.
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

/// A span-independent structural fingerprint of a parsed file: its `Debug` rendering
/// with every `SourceSpan { ... }` region removed, so two files compare equal exactly
/// when their declarations, statements, nesting, and retained comments match.
fn structural_fingerprint(source: &str) -> String {
    let debug = format!("{:#?}", parse_source(source).file);
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug.as_str();
    while let Some(at) = rest.find("SourceSpan {") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "SourceSpan {".len()..];
        let close = after
            .find('}')
            .expect("SourceSpan debug has a closing brace");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

// ---- expressions ----

/// A deeply nested interpolation formats back to itself: the formatter reuses the
/// expression printer at each hole, so a three-deep nest round-trips exactly.
#[test]
fn formats_deeply_nested_interpolation() {
    let value = "$\"a{$\"b{$\"c{x}d\"}e\"}f\"";
    assert_eq!(format_const_value(value), value);
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
        // Keyed durable access uses square brackets; a call keeps parentheses.
        "^books[id].title",
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
        // Duration word literals, already in number agreement.
        "1 day",
        "3 days",
        "1 second",
        "2 weeks",
        // Interval membership: comparison-level, range right operand.
        "x in 0..10",
        "x in 0..=10",
        "x not in 0..100",
        "score in low..high",
    ];
    for source in canonical {
        assert_eq!(format_const_value(source), source, "input {source:?}");
    }
}

#[test]
fn formats_duration_word_literals_in_number_agreement() {
    // The formatter rewrites the unit to agree with the count: singular for `1`,
    // plural otherwise. It is idempotent — an already-agreeing literal is unchanged.
    assert_eq!(format_const_value("1 days"), "1 day");
    assert_eq!(format_const_value("2 day"), "2 days");
    assert_eq!(format_const_value("1 hour"), "1 hour");
    assert_eq!(format_const_value("7 week"), "7 weeks");
    assert_eq!(format_const_value("1 minute"), "1 minute");
}

#[test]
fn reinserts_minimal_parentheses_for_precedence() {
    // The syntax tree drops parentheses; the formatter restores only those required
    // to preserve the parsed grouping.
    let cases = [
        ("(1 + 2) * 3", "(1 + 2) * 3"),
        ("3 * (1 + 2)", "3 * (1 + 2)"),
        ("1 + (2 * 3)", "1 + 2 * 3"),
        ("(a)", "a"),
        ("-(a + b)", "-(a + b)"),
        ("not (a and b)", "not (a and b)"),
        ("(a or b) and c", "(a or b) and c"),
        ("(count ?? 0) < 5", "count ?? 0 < 5"),
        ("(start ?? 1)..n", "start ?? 1..n"),
        ("a ?? b ?? c", "a ?? b ?? c"),
        ("a ?? (b ?? c)", "a ?? b ?? c"),
        ("(a ?? b) ?? c", "(a ?? b) ?? c"),
    ];
    for (input, expected) in cases {
        assert_eq!(format_const_value(input), expected, "input {input:?}");
    }
}

#[test]
fn formatting_is_a_stable_fixed_point() {
    let inputs = [
        "60 * 60 + 1",
        "(1 + 2) * 3",
        "f(a, b: 2)",
        "^books[id].title",
        "not a or b",
    ];
    for input in inputs {
        let once = format_const_value(input);
        let twice = format_const_value(&once);
        assert_eq!(once, twice, "formatting not stable for {input:?}");
    }
}

// ---- statement blocks ----

#[test]
fn formats_statement_blocks_with_braces() {
    let source = "module app\nfn run(n: int) {\n    const total: int = 0\n    var seen[id: int]: bool\n    if n < 0 {\n        print(\"neg\")\n    } else if n == 0 {\n        print(\"zero\")\n    } else {\n        total = total + n\n    }\n    for id in keys(^books) {\n        delete ^books[id]\n    }\n    return total\n}\n";
    let expected = "    const total: int = 0\n    var seen[id: int]: bool\n    if n < 0 {\n        print(\"neg\")\n    } else if n == 0 {\n        print(\"zero\")\n    } else {\n        total += n\n    }\n    for id in keys(^books) {\n        delete ^books[id]\n    }\n    return total";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_compound_assignment_canonically() {
    let source = "module app\nfn run() {\n    count*=3\n    total+=count\n}\n";
    assert_eq!(
        format_function_body(source),
        "    count *= 3\n    total += count"
    );
}

/// A left-anchored `x = x <op> e` over a plain local name folds to the canonical
/// compound form `x <op>= e` for each of the five arithmetic operators. The fold
/// is a pure surface rewrite of an equivalent statement; the right operand is
/// re-rendered unchanged.
#[test]
fn folds_left_anchored_self_update_to_compound_assign() {
    let cases = [
        ("s = s + i", "    s += i"),
        ("s = s - i", "    s -= i"),
        ("s = s * 2", "    s *= 2"),
        ("s = s / 2", "    s /= 2"),
        ("s = s % 2", "    s %= 2"),
        ("s = s + a * b", "    s += a * b"),
    ];
    for (stmt, expected) in cases {
        let source = format!("module app\nfn run() {{\n    {stmt}\n}}\n");
        assert_eq!(format_function_body(&source), expected, "input {stmt:?}");
    }
}

/// The fold is conservative: it fires only when the target and the binary's left
/// operand are the same plain local name and the operator has a compound form.
/// A right-anchored form, a different name, a nested left operand
/// (`x = x + a + b` parses as `(x + a) + b`), a field or index target, and a
/// non-arithmetic operator all keep the explicit `=` assignment.
#[test]
fn leaves_non_self_update_assignments_as_explicit() {
    let unchanged = [
        "n = 1 + n",             // right-anchored: `n -= .. ` would differ for `-`/`/`
        "n = m + 1",             // different name
        "n = n + a + b",         // nested left operand `(n + a)` is not the bare name
        "r.count = r.count + 1", // field target: reading the path is not a bare local
        "xs[i] = xs[i] + 1",     // index target
        "flag = flag and ready", // `and` has no compound form
    ];
    for stmt in unchanged {
        let source = format!("module app\nfn run() {{\n    {stmt}\n}}\n");
        assert_eq!(
            format_function_body(&source),
            format!("    {stmt}"),
            "input {stmt:?} must not fold"
        );
    }
}

/// Folding is idempotent and its output re-parses to a compound-assign statement:
/// formatting the folded form again is a fixed point, and the canonical text is
/// itself a valid compound assignment (span-erased reparse equality).
#[test]
fn compound_assign_fold_is_idempotent_and_reparses() {
    let source = "module app\nfn run() {\n    s = s + i\n}\n";
    let once = format_source(source);
    let twice = format_source(&once);
    assert_eq!(once, twice, "fold must be a fixed point");

    let body = reparsed_run_body(&once);
    assert!(
        matches!(
            body.statements.as_slice(),
            [Statement::CompoundAssign { .. }]
        ),
        "folded output must re-parse to a compound assignment: {:#?}",
        body.statements
    );
    // The already-compound spelling formats to the identical canonical text.
    let already = "module app\nfn run() {\n    s += i\n}\n";
    assert_eq!(format_source(already), once);
}

/// The `use` block is formatter-owned: imports render sorted by module path,
/// deduplicated, and one per line, regardless of their source order or repetition.
#[test]
fn sorts_and_deduplicates_the_use_block() {
    let source = "module app\n\nuse shelf::books\nuse catalog::isbn\nuse shelf::books\n\nfn f(): int {\n    return 0\n}\n";
    let expected =
        "module app\n\nuse catalog::isbn\nuse shelf::books\n\nfn f(): int {\n    return 0\n}\n";
    assert_eq!(format_source(source), expected);
}

/// Sorting and collapsing the `use` block is a fixed point: formatting the
/// canonical block again leaves it unchanged.
#[test]
fn use_block_formatting_is_idempotent() {
    let source = "module app\n\nuse shelf::b\nuse shelf::a\nuse shelf::a\n\nfn f(): int {\n    return 0\n}\n";
    let once = format_source(source);
    assert_eq!(
        once,
        format_source(&once),
        "use-block sort is not a fixed point"
    );
}

/// The formatter owns only the `use` block; it never reorders declarations. With
/// the imports written out of order and the functions written second-then-first,
/// the imports sort but the functions keep their source order.
#[test]
fn use_block_sort_never_reorders_declarations() {
    let source = "module app\n\nuse z::mod\nuse a::mod\n\nfn second(): int {\n    return 2\n}\n\nfn first(): int {\n    return 1\n}\n";
    let formatted = format_source(source);
    assert!(
        formatted.contains("use a::mod\nuse z::mod"),
        "imports must sort:\n{formatted}"
    );
    let second_at = formatted.find("fn second").expect("second present");
    let first_at = formatted.find("fn first").expect("first present");
    assert!(
        second_at < first_at,
        "declaration order must be preserved:\n{formatted}"
    );
}

#[test]
fn formats_loops_and_unlabeled_break() {
    let source = "module app\nfn run() {\n    for id in keys(^books) {\n        while ready {\n            break\n        }\n    }\n}\n";
    let expected =
        "    for id in keys(^books) {\n        while ready {\n            break\n        }\n    }";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_a_range_for_with_a_by_step() {
    let source =
        "module app\nfn run() {\n    for i in 10..=1 by -2 {\n        print($\"{i}\")\n    }\n}\n";
    let expected = "    for i in 10..=1 by -2 {\n        print($\"{i}\")\n    }";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_transaction_and_prefix_try() {
    let source = "module app\nfn commit(id: Id(^books)): Result<int, string> {\n    transaction {\n        ^books[id].title = title\n    }\n    const x = try risky()\n    return ok(x)\n}\n";
    let expected = "    transaction {\n        ^books[id].title = title\n    }\n    const x = try risky()\n    return ok(x)";
    assert_eq!(format_function_body(source), expected);
}

// ---- match ----

#[test]
fn formats_a_match_with_bare_member_arms() {
    let source = "module app\nfn label(s: Status) {\n    match s {\n        active => print(\"a\")\n        archived => print(\"b\")\n    }\n}\n";
    let expected =
        "    match s {\n        active => print(\"a\")\n        archived => print(\"b\")\n    }";
    assert_eq!(format_function_body(source), expected);
}

#[test]
fn formats_a_match_with_qualified_member_path_arms() {
    let source = "module app\nfn label(c: Cat) {\n    match c {\n        tiger::bengal => print(\"a\")\n        lion => print(\"b\")\n    }\n}\n";
    let expected =
        "    match c {\n        tiger::bengal => print(\"a\")\n        lion => print(\"b\")\n    }";
    assert_eq!(format_function_body(source), expected);
}

/// A braced arm body with several statements stays braced; a single-statement arm
/// renders inline after `=>`.
#[test]
fn formats_a_match_with_inline_and_braced_arms() {
    let source = "module app\nfn area(s: Shape): int {\n    match s {\n        dot => return 0\n        circle(r) => {\n            log(r)\n            return r\n        }\n    }\n}\n";
    let expected = "    match s {\n        dot => return 0\n        circle(r) => {\n            log(r)\n            return r\n        }\n    }";
    assert_eq!(format_function_body(source), expected);
}

/// A single blank line between sibling match arms groups them exactly as it groups
/// statements and members: one blank is preserved, arms with no blank stay tight, and
/// the result is idempotent.
#[test]
fn preserves_grouping_blank_between_match_arms() {
    let source = "module app\nfn label(s: Status) {\n    match s {\n        active => print(\"a\")\n\n        archived => print(\"b\")\n        deleted => print(\"c\")\n    }\n}\n";
    let expected = "    match s {\n        active => print(\"a\")\n\n        archived => print(\"b\")\n        deleted => print(\"c\")\n    }";
    assert_eq!(format_function_body(source), expected);
    let once = format_source(source);
    assert_eq!(
        format_source(&once),
        once,
        "match-arm grouping blank is not idempotent"
    );
}

// ---- inline-diverging cuddle integrity ----

/// An `if`/`else if` then-branch always keeps its braces, even inline: the braceless
/// spelling `else if c return` is not legal grammar and would not re-parse. An
/// eligible diverging single statement renders braced-inline (`{ return }`), never
/// braceless; a bare `else` keeps its braceless-inline form. Because the braced-inline
/// form still carries a closing `}`, every clause — including a terminal `else if`
/// with no trailing `else` — cuddles a following keyword and re-parses.
#[test]
fn an_else_if_then_branch_keeps_its_braces() {
    let source = "module app\nfn run(a: int): int {\n    if a > 0 {\n        return 1\n    } else if a == 0 {\n        return 0\n    } else if a == 1 {\n        return 2\n    }\n}\n";
    let once = format_source(source);
    assert!(
        once.contains("} else if a == 0 { return 0 }"),
        "a middle else-if renders braced-inline:\n{once}"
    );
    assert!(
        once.contains("} else if a == 1 { return 2 }"),
        "a terminal else-if keeps its braces rather than going braceless:\n{once}"
    );
    assert!(
        !parse_source(&once).has_errors(),
        "formatted if-chain must re-parse:\n{once}"
    );
    assert_eq!(
        format_source(&once),
        once,
        "if-chain rendering is not idempotent:\n{once}"
    );
}

/// The same cuddle rule governs `checked` arms: an `on out_of_range` arm followed by
/// an `on zero_divisor` arm may not inline, or the second `on` keyword has no `}` to
/// cuddle and the render does not re-parse.
#[test]
fn a_non_final_checked_arm_never_inlines() {
    let source = "module app\nfn run(a: int, b: int): int {\n    return checked a / b\n    on out_of_range {\n        return 0\n    }\n    on zero_divisor {\n        return -1\n    }\n}\n";
    let once = format_source(source);
    assert!(
        once.contains("on out_of_range {"),
        "the non-final checked arm keeps its braced body:\n{once}"
    );
    assert!(
        !parse_source(&once).has_errors(),
        "formatted checked arms must re-parse:\n{once}"
    );
    assert_eq!(
        format_source(&once),
        once,
        "checked-arm rendering is not idempotent:\n{once}"
    );
}

// ---- empty-body rule ----

/// A mandatory block renders `{}` when empty; a member-less `store` stays
/// header-alone. Both forms re-parse and re-format to themselves.
#[test]
fn empty_bodies_follow_the_mandatory_block_rule() {
    let cases = [
        ("module app\nfn run() {}\n", "fn run() {}"),
        (
            "module app\nfn run() {\n    transaction {}\n}\n",
            "    transaction {}",
        ),
        ("module app\nfn run() {\n    if c {}\n}\n", "    if c {}"),
        (
            "module app\nfn run(s: Shape) {\n    match s {}\n}\n",
            "    match s {}",
        ),
    ];
    for (source, fragment) in cases {
        let once = format_source(source);
        assert!(once.contains(fragment), "expected `{fragment}` in:\n{once}");
        assert_eq!(
            format_source(&once),
            once,
            "empty-body render is not idempotent:\n{once}"
        );
        assert!(
            !parse_source(&once).has_errors(),
            "empty-body render must re-parse:\n{once}"
        );
    }
    let store = "module app\nresource B {\n    t: string\n}\nstore ^b: B\n";
    assert!(format_source(store).contains("store ^b: B\n"));
    assert!(!format_source(store).contains("store ^b: B {"));
}

/// A resource group mandates a `{ … }` body, so an empty group renders `{}` and
/// re-parses. Regression for the formatter fuzz oracle counterexample minimized
/// from seed 999, where an empty group had rendered header-alone and the output
/// no longer parsed. Both the plain and the keyed group form are pinned.
#[test]
fn empty_resource_group_renders_braces_and_reparses() {
    let cases = [
        ("module app\nresource R {\n    g {}\n}\n", "    g {}"),
        (
            "module app\nresource R {\n    notes[n: string] {}\n}\n",
            "    notes[n: string] {}",
        ),
    ];
    for (source, fragment) in cases {
        let once = format_source(source);
        assert!(once.contains(fragment), "expected `{fragment}` in:\n{once}");
        assert_eq!(
            format_source(&once),
            once,
            "empty-group render is not idempotent:\n{once}"
        );
        assert!(
            !parse_source(&once).has_errors(),
            "empty-group render must re-parse:\n{once}"
        );
    }
}

// ---- declarations ----

#[test]
fn formats_const_declaration_with_docs() {
    let source = "module app\n/// The maximum number of loans.\nconst MaxLoans: int = 5\n";
    let expected = "/// The maximum number of loans.\nconst MaxLoans: int = 5";
    assert_eq!(format_decl(source), expected);
}

#[test]
fn formats_empty_doc_comment_lines_without_trailing_whitespace() {
    // A blank line between doc paragraphs renders as a bare `///` with no trailing
    // space; the structural checks pin the two facts behind the golden.
    let source =
        "module app\n/// First paragraph.\n///\n/// Second paragraph.\nconst MaxLoans: int = 5\n";
    let expected = "/// First paragraph.\n///\n/// Second paragraph.\nconst MaxLoans: int = 5";
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
    let source = "module app\nresource Book {\n    /// Display title.\n    required title: string\n    tags[pos: int]: string\n    notes[noteId: string] {\n        text: string\n    }\n}\nstore ^books[id: int]: Book {\n    index byShelf[shelf, id] unique\n}\n";
    let expected = "module app\n\nresource Book {\n    /// Display title.\n    required title: string\n    tags[pos: int]: string\n    notes[noteId: string] {\n        text: string\n    }\n}\n\nstore ^books[id: int]: Book {\n    index byShelf[shelf, id] unique\n}";
    assert_eq!(format_source(source).trim_end(), expected);
}

/// A resource and the store that follows it each brace their own body; formatting is
/// a fixed point across the pair.
#[test]
fn formats_a_resource_then_store_pair() {
    let source = "module app\nresource Book {\n    required title: string\n}\nstore ^books[id: int]: Book {\n    index byTitle[title, id]\n}\n";
    let expected = "module app\n\nresource Book {\n    required title: string\n}\n\nstore ^books[id: int]: Book {\n    index byTitle[title, id]\n}\n";
    assert_eq!(format_source(source), expected);
    assert_eq!(format_source(expected), expected);
}

#[test]
fn formats_function_declaration_with_params() {
    let source = "module app\npub fn add(title: string, total: int): int {\n    return total\n}\n";
    let expected = "pub fn add(title: string, total: int): int {\n    return total\n}";
    assert_eq!(format_decl(source), expected);
}

#[test]
fn formats_optional_function_return_and_absent_value() {
    let source = "module app\nfn f(): int? {\n    return absent\n}\n";
    assert_eq!(
        format_source(source),
        "module app\n\nfn f(): int? {\n    return absent\n}\n"
    );
}

#[test]
fn formats_whole_file_with_blank_line_policy() {
    let source = "module shelf::books\nuse std::clock\nuse shelf::books\nconst MaxLoans: int = 5\nresource Book {\n    required title: string\n}\nstore ^books[id: int]: Book\npub fn add(title: string): int {\n    return 1\n}\n";
    let expected = "module shelf::books\n\nuse shelf::books\nuse std::clock\n\nconst MaxLoans: int = 5\n\nresource Book {\n    required title: string\n}\n\nstore ^books[id: int]: Book\n\npub fn add(title: string): int {\n    return 1\n}\n";
    assert_eq!(format_source(source), expected);
}

// ---- B5/B6 canonical rendering ----

/// A B5 chained `if const` head renders its bindings joined by `and`, with the
/// optional trailing condition last, and is a fixed point.
#[test]
fn formats_if_const_chain_canonically() {
    let source = "module app\nfn run(): int {\n    if const a = ^c[1].v and const b = ^c[2].v and a < b {\n        return 1\n    }\n    return 0\n}\n";
    let expected = "    if const a = ^c[1].v and const b = ^c[2].v and a < b {\n        return 1\n    }\n    return 0";
    assert_eq!(format_function_body(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
    // No longer a verbatim echo: the render is regenerated from the AST.
    let Statement::IfConstChain { bindings, .. } = &reparsed_run_body(source).statements[0] else {
        panic!("expected an if-const chain");
    };
    assert_eq!(bindings.len(), 2);
}

/// A B6 let-else renders inline when its else body is a single diverging statement,
/// and braced otherwise; both are fixed points.
#[test]
fn formats_let_else_canonically() {
    let inline =
        "module app\nfn run(): int {\n    const x = ^c[1].v else return -1\n    return x\n}\n";
    assert_eq!(
        format_function_body(inline),
        "    const x = ^c[1].v else return -1\n    return x"
    );
    let braced = "module app\nfn run(): int {\n    const x = ^c[1].v else {\n        log(\"x\")\n        return -1\n    }\n    return x\n}\n";
    assert_eq!(
        format_function_body(braced),
        "    const x = ^c[1].v else {\n        log(\"x\")\n        return -1\n    }\n    return x"
    );
    for source in [inline, braced] {
        let once = format_source(source);
        assert_eq!(format_source(&once), once);
        let Statement::LetElse { .. } = &reparsed_run_body(source).statements[0] else {
            panic!("expected a let-else");
        };
    }
}

// ---- multiline call layout ----

/// A trailing comma forces the inner call multiline, and a call wrapping a multiline
/// argument must expand too; a single pass is already a fixed point.
#[test]
fn single_line_call_wrapping_a_trailing_comma_call_is_idempotent() {
    let source = "module app\nfn run() {\n    print(h(g(a: 1, b: 2,)))\n}\n";
    let once = format_source(source);
    assert_eq!(format_source(&once), once, "not idempotent:\n{once}");
    assert!(format_preserves_comments(source, &once));
}

/// A string interpolation is lexed within one line, so an embedded call never expands
/// across lines; the formatter keeps it inline and idempotent.
#[test]
fn trailing_comma_call_inside_interpolation_is_idempotent() {
    let cases = [
        (
            "module app\nfn run() {\n    x = $\"a{g(a: 1,)}b\"\n}\n",
            "$\"a{g(a: 1)}b\"",
        ),
        (
            "module app\nfn run() {\n    print($\"a{g(a: 1, b: 2,)}b\")\n}\n",
            "$\"a{g(a: 1, b: 2)}b\"",
        ),
    ];
    for (source, inline_interp) in cases {
        let once = format_source(source);
        assert!(
            once.contains(inline_interp),
            "embedded call must render inline:\n{once}"
        );
        assert_eq!(format_source(&once), once, "not idempotent:\n{once}");
        assert!(!parse_source(&once).has_errors(), "must re-parse:\n{once}");
    }
}

#[test]
fn preserves_multiline_trailing_comma_calls() {
    let source = "module app\nfn fail() {\n    log(\n        code: \"book.absent\",\n        message: \"missing book\",\n    )\n}\n";
    let expected = "module app\n\nfn fail() {\n    log(\n        code: \"book.absent\",\n        message: \"missing book\",\n    )\n}\n";
    assert_eq!(format_source(source), expected);
}

// ---- blank-line policy ----

/// A single blank line between statements or members is preserved, two or more
/// collapse to one, and a leading or trailing blank inside a body is dropped.
#[test]
fn preserves_single_intra_body_blank_line() {
    let source = "module app\nresource Book {\n    required title: string\n\n\n    loanedTo: string\n}\npub fn run() {\n\n    const a = 1\n\n    const b = 2\n\n}\n";
    let expected = "module app\n\nresource Book {\n    required title: string\n\n    loanedTo: string\n}\n\npub fn run() {\n    const a = 1\n\n    const b = 2\n}\n";
    assert_eq!(format_source(source), expected);
    assert_eq!(
        format_source(&format_source(source)),
        expected,
        "not idempotent"
    );
}

/// A `///` doc comment attached to a member carries the member's grouping blank line,
/// and the result is idempotent.
#[test]
fn preserves_blank_above_doc_commented_member() {
    let source = "module app\nresource Book {\n    required title: string\n\n    /// Who currently holds the book.\n    loanedTo: string\n}\n";
    let expected = "module app\n\nresource Book {\n    required title: string\n\n    /// Who currently holds the book.\n    loanedTo: string\n}\n";
    assert_eq!(format_source(source), expected);
    assert_eq!(
        format_source(&format_source(source)),
        expected,
        "not idempotent"
    );
}

#[test]
fn comment_after_blank_line_stays_attached_to_following_item() {
    let source =
        "module app\npub fn run() {\n    const a = 1\n\n    // about b\n    const b = 2\n}\n";
    let expected =
        "module app\n\npub fn run() {\n    const a = 1\n\n    // about b\n    const b = 2\n}\n";
    assert_eq!(format_source(source), expected);
}

#[test]
fn top_level_comment_after_blank_stays_with_following_decl_across_block_bearing_predecessors() {
    let predecessors = [
        "resource Item {\n    name: text\n}",
        "enum Color {\n    red\n    green\n}",
        "store ^items[id: text]: Item {\n    index by_name[name]\n}",
        "pub fn one() {\n    const a = 1\n}",
    ];
    for predecessor in predecessors {
        let source = format!(
            "module app\n\n{predecessor}\n\n// about two\npub fn two() {{\n    const b = 2\n}}\n"
        );
        let once = format_source(&source);
        assert!(
            once.contains("\n\n// about two\npub fn two()"),
            "comment detached after predecessor `{predecessor}`:\n{once}"
        );
        assert_eq!(
            format_source(&once),
            once,
            "not idempotent after `{predecessor}`:\n{once}"
        );
    }
}

#[test]
fn top_level_plain_comment_stays_glued_to_following_doc_comment() {
    let adjacent = "module app\n\n// a plain note\n/// the ceiling\nconst limit: int = 10\n";
    assert_eq!(format_source(adjacent), adjacent);
    assert_eq!(format_source(&format_source(adjacent)), adjacent);

    let section_break =
        "module app\n\n// a standalone note\n\n/// the ceiling\nconst limit: int = 10\n";
    assert_eq!(format_source(section_break), section_break);
    assert_eq!(format_source(&format_source(section_break)), section_break);
}

#[test]
fn keeps_standalone_doc_paragraph_separate_from_following_declaration_docs() {
    let source = "module app\n/// Module overview.\n///\n\n/// Stored books.\nresource Book {\n    title: string\n}\n";
    let expected = "module app\n\n/// Module overview.\n///\n\n/// Stored books.\nresource Book {\n    title: string\n}\n";
    assert_eq!(format_source(source), expected);
}

// ---- comment attachment ----

#[test]
fn round_trips_ordinary_line_comments_by_placement() {
    let source = "module app\nfn run() {\n    // set up the total\n    const total: int = 0\n    print(total) // show it\n    // nothing left to do\n}\n";
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
    let recanonicalized = format_source(&format_source(source));
    assert_eq!(
        comment_facts(&reparsed_run_body(&recanonicalized).comments),
        expected
    );
}

/// An own-line comment renders at the block's canonical indent regardless of its
/// source column, so a reparse cannot misread it as opening a deeper block.
#[test]
fn renders_own_line_body_comments_at_block_indent() {
    let source = "module app\nfn run() {\n    print(\"before\")\n            // odd indent\n    print(\"after\")\n}\n";
    let expected = "module app\n\nfn run() {\n    print(\"before\")\n    // odd indent\n    print(\"after\")\n}\n";
    assert_eq!(format_source(source), expected);
    let body = reparsed_run_body(&format_source(source));
    assert_eq!(
        comment_facts(&body.comments),
        [("odd indent", CommentPlacement::OwnLine, CommentMarker::Line)]
    );
}

#[test]
fn rejects_body_doc_comments_at_parse() {
    // A `///` documents the next declaration or member; inside a function body there
    // is nothing to document, so it is a parse error rather than retained trivia.
    for source in [
        "module app\nfn run() {\n    /// orphan doc\n    print(\"a\")\n}\n",
        "module app\nfn run() {\n    const x: int = 1 /// trailing doc\n}\n",
    ] {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
            "a body doc comment must be a parse error: {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn round_trips_comments_attached_inside_nested_blocks() {
    // An own-line and a trailing comment inside the `if` belong to the then-block; the
    // comment after the `if` belongs to the outer body.
    let source = "module app\nfn run(n: int) {\n    if n < 0 {\n        // negative branch\n        print(\"neg\") // report\n    }\n    // after the if\n    return\n}\n";
    let then_expected = [
        (
            "negative branch",
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ),
        ("report", CommentPlacement::Trailing, CommentMarker::Line),
    ];
    let outer_expected = [(
        "after the if",
        CommentPlacement::OwnLine,
        CommentMarker::Line,
    )];

    let check = |source: &str| {
        let body = reparsed_run_body(source);
        assert_eq!(
            comment_facts(&body.comments),
            outer_expected,
            "outer body comments"
        );
        let Statement::If { then_block, .. } = &body.statements[0] else {
            panic!("first statement is the if: {:?}", body.statements[0]);
        };
        assert_eq!(
            comment_facts(&then_block.comments),
            then_expected,
            "then-block comments"
        );
    };
    check(source);
    check(&format_source(&format_source(source)));
}

// ---- parameter docs ----

#[test]
fn documented_parameters_format_one_per_line() {
    let source = "module app\nfn f(\n    /// the book to file\n    book: int,\n    /// shelf it is filed under\n    shelf: string,\n) {\n    return\n}\n";
    let expected = "fn f(\n    /// the book to file\n    book: int,\n    /// shelf it is filed under\n    shelf: string,\n) {\n    return\n}";
    assert_eq!(format_decl(source), expected);
}

#[test]
fn documented_parameter_signature_round_trips() {
    let source = "module app\nfn f(\n    /// first line\n    /// second line\n    book: int,\n    shelf: string,\n) {\n    return\n}\n";
    let param_docs = |source: &str| {
        let parsed = parse_source(source);
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        parsed
            .file
            .function("f")
            .expect("formatted source defines fn f")
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty.to_string(), param.docs.clone()))
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
    assert_eq!(
        format_source(&once),
        once,
        "signature formatting is not a fixed point"
    );
    assert_eq!(param_docs(&once), expected);
}

// ---- top-level and member comments ----

#[test]
fn preserves_top_level_and_member_line_comments() {
    let source = "module app\n// shared constants\nconst Max:int=5\n// stored records\nresource Book {\n    // visible label\n    title: string\n}\n";
    let expected = "module app\n\n// shared constants\nconst Max: int = 5\n\n// stored records\nresource Book {\n    // visible label\n    title: string\n}\n";
    assert_eq!(format_source(source), expected);
}

/// An indented top-level own-line comment re-renders at column 1, round-trips
/// without comment loss, and is a fixed point; both `//` and `///` are covered.
#[test]
fn preserves_indented_top_level_own_line_comments() {
    let source = "module app\n    // indented before first decl\nconst Max:int=5\n    /// indented between decls\nconst Min:int=0\n    // indented at end of file\n";
    let expected = "module app\n\n// indented before first decl\nconst Max: int = 5\n\n/// indented between decls\nconst Min: int = 0\n\n// indented at end of file\n";
    assert_eq!(format_source(source), expected);
    assert!(format_preserves_comments(source, expected));
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn rejects_indented_top_level_doc_comment_without_target() {
    let parsed = parse_source("module app\n    /// dangling doc at eof\n");
    assert!(
        parsed.diagnostics.iter().any(|d| d.code == "parse.syntax"),
        "an indented dangling doc comment must be a parse error: {:#?}",
        parsed.diagnostics
    );
}

// ---- header-trailing comments (MSY01 behavior) ----

/// A comment trailing a bodyless top-level header stays at the header's end; a comment
/// trailing a body-bearing header is owned by the block and renders as its first line.
#[test]
fn header_trailing_comments_route_by_body() {
    let source = "module app\nconst Max:int=5 // const rationale\n/// Stored books.\nresource Book { // resource rationale\n    title: string\n}\nstore ^books: Book // store rationale\nfn run() { // function rationale\n    return\n}\n";
    let expected = "module app\n\nconst Max: int = 5 // const rationale\n\n/// Stored books.\nresource Book {\n    // resource rationale\n    title: string\n}\n\nstore ^books: Book // store rationale\n\nfn run() {\n    // function rationale\n    return\n}\n";
    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

/// A comment trailing a member header routes the same way: a bodyless field, index, or
/// enum leaf keeps it trailing; a group or category owns it as its body's first line.
#[test]
fn member_header_trailing_comments_route_by_body() {
    let source = "module app\nresource Book {\n    details { // group rationale\n        required title: string // field rationale\n    }\n}\nstore ^books: Book {\n    index byTitle[title] // index rationale\n}\nenum Status {\n    category live { // category rationale\n        active // member rationale\n    }\n}\n";
    let expected = "module app\n\nresource Book {\n    details {\n        // group rationale\n        required title: string // field rationale\n    }\n}\n\nstore ^books: Book {\n    index byTitle[title] // index rationale\n}\n\nenum Status {\n    category live {\n        // category rationale\n        active // member rationale\n    }\n}\n";
    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_multiline_top_level_headers() {
    // A multiline bodyless const keeps its trailing comment after the close paren; a
    // multiline function header is body-bearing, so the comment moves inside.
    let source = "module app\nconst Info = save(\n    title: \"x\",\n) // const rationale\nfn f(\n    /// the book to file\n    book: int,\n) { // function rationale\n    return\n}\n";
    let expected = "module app\n\nconst Info = save(\n    title: \"x\",\n) // const rationale\n\nfn f(\n    /// the book to file\n    book: int,\n) {\n    // function rationale\n    return\n}\n";
    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_multiline_statements() {
    let source = "module app\nfn run() {\n    log(\n        code: \"book.absent\",\n        message: \"missing book\",\n    ) // retained rationale\n}\n";
    let expected = "module app\n\nfn run() {\n    log(\n        code: \"book.absent\",\n        message: \"missing book\",\n    ) // retained rationale\n}\n";
    assert_eq!(format_source(source), expected);
    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![(
            "retained rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn preserves_trailing_comments_on_prefix_try_statements() {
    let source = "module app\nfn run(): Result<int, string> {\n    const x = try risky() // try rationale\n    return ok(x)\n}\n";
    let expected = "module app\n\nfn run(): Result<int, string> {\n    const x = try risky() // try rationale\n    return ok(x)\n}\n";
    assert_eq!(format_source(source), expected);
    let body = reparsed_run_body(source);
    assert_eq!(
        comment_facts(&body.comments),
        vec![(
            "try rationale",
            CommentPlacement::Trailing,
            CommentMarker::Line
        )]
    );
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

/// A comment trailing a match-arm body statement stays on that statement; the inline
/// arm expands to a braced block so the comment has a home, and it is a fixed point.
#[test]
fn preserves_trailing_comments_on_match_arm_bodies() {
    let source = "module app\nfn run() {\n    match status {\n        active => return // active rationale\n        inactive => return\n    }\n}\n";
    let expected = "module app\n\nfn run() {\n    match status {\n        active => {\n            return // active rationale\n        }\n        inactive => return\n    }\n}\n";
    assert_eq!(format_source(source), expected);
    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}

#[test]
fn comment_preservation_guard_rejects_unstable_rewrites() {
    let source = "module app\n\nconst info = save(\n    title: \"x\",\n) // const rationale\n";
    let unstable_rewrite =
        "module app\n\nconst info = save( // const rationale\n    title: \"x\",\n)\n";
    assert!(!format_preserves_comments(source, unstable_rewrite));
}

// ---- corpus-dependent goldens ----

/// The canonical runnable sample is the conformance oracle; the documented
/// `sample.md` is in fmt-canonical form and formatting it is a fixed point.
#[test]
fn canonical_sample_is_already_fmt_canonical() {
    let source = common::reference_sample();
    assert_eq!(
        format_source(&source),
        source,
        "the canonical sample.md is not in fmt-canonical form"
    );
}

/// Corpus contract for the whole formatter over every documented source file:
/// `format_source` is a fixed point, re-parses cleanly, and preserves the
/// declaration tree.
#[test]
fn format_source_preserves_structure_and_reparses_cleanly() {
    let blocks = common::documented_source_blocks();
    assert!(blocks.len() >= 5, "expected several source files");
    for block in blocks {
        let source = block.source;
        let once = format_source(&source);
        assert_eq!(
            once,
            format_source(&once),
            "format_source is not a fixed point for:\n{source}"
        );
        assert!(
            !parse_source(&once).has_errors(),
            "formatted output should re-parse cleanly:\n{once}"
        );
        assert_eq!(
            structural_fingerprint(&source),
            structural_fingerprint(&once),
            "formatting changed the declaration tree for:\n{source}\n--- formatted ---\n{once}"
        );
    }
}

#[test]
fn check_format_is_the_one_owned_format_policy() {
    use marrow_syntax::{FormatRefusal, check_format};

    // Valid source formats to its canonical form.
    let formatted = check_format("pub fn f():int{\nreturn 1\n}\n").expect("valid source formats");
    assert!(formatted.contains("pub fn f(): int"), "got: {formatted:?}");
    // Idempotent: the formatted output re-formats to itself.
    assert_eq!(check_format(&formatted).expect("re-formats"), formatted);

    // Unparsed source is refused with its parse diagnostics carried.
    match check_format("pub fn f(: int {\n    return 1\n}\n") {
        Err(FormatRefusal::ParseInvalid(diagnostics)) => assert!(!diagnostics.is_empty()),
        other => panic!("expected ParseInvalid, got {other:?}"),
    }
}

#[test]
fn check_format_refuses_sources_carrying_recovery_nodes() {
    use marrow_syntax::{FormatRefusal, check_format};

    // A parser-owned recovery node (`base.`, `Enum::`) or an incomplete type
    // annotation always travels with its parse diagnostic, so `has_errors` is true
    // and the one format policy refuses before any node reaches the formatter's node
    // dispatch. No recovery-aware refusal logic lives in the formatter itself; the
    // existing `has_errors` gate is the sole guard.
    for source in [
        "pub fn f() {\n    return book.\n}\n",  // Recovery::Member
        "pub fn f() {\n    return book?.\n}\n", // Recovery::OptionalMember
        "pub fn f() {\n    return Role::\n}\n", // Recovery::Path
        "const Bad: = 5\n",                     // TypeExpr::Incomplete
    ] {
        assert!(
            parse_source(source).has_errors(),
            "expected a recovery-bearing source to have errors: {source:?}"
        );
        match check_format(source) {
            Err(FormatRefusal::ParseInvalid(diagnostics)) => assert!(
                !diagnostics.is_empty(),
                "refusal must carry the parse diagnostics: {source:?}"
            ),
            other => panic!("expected ParseInvalid for {source:?}, got {other:?}"),
        }
    }
}
