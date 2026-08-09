//! Production-path coverage for the bounded syntax diagnostic substrate: one
//! live collector per entry point, typed Count/OwnedBytes ceilings with exact
//! edges, Count precedence and Bytes-to-Count strengthening, destructive
//! discard, stable lexer-first tie order, discarding-probe isolation,
//! finalize-before-submit help (A8), the nonempty wrapper (A6), and the
//! formatting refusals over the bounded result.

use crate::common::{lexer_reason, parse_reason};
use marrow_syntax::{
    Diagnostic, ExpectedSyntax, FormatRefusal, LexerDiagnosticReason, PARSE_SYNTAX,
    ParseDiagnosticReason, SYNTAX_DIAGNOSTIC_COUNT_LIMIT, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT,
    Severity, SyntaxDiagnosticLimit, SyntaxDiagnostics, UnsupportedSyntax, check_format,
    format_preserves_comments, format_source, lex_source, parse_expression, parse_source,
};

const COUNT: usize = SYNTAX_DIAGNOSTIC_COUNT_LIMIT;
const BYTES: usize = SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT;

/// Borrow the complete payload the test expects, panicking with the limit
/// otherwise.
fn complete(diagnostics: &SyntaxDiagnostics) -> &[Diagnostic] {
    diagnostics
        .as_complete()
        .unwrap_or_else(|limit| panic!("expected complete diagnostics, hit {limit:?}"))
        .as_slice()
}

/// `count` top-level lines that each produce exactly one small parser
/// diagnostic (the unknown-declaration header error).
fn error_lines(count: usize) -> String {
    "wat\n".repeat(count)
}

/// One line producing exactly one diagnostic whose message embeds a
/// `filler_len`-byte identifier: a stray token after a complete const type
/// annotation, reported as "unexpected `<filler>` after the type".
fn oversized_type_line(filler_len: usize) -> String {
    format!("const C: int {} = 1\n", "a".repeat(filler_len))
}

/// The per-row owned-byte overhead of [`oversized_type_line`] beyond its
/// filler, derived from a probe parse rather than hardcoded prose lengths.
fn probe_row_overhead() -> usize {
    let parsed = parse_source(&oversized_type_line(64));
    let summary = parsed.diagnostics.summary();
    assert_eq!(
        summary.count(),
        1,
        "the probe line must yield one diagnostic"
    );
    summary.owned_bytes() - 64
}

/// A source whose parse retains exactly `total_owned_bytes` of diagnostic
/// payload across a handful of oversized rows, staying far below the count
/// ceiling so only the byte ceiling is in play.
fn byte_dense_source(total_owned_bytes: usize) -> String {
    let overhead = probe_row_overhead();
    let filler = 1 << 17;
    let row_bytes = filler + overhead;
    let mut full_rows = total_owned_bytes / row_bytes;
    let mut last = total_owned_bytes - full_rows * row_bytes;
    if last <= overhead {
        full_rows -= 1;
        last = total_owned_bytes - full_rows * row_bytes;
    }
    let mut source = String::new();
    for _ in 0..full_rows {
        source.push_str(&oversized_type_line(filler));
    }
    source.push_str(&oversized_type_line(last - overhead));
    source
}

#[test]
fn owned_bytes_charge_is_the_finalized_message_and_help() {
    let parsed = parse_source(&oversized_type_line(64));
    let rows = complete(&parsed.diagnostics);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert!(rows[0].help.is_none());
    assert_eq!(
        parsed.diagnostics.summary().owned_bytes(),
        rows[0].message.len(),
        "a help-less row charges exactly its message bytes"
    );
}

/// A8: the loop-label recovery finalizes its help before submission, and the
/// byte charge covers message plus help exactly.
#[test]
fn help_is_finalized_before_submission_and_charged() {
    let source = "fn f() {\n    outer: while true {\n        break\n    }\n}\n";
    let parsed = parse_source(source);
    let rows = complete(&parsed.diagnostics);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(
        rows[0].reason,
        parse_reason(ParseDiagnosticReason::Unsupported(
            UnsupportedSyntax::LoopLabels
        ))
    );
    let help = rows[0]
        .help
        .as_deref()
        .expect("the loop-label diagnostic carries its remedy as help");
    assert_eq!(
        help,
        "extract a function and use return to leave nested loops"
    );
    assert_eq!(
        parsed.diagnostics.summary().owned_bytes(),
        rows[0].message.len() + help.len()
    );
}

#[test]
fn count_at_the_limit_stays_complete() {
    let parsed = parse_source(&error_lines(COUNT));
    assert_eq!(parsed.diagnostics.summary().count(), COUNT);
    assert_eq!(complete(&parsed.diagnostics).len(), COUNT);
    assert!(parsed.has_errors());
}

#[test]
fn count_one_past_the_limit_discards_destructively() {
    let parsed = parse_source(&error_lines(COUNT + 1));
    let limit = parsed
        .diagnostics
        .as_complete()
        .expect_err("the payload must be discarded past the count ceiling");
    assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });
    assert_eq!(parsed.diagnostics.summary().count(), COUNT + 1);
    assert!(parsed.has_errors());
    let limit = parsed
        .diagnostics
        .into_complete()
        .expect_err("consuming access agrees with borrowing access");
    assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });
}

