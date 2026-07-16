//! The shared `DeclParser` navigation and error surface: header-line and span
//! bounds, `{ … }` block consumption, the declaration-keyword lookahead, and the
//! diagnostic emitters the declaration bodies build on.

use super::DeclParser;
use super::ParseError;
use super::tokens::{comment_from_token, first_line_end, is_line_comment, line_end};
use crate::ast::{Comment, CommentMarker, CommentPlacement};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, Severity, SourceSpan,
};
use crate::token::{Keyword, Token, TokenKind};

impl<'a> DeclParser<'a> {
    /// Collect the tokens of the current header line (up to the next
    /// `NEWLINE`/`{`/`}`/`EOF`) and advance past a closing `NEWLINE`. A body-bearing
    /// header ends at its opening `{`, which is left in place for the caller to
    /// open the block; a bodyless declaration ends at its `NEWLINE`, which is
    /// consumed. A header line continues across newlines suppressed inside open
    /// delimiters, so a multi-line const value stays one header line. A trailing
    /// comment is excluded from the returned slice so the caller sees only header
    /// content.
    pub(super) fn take_header_line(&mut self) -> &'a [Token] {
        let (line, _) = self.take_header_line_with_trailing_comment();
        line
    }

    pub(super) fn take_header_line_with_trailing_comment(
        &mut self,
    ) -> (&'a [Token], Option<Comment>) {
        let end = self.header_end();
        let (content_end, trailing_comment) = match self.tokens[self.pos..end].last() {
            Some(token) if is_line_comment(token.kind) => {
                (end - 1, self.comment_from_header(*token))
            }
            _ => (end, None),
        };
        let line = &self.tokens[self.pos..content_end];
        self.pos = end;
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
        (line, trailing_comment)
    }

    pub(super) fn peek_header_trailing_comment(&self) -> Option<Comment> {
        let end = self.header_end();
        self.tokens
            .get(self.pos..end)
            .and_then(|tokens| tokens.last())
            .and_then(|token| self.comment_from_header(*token))
    }

    fn comment_from_header(&self, token: Token) -> Option<Comment> {
        if !is_line_comment(token.kind) {
            return None;
        }
        let marker = match token.kind {
            TokenKind::DocComment => CommentMarker::Doc,
            _ => CommentMarker::Line,
        };
        Some(comment_from_token(
            self.source,
            token,
            CommentPlacement::Trailing,
            marker,
        ))
    }

    pub(super) fn header_end(&self) -> usize {
        line_end(self.tokens, self.pos)
    }

    /// The span of the current declaration's first physical line at column 1.
    /// The line starts before any indentation, which a token's `column` recovers
    /// as the byte offset from the line start. This is the span stored on
    /// declaration and resource-member nodes.
    pub(super) fn header_span(&self) -> SourceSpan {
        let token = self.tokens[self.pos];
        let start_byte = token.span.start_byte - (token.span.column as usize - 1);
        SourceSpan {
            start_byte,
            end_byte: first_line_end(self.source, start_byte),
            line: token.span.line,
            column: 1,
        }
    }

    /// The span of the current line's content, starting after its indentation.
    /// Declaration and member error diagnostics point here, at the first
    /// non-space column.
    pub(super) fn content_span(&self) -> SourceSpan {
        self.content_span_of(self.tokens[self.pos])
    }

    /// The span from `token`'s position to the end of its physical line. `token`
    /// is the first content token of the line, so the span starts at the first
    /// non-space column.
    pub(super) fn content_span_of(&self, token: Token) -> SourceSpan {
        SourceSpan {
            start_byte: token.span.start_byte,
            end_byte: first_line_end(self.source, token.span.start_byte),
            line: token.span.line,
            column: token.span.column,
        }
    }

    /// Consume a balanced `{ … }` run starting at the current `{`, returning the
    /// exclusive index just past the matching `}`.
    pub(super) fn consume_block(&mut self) -> usize {
        self.consume_balanced_block(0)
    }

    /// Consume the rest of a `{ … }` block whose opening `{` was already advanced,
    /// stopping after its matching `}`.
    pub(super) fn skip_to_block_end(&mut self) {
        self.consume_balanced_block(1);
    }

    /// Consume tokens until the `{`/`}` depth returns to zero, seeded at
    /// `open_depth` (zero when the opening `{` is still ahead, one when it was
    /// already advanced). Returns the exclusive index just past the matching `}`,
    /// tolerating end-of-file before the block closes. `}` is the hard recovery
    /// sync anchor.
    fn consume_balanced_block(&mut self, open_depth: usize) -> usize {
        let mut depth = open_depth;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::LeftBrace => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::RightBrace => {
                    self.advance();
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {
                    self.advance();
                }
            }
        }
        self.pos
    }

    /// Report a stray `{ … }` block at the top level, where a declaration was
    /// expected, and consume it so the following declarations still parse.
    pub(super) fn report_stray_indented_lines(&mut self) {
        let span = self.content_span_of(self.tokens[self.pos]);
        self.error_span(
            span,
            ParseDiagnosticReason::Expected(ExpectedSyntax::Declaration),
            "expected a top-level declaration",
        );
        self.advance(); // `{`
        self.skip_to_block_end();
    }

    /// Whether the cursor is at a block-opening `{`.
    pub(super) fn at_block_open(&self) -> bool {
        matches!(self.peek(), Some(TokenKind::LeftBrace))
    }

    /// Advance past a block-opening `{` and the `NEWLINE`s that follow the header
    /// line, leaving the cursor at the first body line.
    pub(super) fn open_brace_block(&mut self) {
        self.advance(); // `{`
        self.skip_newlines();
    }

    pub(super) fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    pub(super) fn peek(&self) -> Option<TokenKind> {
        self.tokens.get(self.pos).map(|token| token.kind)
    }

    pub(super) fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos];
        self.pos += 1;
        token
    }

    /// Whether the source byte immediately after `token` is a space. A keyword
    /// introduces a declaration only when a space follows it, so `module x` is a
    /// module declaration but `module::x` is a name path.
    fn space_after(&self, token: Token) -> bool {
        self.source.as_bytes().get(token.span.end_byte) == Some(&b' ')
    }

    pub(super) fn keyword_introduces_decl(&self) -> bool {
        self.space_after(self.tokens[self.pos])
    }

    /// Whether the current line is a function header: `fn `, `pub fn `,
    /// `internal fn `, or `private fn ` (the visibility words being plain
    /// identifiers). The trailing-space rule applies to each word.
    pub(super) fn starts_function_header(&self) -> bool {
        let lead = self.tokens[self.pos];
        match lead.kind {
            TokenKind::Keyword(Keyword::Fn) => self.space_after(lead),
            TokenKind::Keyword(Keyword::Pub) => {
                self.space_after(lead) && self.followed_by_keyword_space(Keyword::Fn)
            }
            TokenKind::Identifier
                if lead.text(self.source) == "internal" || lead.text(self.source) == "private" =>
            {
                self.space_after(lead) && self.followed_by_keyword_space(Keyword::Fn)
            }
            _ => false,
        }
    }

    /// Whether the current line is an enum header: `enum ` or `pub enum `. The
    /// trailing-space rule applies to each word, matching `pub fn`.
    pub(super) fn starts_enum_header(&self) -> bool {
        let lead = self.tokens[self.pos];
        match lead.kind {
            TokenKind::Keyword(Keyword::Enum) => self.space_after(lead),
            TokenKind::Keyword(Keyword::Pub) => {
                self.space_after(lead) && self.followed_by_keyword_space(Keyword::Enum)
            }
            _ => false,
        }
    }

    /// Whether the token after the current one is `keyword` immediately followed
    /// by a space.
    fn followed_by_keyword_space(&self, keyword: Keyword) -> bool {
        self.tokens.get(self.pos + 1).is_some_and(|token| {
            token.kind == TokenKind::Keyword(keyword) && self.space_after(*token)
        })
    }

    /// Whether the current line is `pub resource `/`pub store ` — a `pub` applied
    /// to a declaration kind that is not visibility-gated. The trailing-space rule
    /// applies to each word, so `pub` here introduces the (rejected) declaration
    /// rather than a name path.
    pub(super) fn pub_precedes_ungated_decl(&self) -> bool {
        let lead = self.tokens[self.pos];
        lead.kind == TokenKind::Keyword(Keyword::Pub)
            && self.space_after(lead)
            && (self.followed_by_keyword_space(Keyword::Resource)
                || self.followed_by_keyword_space(Keyword::Store))
    }

    /// Report an error spanning the current header line.
    pub(super) fn error_header(
        &mut self,
        reason: ParseDiagnosticReason,
        message: impl Into<String>,
    ) {
        let span = self.header_span();
        self.take_header_line();
        self.error_span(span, reason, message);
    }

    pub(super) fn error_span(
        &mut self,
        span: SourceSpan,
        reason: ParseDiagnosticReason,
        message: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            code: reason.code(),
            reason: DiagnosticReason::Parser(reason),
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span,
        });
    }

    /// Report a parse error at its own pinned span, or at `fallback` (the header
    /// line) when it carries none.
    pub(super) fn report(&mut self, fallback: SourceSpan, error: ParseError) {
        let (span, reason, message) = error.locate(fallback);
        self.error_span(span, reason, message);
    }
}
