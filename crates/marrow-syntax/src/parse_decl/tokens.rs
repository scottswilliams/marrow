//! Low-level helpers over a token slice shared across the declaration and
//! statement parsers: line and span bounds, top-level delimiter scanning, comment
//! construction, and the bridge into the expression parser.

use super::{ParseError, ParseResult};
use crate::PARSE_SYNTAX;
use crate::ast::{Comment, CommentMarker, CommentPlacement, Expression, TypeRef};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, Severity, SourceSpan,
};
use crate::parse_expr::{ExprParser, join_spans};
use crate::token::{Keyword, Token, TokenKind, is_qualified_name};

/// The end byte of the physical line containing `start`, excluding the trailing
/// `\r`/`\n`. This matches `Line::end_byte` for a declaration's first line.
pub(super) fn first_line_end(source: &str, start: usize) -> usize {
    let tail = &source[start..];
    let break_at = tail
        .find('\n')
        .map(|index| {
            if tail[..index].ends_with('\r') {
                index - 1
            } else {
                index
            }
        })
        .unwrap_or(tail.len());
    start + break_at
}
/// Strip the `;;` doc-comment marker and surrounding whitespace, matching
/// `Line::doc_comment`.
pub(super) fn doc_comment_text(text: &str) -> String {
    text.strip_prefix(";;").unwrap_or(text).trim().to_string()
}
/// The end byte of the physical line that ends just before `pos`, excluding that
/// line's trailing `\r`/`\n`. Used to bound a function body at the end of its
/// last line, the line just above the line that closed the block.
pub(super) fn line_text_end_before(source: &str, pos: usize) -> usize {
    let before = &source[..pos.min(source.len())];
    let before = before.strip_suffix('\n').unwrap_or(before);
    let before = before.strip_suffix('\r').unwrap_or(before);
    before.len()
}
/// The `::`-separated source text spanned by the `module`/`use` name tokens, if
/// it is a qualified name. The text is validated lexically (not by token kind),
/// so a keyword that is also a valid path segment — such as the `bytes` in
/// `use std::bytes` — is accepted, the same way it is mid-path in an expression.
pub(super) fn qualified_name(source: &str, tokens: &[Token]) -> Option<String> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let text = &source[first.span.start_byte..last.span.end_byte];
    is_qualified_name(text).then(|| text.to_string())
}
pub(super) fn push_parse_error(
    diagnostics: &mut Vec<Diagnostic>,
    span: SourceSpan,
    error: ParseError,
) {
    diagnostics.push(Diagnostic {
        code: PARSE_SYNTAX,
        reason: DiagnosticReason::Parser(error.reason),
        severity: Severity::Error,
        message: error.message.to_string(),
        help: None,
        span,
    });
}
/// Split tokens on top-level commas (depth 0), dropping a trailing empty group
/// from a trailing comma.
pub(super) fn split_top_level_commas(tokens: &[Token]) -> Vec<&[Token]> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            TokenKind::Comma if depth == 0 => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < tokens.len() {
        parts.push(&tokens[start..]);
    }
    parts
}
/// Index of the first top-level `=` (assignment separator). Equality is `==`, so
/// a depth-0 `=` is unambiguously the assignment in a statement; the depth-0
/// restriction still keeps named-argument colons and nested forms from splitting.
pub(super) fn find_top_level_equal(tokens: &[Token]) -> Option<usize> {
    find_top_level(tokens, TokenKind::Equal)
}
/// Index of the first token satisfying `predicate` at parenthesis/bracket depth 0.
/// The traversal tracks delimiter depth; the predicate receives each candidate
/// index and the full slice so it can peek at neighbouring tokens.
fn find_at_top_level(
    tokens: &[Token],
    predicate: impl Fn(usize, &[Token]) -> bool,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            _ if depth == 0 && predicate(index, tokens) => return Some(index),
            _ => {}
        }
    }
    None
}
/// Index of the leading `-` of a top-level `->` arrow (the `-` `>` token pair the
/// lexer emits). The arrow separates an evolve rename's two paths; restricting to
/// depth 0 keeps an arrow inside a parenthesized key from splitting the rename.
pub(super) fn find_arrow(tokens: &[Token]) -> Option<usize> {
    find_at_top_level(tokens, |index, tokens| {
        tokens[index].kind == TokenKind::Minus
            && tokens.get(index + 1).map(|token| token.kind) == Some(TokenKind::Greater)
    })
}
/// Index of the first occurrence of `kind` at parenthesis/bracket depth 0.
pub(super) fn find_top_level(tokens: &[Token], kind: TokenKind) -> Option<usize> {
    find_at_top_level(tokens, |index, tokens| tokens[index].kind == kind)
}
pub(super) fn expr_of(
    source: &str,
    tokens: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    ExprParser::new(source, tokens).parse_complete(diagnostics)
}
pub(super) fn reject_structural_type_tokens(
    tokens: &[Token],
    expected: ExpectedSyntax,
    message: &'static str,
) -> ParseResult<()> {
    if tokens
        .iter()
        .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Maybe)))
    {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(expected),
            "`maybe` is only valid before a function return type",
        ));
    }
    if tokens.iter().any(|token| token.kind == TokenKind::Equal) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(expected),
            message,
        ));
    }
    Ok(())
}
pub(super) fn type_ref_from_tokens(source: &str, tokens: &[Token]) -> TypeRef {
    let start = tokens[0].span.start_byte;
    let end = tokens[tokens.len() - 1].span.end_byte;
    let span = join_spans(tokens[0].span, tokens[tokens.len() - 1].span);
    // Type spelling is resolved downstream; syntax stores the annotation text in
    // a whitespace-free form so wrapped annotations format as one line.
    let text = source[start..end]
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    TypeRef { text, span }
}
pub(super) fn line_span(tokens: &[Token]) -> SourceSpan {
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => join_spans(first.span, last.span),
        _ => SourceSpan::default(),
    }
}
/// Index of the layout token (`NEWLINE`/`INDENT`/`DEDENT`/`EOF`) that ends the
/// line starting at `pos`, or `tokens.len()` if none follows. A header line
/// continues across newlines suppressed inside open delimiters, so this stops at
/// the first structural token rather than any newline.
pub(super) fn line_end(tokens: &[Token], pos: usize) -> usize {
    let mut index = pos;
    while index < tokens.len()
        && !matches!(
            tokens[index].kind,
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
        )
    {
        index += 1;
    }
    index
}
pub(super) fn is_line_comment(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Comment | TokenKind::DocComment)
}
/// Build a `Comment` from a line comment token, stripping the leading marker and
/// surrounding whitespace so the formatter renders a canonical `; text` line.
pub(super) fn comment_from_token(
    source: &str,
    token: Token,
    placement: CommentPlacement,
    marker: CommentMarker,
) -> Comment {
    let text = token
        .text(source)
        .trim_start_matches(';')
        .trim()
        .to_string();
    Comment {
        text,
        placement,
        marker,
        span: token.span,
    }
}