#[test]
fn limited_totals_saturate_at_limit_plus_one() {
    let parsed = parse_source(&error_lines(COUNT + 900));
    assert_eq!(parsed.diagnostics.summary().count(), COUNT + 1);
}

#[test]
fn owned_bytes_at_the_limit_stay_complete() {
    let parsed = parse_source(&byte_dense_source(BYTES));
    let summary = parsed.diagnostics.summary();
    assert_eq!(summary.owned_bytes(), BYTES);
    assert!(summary.count() < COUNT);
    assert!(parsed.diagnostics.as_complete().is_ok());
}

#[test]
fn owned_bytes_one_past_the_limit_discard_destructively() {
    let parsed = parse_source(&byte_dense_source(BYTES + 1));
    let limit = parsed
        .diagnostics
        .as_complete()
        .expect_err("the payload must be discarded past the byte ceiling");
    assert_eq!(limit, SyntaxDiagnosticLimit::OwnedBytes { limit: BYTES });
    assert_eq!(parsed.diagnostics.summary().owned_bytes(), BYTES + 1);
}

#[test]
fn a_single_oversized_row_saturates_owned_bytes_at_limit_plus_one() {
    let parsed = parse_source(&oversized_type_line(3 * BYTES));
    let limit = parsed
        .diagnostics
        .as_complete()
        .expect_err("one oversized row crosses the byte ceiling");
    assert_eq!(limit, SyntaxDiagnosticLimit::OwnedBytes { limit: BYTES });
    let summary = parsed.diagnostics.summary();
    assert_eq!(summary.owned_bytes(), BYTES + 1);
    assert_eq!(summary.count(), 1);
}

#[test]
fn count_wins_a_simultaneous_crossing() {
    let mut source = error_lines(COUNT);
    source.push_str(&oversized_type_line(2 * BYTES));
    let parsed = parse_source(&source);
    let limit = parsed
        .diagnostics
        .as_complete()
        .expect_err("the crossing push must discard the payload");
    assert_eq!(
        limit,
        SyntaxDiagnosticLimit::Count { limit: COUNT },
        "Count is selected when one push crosses both ceilings"
    );
    let summary = parsed.diagnostics.summary();
    assert_eq!(summary.count(), COUNT + 1);
    assert_eq!(summary.owned_bytes(), BYTES + 1);
}

#[test]
fn a_bytes_limit_strengthens_to_count_and_payload_never_returns() {
    let mut source = String::new();
    source.push_str(&oversized_type_line(600 * 1024));
    source.push_str(&oversized_type_line(600 * 1024));
    source.push_str(&error_lines(COUNT + 100));
    let parsed = parse_source(&source);
    let limit = parsed
        .diagnostics
        .as_complete()
        .expect_err("the payload destroyed at the byte crossing never re-materializes");
    assert_eq!(
        limit,
        SyntaxDiagnosticLimit::Count { limit: COUNT },
        "a Bytes limit strengthens to Count once the count ceiling is also crossed"
    );
    let summary = parsed.diagnostics.summary();
    assert_eq!(summary.count(), COUNT + 1);
    assert_eq!(summary.owned_bytes(), BYTES + 1);
}

#[test]
fn one_owner_orders_rows_by_position_with_lexer_first_ties() {
    // A tab at the line start and the unparseable line report at the same
    // (line, start byte); the lexer row sorts first on the tie.
    let parsed = parse_source("\twat\n");
    let rows = complete(&parsed.diagnostics);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    assert_eq!(
        rows[0].reason,
        lexer_reason(LexerDiagnosticReason::TabIndentation)
    );
    assert_eq!(
        rows[1].reason,
        parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Declaration))
    );
    assert_eq!(rows[0].span.line, rows[1].span.line);
    assert_eq!(rows[0].span.start_byte, rows[1].span.start_byte);

    // Position still dominates producer: an earlier parser row sorts before a
    // later lexer row on the same line.
    let parsed = parse_source("wat ~\n");
    let rows = complete(&parsed.diagnostics);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    assert_eq!(
        rows[0].reason,
        parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Declaration))
    );
    assert_eq!(
        rows[1].reason,
        lexer_reason(LexerDiagnosticReason::ReservedTilde)
    );
    assert!(rows[0].span.start_byte < rows[1].span.start_byte);
}

#[test]
fn parse_expression_reports_lexer_and_parser_findings_through_one_owner() {
    let (expression, diagnostics) = parse_expression("1 + ~");
    let rows = complete(&diagnostics);
    assert!(
        rows.iter()
            .any(|row| row.reason == lexer_reason(LexerDiagnosticReason::ReservedTilde)),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.reason
            == parse_reason(ParseDiagnosticReason::Expected(ExpectedSyntax::Expression))),
        "{rows:#?}"
    );
    assert!(expression.is_none());
}

#[test]
fn parse_expression_is_bounded() {
    let source = format!("$\"{}\"", "{}".repeat(COUNT + 200));
    let (_, diagnostics) = parse_expression(&source);
    let limit = diagnostics
        .as_complete()
        .expect_err("an error-dense expression is bounded");
    assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });
    assert_eq!(diagnostics.summary().count(), COUNT + 1);
}

