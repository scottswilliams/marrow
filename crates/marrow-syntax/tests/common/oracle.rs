//! The reusable bounded syntax oracle: the shared property checker every
//! source-bytes driver (the deterministic corpus and the seeded random-mutation
//! pass in `cases/fuzz.rs`) is an input adapter over.
//!
//! The oracle is defined purely over the crate's public surface — `lex_source`,
//! `parse_source`, `format_source`, `format_preserves_comments` — so it holds the
//! same contract an external consumer sees. Two lenses:
//!
//! - [`assert_total_invariants`] holds for **any** source bytes, valid or not:
//!   parsing never panics, is deterministic, tiles the source losslessly into its
//!   token stream, and recovers with a bounded number of diagnostics carrying
//!   well-formed spans. For an input that parses without error nodes it adds the
//!   formatter's total contract — a fixed point whose output re-parses without new
//!   errors.
//! - [`assert_formatter_faithful`] holds for a **valid program** and adds the
//!   stronger formatter contract: the canonical output preserves every comment and
//!   the whole declaration tree (span-stripped AST equality).
//!
//! A minimized counterexample from either lens becomes a deterministic fixture in
//! `cases/fuzz.rs`, then a fix.

use marrow_syntax::{
    NESTING_DEPTH_LIMIT, ParsedSource, Severity, format_preserves_comments, format_source,
    lex_source, parse_source,
};

/// Recovery is bounded to at most one diagnostic per source byte, plus one. Every
/// diagnostic follows forward progress over a distinct byte — the lexer emits one
/// finding per rejected character and no token for it, and the declaration and
/// expression parsers emit at most one finding per header line or consumed token —
/// so a well-formed front end reports linearly in the input. A regression that
/// re-introduced a cascading second diagnostic (the deleted recovery zoo) or an
/// unbounded recovery loop overruns this cap. It is a coarse blast-radius bound,
/// not a replacement for the exact "exactly one diagnostic" recovery tests.
fn diagnostic_cap(source: &str) -> usize {
    source.len() + 1
}

/// A source `depth` levels deep in a construct trips the nesting limit rather than
/// overflowing the native stack, so the token stream, AST, and every later walk
/// stay bounded. The oracle asserts the cap is reachable, not any particular value.
pub const OVER_DEEP: usize = NESTING_DEPTH_LIMIT + 50;

/// The invariants that hold for arbitrary source bytes. Calling this at all is the
/// no-panic and termination property; the assertions are determinism, lossless
/// token tiling with well-formed spans, the recovery bound, and — for a clean
/// parse — the formatter's total (idempotent, re-parseable) contract.
pub fn assert_total_invariants(source: &str) {
    // Parsing is a pure function of the source: a second parse yields the identical
    // tree and diagnostics. This also exercises the no-panic property twice.
    let first = parse_source(source);
    let second = parse_source(source);
    assert_eq!(
        first.file, second.file,
        "parse is not deterministic for {source:?}"
    );
    assert_eq!(
        first.diagnostics, second.diagnostics,
        "diagnostics are not deterministic for {source:?}"
    );

    assert_token_tiling(source);

    let cap = diagnostic_cap(source);
    assert!(
        first.diagnostics.len() <= cap,
        "recovery emitted {} diagnostics, past the bound of {cap}, for {source:?}",
        first.diagnostics.len()
    );

    for diagnostic in &first.diagnostics {
        let span = diagnostic.span;
        assert!(
            span.start_byte <= span.end_byte && span.end_byte <= source.len(),
            "diagnostic span {}..{} out of bounds for source of len {} in {source:?}",
            span.start_byte,
            span.end_byte,
            source.len()
        );
        assert!(
            span.line >= 1 && span.column >= 1,
            "diagnostic anchored at line {} column {} (positions are 1-based) in {source:?}",
            span.line,
            span.column
        );
    }

    assert_formatter_total(&first, source);
}

