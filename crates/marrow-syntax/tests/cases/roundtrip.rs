//! The lossless-parser property: the token stream tiles the source exactly.
//!
//! Every source byte is covered exactly once, in order, by either a token's own
//! span or an inter-token gap (whitespace, blank lines, and the content the
//! layout limit drops from the stream but not from the file). So walking the
//! tokens in order, emitting each gap and then each token's text, reconstructs
//! the source byte-for-byte. This is what makes trivia recoverable without a
//! separate side table: comments, doc comments, indentation, and newlines are
//! already tokens, and the remaining whitespace is exactly the gaps.
//!
//! The reconstruction slices from the source, so equality alone would be
//! satisfied by any tiling; the load-bearing guard is the span well-formedness
//! this asserts alongside it — spans in bounds, sorted by start, and
//! non-overlapping. A regression that made the interpolation lexer emit
//! overlapping or out-of-order spans (the case `tokens_in_range` warns about),
//! or that dropped a byte from a token boundary, breaks this property.

use marrow_syntax::lex_source;

use crate::common::mw_blocks;

/// Reconstruct `source` from its token stream, asserting the spans are in
/// bounds, sorted by start byte, and non-overlapping as it goes.
fn reconstruct(source: &str) -> String {
    let tokens = lex_source(source).tokens;
    let mut out = String::new();
    let mut cursor = 0usize;
    let mut prev_start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        let start = token.span.start_byte;
        let end = token.span.end_byte;
        assert!(
            end >= start && end <= source.len(),
            "token {index} {:?} span {start}..{end} out of bounds for source of len {}",
            token.kind,
            source.len()
        );
        assert!(
            start >= prev_start,
            "token {index} {:?} starts at {start}, before the previous token's start {prev_start}",
            token.kind
        );
        assert!(
            start >= cursor,
            "token {index} {:?} at {start}..{end} overlaps the byte already covered up to {cursor}",
            token.kind
        );
        prev_start = start;
        // The gap before the token is inter-token trivia the stream does not
        // tokenize (spaces, blank lines, over-limit content); slice it back in.
        out.push_str(&source[cursor..start]);
        out.push_str(&source[start..end]);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

fn assert_round_trips(label: &str, source: &str) {
    assert_eq!(
        reconstruct(source),
        source,
        "token stream did not reconstruct {label}"
    );
}

/// Every documented `.mw` example reconstructs exactly from its tokens.
#[test]
fn documented_examples_round_trip() {
    for block in mw_blocks() {
        assert_round_trips(&format!("{}#{}", block.path, block.index), &block.source);
    }
}

/// Whitespace, comments, layout, and the lexer's trickiest constructs — nested
/// and escaped-quote interpolation, durations, CRLF endings, over-indentation,
/// and content past the layout nesting limit — all reconstruct exactly.
#[test]
fn adversarial_inputs_round_trip() {
    let cases = [
        "resource Book\n    title: string   \n",
        "fn f()\n\n\n    return\n",
        "const X = $\"a{f(\\\"x\\\")}b\"\n",
        "const X = $\"a{1}b\"\n",
        "const X = $\"outer{$\"inner{1}\"}\"\n",
        "const X = 1.day\n",
        "fn f()\n    g(\n        a: 1,\n    )\n",
        "  indented\n",
        ";; docs\nresource R\n    x: int\n",
        "resource R\n    x: int ; trailing\n",
        "\t\n",
        "",
        "\n\n\n",
        "resource R\r\n    x: int\r\n",
    ];
    for (index, source) in cases.iter().enumerate() {
        assert_round_trips(&format!("adversarial #{index}"), source);
    }
}

/// A nest far past the 256-level layout limit is the one place the lexer bounds
/// the token stream by dropping over-deep content; that content still lives in
/// an inter-token gap, so the file reconstructs exactly even though the stream
/// stays bounded.
#[test]
fn over_deep_nesting_round_trips() {
    let mut source = String::from("enum E\n");
    for level in 0..400 {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str(&format!("m{level}\n"));
    }
    assert_round_trips("over-deep nest", &source);
}