#[test]
fn lex_source_is_bounded() {
    let lexed = lex_source(&"~".repeat(COUNT + 200));
    let limit = lexed
        .diagnostics
        .as_complete()
        .expect_err("an error-dense lex is bounded");
    assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });

    let lexed = lex_source("module app\n");
    assert!(complete(&lexed.diagnostics).is_empty());
}

/// The for-header probe parses its operands silently; a failed probe leaves no
/// trace in the count or the byte total — only the single header diagnostic.
#[test]
fn a_discarded_probe_leaves_no_trace_in_count_or_bytes() {
    let source = "fn f() {\n    for x in {\n        x\n    }\n}\n";
    let parsed = parse_source(source);
    let rows = complete(&parsed.diagnostics);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert!(
        rows[0]
            .message
            .contains("expected `for <binding> in <iterable>`"),
        "{rows:#?}"
    );
    assert!(rows[0].help.is_none());
    assert_eq!(
        parsed.diagnostics.summary().owned_bytes(),
        rows[0].message.len()
    );
}

#[test]
fn payload_rows_are_error_only_and_summaries_gate_has_errors() {
    let clean = parse_source("module app\n");
    assert!(!clean.has_errors());
    assert_eq!(clean.diagnostics.summary().count(), 0);
    assert_eq!(clean.diagnostics.summary().owned_bytes(), 0);
    assert!(complete(&clean.diagnostics).is_empty());

    let dirty = parse_source("wat\n~\n");
    assert!(dirty.has_errors());
    let rows = complete(&dirty.diagnostics);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row.severity == Severity::Error));
    assert_eq!(dirty.diagnostics.summary().count(), rows.len());

    let limited = parse_source(&error_lines(COUNT + 1));
    assert!(limited.has_errors(), "a Limited result still has errors");
}

#[test]
fn summaries_are_copyable_facts() {
    let parsed = parse_source("wat\n");
    let summary = parsed.diagnostics.summary();
    let copy = summary;
    assert_eq!(summary.count(), copy.count());
    assert_eq!(summary.owned_bytes(), copy.owned_bytes());
}

/// A6: only `into_non_empty` constructs the nonempty wrapper; an empty payload
/// yields `None`, and the wrapper exposes the same rows through `as_slice`.
#[test]
fn nonempty_wrapper_construction_and_consumption() {
    let clean = parse_source("module app\n")
        .diagnostics
        .into_complete()
        .expect("a clean parse stays complete");
    assert!(clean.into_non_empty().is_none());

    let dirty = parse_source("wat\n")
        .diagnostics
        .into_complete()
        .expect("one error row stays complete");
    let expected_len = dirty.as_slice().len();
    assert_eq!(expected_len, 1);
    let nonempty = dirty
        .into_non_empty()
        .expect("an error payload is nonempty");
    assert_eq!(nonempty.as_slice().len(), expected_len);
    assert_eq!(nonempty.as_slice()[0].code, PARSE_SYNTAX);
}

/// `into_boxed_slice` consumes the complete payload into the same rows
/// `as_slice` exposes, in the same order.
#[test]
fn into_boxed_slice_yields_the_borrowed_rows() {
    let complete = parse_source("wat\n@\n")
        .diagnostics
        .into_complete()
        .expect("two error rows stay complete");
    let borrowed: Vec<_> = complete.as_slice().to_vec();
    assert_eq!(borrowed.len(), 2);
    let owned = complete.into_boxed_slice();
    assert_eq!(owned.as_ref(), borrowed.as_slice());
}

#[test]
fn check_format_names_its_refusals_or_formats() {
    assert_eq!(
        check_format("module app\n").expect("clean source formats"),
        "module app\n"
    );

    match check_format("wat\n").expect_err("an invalid parse refuses to format") {
        FormatRefusal::ParseInvalid(diagnostics) => {
            assert_eq!(diagnostics.as_slice().len(), 1);
            assert_eq!(diagnostics.as_slice()[0].code, PARSE_SYNTAX);
        }
        other => panic!("expected ParseInvalid, got {other:?}"),
    }

    match check_format(&error_lines(COUNT + 1)).expect_err("a limited parse refuses to format") {
        FormatRefusal::DiagnosticLimit(limit) => {
            assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });
        }
        other => panic!("expected DiagnosticLimit, got {other:?}"),
    }
}

#[test]
fn format_functions_refuse_only_a_diagnostic_limit() {
    assert!(
        format_source("wat\n").is_ok(),
        "a complete parse formats best-effort"
    );
    let limit = format_source(&error_lines(COUNT + 1)).expect_err("a limited parse cannot format");
    assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });

    assert_eq!(
        format_preserves_comments("module app\n", "module app\n"),
        Ok(true)
    );
    let limit = format_preserves_comments(&error_lines(COUNT + 1), "module app\n")
        .expect_err("a limited parse cannot be compared");
    assert_eq!(limit, SyntaxDiagnosticLimit::Count { limit: COUNT });
}