/// The formatter's total contract that holds for any input that parses without error
/// nodes, comment-bearing or not: a single pass is already a fixed point, and its
/// output is itself valid Marrow. A malformed parse carries error nodes whose
/// rendering is not a contract, so the formatter is only exercised over a clean parse.
///
/// Idempotence is asserted unconditionally over comments (MSY01): a comment trailing a
/// body-bearing header attaches to one deterministic owner — the block — so every
/// admitted spelling formats to one fixed point. A regression that re-introduced a
/// byte-span-attributed comment with no stable home, or the earlier blank-line and
/// empty-body non-idempotencies, is caught here over arbitrary bytes.
fn assert_formatter_total(parsed: &ParsedSource, source: &str) {
    if parsed.has_errors() {
        return;
    }
    let once = format_source(source);
    let twice = format_source(&once);
    assert_eq!(
        once, twice,
        "format is not idempotent for a clean parse of {source:?}"
    );
    let reparsed = parse_source(&once);
    assert!(
        !reparsed.has_errors(),
        "formatted output must re-parse without errors:\n{once}\n{:#?}",
        reparsed.diagnostics
    );
}

/// The stronger formatter contract for a valid program: the canonical output
/// preserves every comment (marker and normalized text compared directly;
/// placement guarded by the structural fingerprint plus idempotence) and the
/// whole declaration tree. Structure is compared as span-stripped AST equality, since formatting
/// necessarily shifts byte positions. The caller guarantees `source` parses
/// cleanly; a clean parse is asserted here so a silent regression to a malformed
/// corpus entry cannot make this vacuous.
pub fn assert_formatter_faithful(source: &str) {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "a faithful-formatter subject must parse cleanly:\n{source}\n{:#?}",
        parsed.diagnostics
    );
    let once = format_source(source);
    assert!(
        format_preserves_comments(source, &once),
        "canonical formatting dropped or moved a comment for:\n{source}\n--- formatted ---\n{once}"
    );
    assert_eq!(
        structural_fingerprint(source),
        structural_fingerprint(&once),
        "formatting changed the declaration tree for:\n{source}\n--- formatted ---\n{once}"
    );
}

/// Reconstruct `source` from its token stream, asserting as it goes that the spans
/// are in bounds, sorted by start byte, and non-overlapping. Every source byte is
/// covered exactly once by a token span or an inter-token gap (whitespace, blank
/// lines, and the content the layout limit drops from the stream), so replaying the
/// gaps and tokens in order must yield the source byte-for-byte.
fn assert_token_tiling(source: &str) {
    let tokens = lex_source(source).tokens;
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0usize;
    let mut prev_start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        let start = token.span.start_byte;
        let end = token.span.end_byte;
        assert!(
            end >= start && end <= source.len(),
            "token {index} {:?} span {start}..{end} out of bounds for len {}",
            token.kind,
            source.len()
        );
        assert!(
            start >= prev_start,
            "token {index} {:?} starts at {start}, before previous start {prev_start}",
            token.kind
        );
        assert!(
            start >= cursor,
            "token {index} {:?} at {start}..{end} overlaps covered bytes up to {cursor}",
            token.kind
        );
        prev_start = start;
        out.push_str(&source[cursor..start]);
        out.push_str(&source[start..end]);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    assert_eq!(out, source, "token stream did not reconstruct the source");
}

/// A span-independent structural fingerprint of a parsed file: its `Debug`
/// rendering with every `SourceSpan { ... }` region removed. Two files compare
/// equal exactly when their declarations, statements, nesting, and retained
/// comments match, ignoring the byte positions formatting shifts.
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

/// Whether any diagnostic is an error (not a warning): used by the drivers to
/// prove the corpus actually exercised the recovery path, so the no-panic and
/// bound assertions over malformed input are not vacuous.
pub fn has_error_diagnostic(source: &str) -> bool {
    parse_source(source)
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}
