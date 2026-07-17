//! MSY01 — comment ownership over the brace grammar. A comment trailing a
//! body-bearing header, whether it cuddles the `{`, sits before a next-line `{`, or
//! opens the block on its own line, attaches to one deterministic owner: the block.
//! So `format(format(x)) == format(x)` and `parse(format(x))` preserves every
//! comment's marker, text, and attachment, for every position the grammar admits.

use crate::common::oracle::assert_formatter_faithful;
use marrow_syntax::{format_source, parse_source};

/// The strong contract for a comment-bearing program: it parses cleanly, its
/// canonical form is a fixed point, and formatting preserves every comment and the
/// declaration tree. `assert_formatter_faithful` folds all three together.
fn faithful(source: &str) {
    assert_formatter_faithful(source);
}

/// Wrap statement `body` in a `run` function so a statement-level comment case reads
/// as a whole program.
fn run(body: &str) -> String {
    format!("module app\nfn run(n: int) {{\n{body}}}\n")
}

// ---- header-trailing comment, every admitted spelling, converges ----

/// The three admitted spellings of a comment on a compound header — cuddled to the
/// `{`, before a next-line `{`, and opening the block on its own line — all format to
/// the same fixed point with the comment owned by the block.
#[test]
fn compound_header_comment_spellings_converge() {
    let cuddled = run("    if n < 0 { // note\n        return\n    }\n");
    let before_brace = run("    if n < 0 // note\n    {\n        return\n    }\n");
    let own_line = run("    if n < 0 {\n        // note\n        return\n    }\n");
    let canonical = format_source(&cuddled);
    assert_eq!(format_source(&before_brace), canonical);
    assert_eq!(format_source(&own_line), canonical);
    assert!(
        canonical.contains("if n < 0 {\n        // note\n        return"),
        "the header comment is owned by the block as its first line:\n{canonical}"
    );
    for source in [&cuddled, &before_brace, &own_line] {
        faithful(source);
    }
}

#[test]
fn header_comments_on_every_compound_form_round_trip() {
    for source in [
        run("    if n < 0 // note\n    {\n        return\n    }\n"),
        run("    while n < 0 // note\n    {\n        n = n\n    }\n"),
        run("    for i in 1..10 // note\n    {\n        n = i\n    }\n"),
        run("    transaction // note\n    {\n        n = n\n    }\n"),
        run(
            "    if n < 0 {\n        return\n    } else if n > 0 // note\n    {\n        return\n    }\n",
        ),
        run("    match n // note\n    {\n        dot => return\n    }\n"),
    ] {
        faithful(&source);
        let once = format_source(&source);
        assert_eq!(format_source(&once), once, "not idempotent:\n{once}");
    }
}

#[test]
fn declaration_header_comments_round_trip() {
    for source in [
        "module app\nfn run() // note\n{\n    return\n}\n".to_string(),
        "module app\nresource B // note\n{\n    t: string\n}\n".to_string(),
        "module app\nresource B {\n    t: string\n}\nstore ^b: B // note\n{\n    index byT[t]\n}\n"
            .to_string(),
        "module app\nenum E // note\n{\n    a\n    b\n}\n".to_string(),
        "module app\ntest \"x\" // note\n{\n    assert true\n}\n".to_string(),
    ] {
        faithful(&source);
    }
}

/// A comment cuddled after the opening `{` and a comment on the block's own first
/// line are both block-owned and land at the block's canonical indent.
#[test]
fn empty_and_comment_only_bodies_keep_the_comment() {
    for source in [
        run("    if n < 0 { // note\n    }\n"),
        run("    if n < 0 {\n        // only\n    }\n"),
        run("    if n < 0 // note\n    {\n    }\n"),
    ] {
        faithful(&source);
    }
}

// ---- match arms: inter-arm own-line and arm-trailing comments ----

#[test]
fn match_arm_comments_round_trip() {
    let source = run(
        "    match n {\n        // leading arm\n        dot => return // arm tr\n        \
         // between arms\n        circle => {\n            return\n        }\n    }\n",
    );
    faithful(&source);
    let once = format_source(&source);
    assert_eq!(format_source(&once), once, "not idempotent:\n{once}");
}

// ---- doc comments on members survive ----

#[test]
fn member_doc_comments_round_trip() {
    faithful("module app\nresource B {\n    /// the title\n    t: string\n}\n");
    faithful("module app\nenum E {\n    /// first\n    a\n    b\n}\n");
}

// ---- empty-body adjudication: mandatory blocks render `{}` ----

/// A construct whose grammar mandates a block renders `{}` when empty, and that form
/// re-parses and re-formats to itself.
#[test]
fn mandatory_blocks_render_empty_braces() {
    let cases = [
        ("module app\nfn run() {\n}\n", "fn run() {}"),
        ("module app\nfn run() {}\n", "fn run() {}"),
        (
            "module app\nfn run() {\n    transaction {\n    }\n}\n",
            "    transaction {}",
        ),
        (
            "module app\nfn run() {\n    if c {\n    }\n}\n",
            "    if c {}",
        ),
        (
            "module app\nfn run(s: Shape) {\n    match s {\n    }\n}\n",
            "    match s {}",
        ),
    ];
    for (source, expected_fragment) in cases {
        let once = format_source(source);
        assert!(
            once.contains(expected_fragment),
            "expected `{expected_fragment}` in:\n{once}"
        );
        let reparsed = parse_source(&once);
        assert!(
            !reparsed.has_errors(),
            "empty-brace output must re-parse:\n{once}\n{:#?}",
            reparsed.diagnostics
        );
        assert_eq!(format_source(&once), once, "not idempotent:\n{once}");
    }
}

/// A `store` with no members legitimately has no body and stays header-alone; the
/// empty-brace form the parser also accepts normalizes to it.
#[test]
fn a_bodyless_store_stays_header_alone() {
    let header_alone = "module app\nresource B {\n    t: string\n}\nstore ^b: B\n";
    let empty_braces = "module app\nresource B {\n    t: string\n}\nstore ^b: B {\n}\n";
    let canonical = format_source(header_alone);
    assert!(
        canonical.contains("store ^b: B\n"),
        "a member-less store stays header-alone:\n{canonical}"
    );
    assert!(
        !canonical.contains("store ^b: B {"),
        "no empty braces on a member-less store:\n{canonical}"
    );
    assert_eq!(
        format_source(empty_braces),
        canonical,
        "empty braces normalize to header-alone"
    );
}
