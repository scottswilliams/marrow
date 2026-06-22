//! Low-level helpers over a token slice shared across the declaration and
//! statement parsers: line and span bounds, top-level delimiter scanning, comment
//! construction, and the bridge into the expression parser.

use std::borrow::Cow;

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
fn qualified_name_text(source: &str, tokens: &[Token]) -> Option<String> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let text = &source[first.span.start_byte..last.span.end_byte];
    is_qualified_name(text).then(|| text.to_string())
}
/// Why a `use`/`module` path failed to parse: a reserved word stands where a
/// path segment must be (with the offending token), or the tokens do not spell a
/// dotted/`::`-qualified name at all.
pub(super) enum PathNameError {
    ReservedSegment(Token),
    NotQualified,
}
pub(super) fn module_name(source: &str, tokens: &[Token]) -> Result<String, PathNameError> {
    if let Some(reserved) = reserved_segment(tokens) {
        return Err(PathNameError::ReservedSegment(*reserved));
    }
    qualified_name_text(source, tokens).ok_or(PathNameError::NotQualified)
}
pub(super) fn import_name(source: &str, tokens: &[Token]) -> Result<String, PathNameError> {
    // `std::bytes` is the one import whose final segment is a reserved word, so a
    // reserved segment elsewhere is the path error.
    if let Some(reserved) =
        reserved_segment(tokens).filter(|_| !is_std_bytes_import(source, tokens))
    {
        return Err(PathNameError::ReservedSegment(*reserved));
    }
    qualified_name_text(source, tokens).ok_or(PathNameError::NotQualified)
}
fn reserved_segment(tokens: &[Token]) -> Option<&Token> {
    tokens
        .iter()
        .step_by(2)
        .find(|token| matches!(token.kind, TokenKind::Keyword(_)))
}
fn is_std_bytes_import(source: &str, tokens: &[Token]) -> bool {
    matches!(
        tokens,
        [std, sep, bytes]
            if std.kind == TokenKind::Identifier
                && std.text(source) == "std"
                && sep.kind == TokenKind::DoubleColon
                && bytes.kind == TokenKind::Keyword(Keyword::Bytes)
    )
}
pub(super) fn push_parse_error(
    diagnostics: &mut Vec<Diagnostic>,
    fallback: SourceSpan,
    error: ParseError,
) {
    let (span, reason, message) = error.locate(fallback);
    diagnostics.push(Diagnostic {
        code: PARSE_SYNTAX,
        reason: DiagnosticReason::Parser(reason),
        severity: Severity::Error,
        message,
        help: None,
        span,
    });
}
/// Drop comment tokens from a token slice. A `;` or `;;` line inside an open
/// delimiter lexes to a `Comment`/`DocComment` token with no newline; like a
/// blank line, it does not separate or close anything, so a declaration list
/// that spans several physical lines reads it as absent. Returns the slice
/// unchanged when it holds no comments, so the common single-line list keeps its
/// borrow.
pub(super) fn strip_comment_tokens(tokens: &[Token]) -> Cow<'_, [Token]> {
    if tokens.iter().any(|token| is_line_comment(token.kind)) {
        Cow::Owned(
            tokens
                .iter()
                .copied()
                .filter(|token| !is_line_comment(token.kind))
                .collect(),
        )
    } else {
        Cow::Borrowed(tokens)
    }
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
    // A type annotation is a single type production: one head word, optionally
    // extended by `::` name segments and attached `[...]`/`(...)` groups. Any
    // depth-0 token past that end (a `in`, `@`, `where`, or a second bare word)
    // is not part of the type; reject it where it begins rather than gluing it
    // into the spelling.
    let end = type_token_len(tokens);
    if let Some(trailing) = tokens.get(end) {
        return Err(ParseError::at(
            trailing.span,
            ParseDiagnosticReason::Expected(expected),
            message,
        ));
    }
    Ok(())
}

/// The number of leading tokens that make up one complete type production: the
/// head token, then each following `::` name segment and each attached
/// `[...]`/`(...)` group at depth 0. Bracket contents are spanned whole, so
/// whitespace and nested types inside them do not end the type.
fn type_token_len(tokens: &[Token]) -> usize {
    let mut index = if tokens.is_empty() { 0 } else { 1 };
    while index < tokens.len() {
        match tokens[index].kind {
            TokenKind::DoubleColon => index += 2,
            TokenKind::LeftBracket | TokenKind::LeftParen => {
                match balanced_group_end(tokens, index) {
                    Some(close) => index = close + 1,
                    None => return tokens.len(),
                }
            }
            _ => break,
        }
    }
    index.min(tokens.len())
}

/// Index of the bracket that closes the `[`/`(` at `open`, matching nested
/// brackets of either kind. `None` when the group never closes.
fn balanced_group_end(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, token) in tokens[open..].iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
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
