//! The shared `DeclParser` navigation and error surface: header-line and span
//! bounds, indented-block consumption, the declaration-keyword lookahead, and the
//! diagnostic emitters the declaration bodies build on.

use super::DeclParser;
use super::tokens::{comment_from_token, first_line_end, is_line_comment, line_end};
use crate::PARSE_SYNTAX;
use crate::ast::{Comment, CommentMarker, CommentPlacement};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, Severity, SourceSpan,
};
use crate::token::{Keyword, Token, TokenKind};

impl<'a> DeclParser<'a> {
    /// Collect the tokens of the current header line (up to the next
    /// `NEWLINE`/`INDENT`/`DEDENT`/`EOF`) and advance past the closing `NEWLINE`.
    /// A header line continues across newlines suppressed inside open delimiters,
    /// so a multi-line const value stays one header line. A trailing comment is
    /// excluded from the returned slice so the caller sees only header content.
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
                let marker = match token.kind {
                    TokenKind::DocComment => CommentMarker::Doc,
                    _ => CommentMarker::Line,
                };
                (
                    end - 1,
                    Some(comment_from_token(
                        self.source,
                        *token,
                        CommentPlacement::Trailing,
                        marker,
                    )),
                )
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
        let token = self.tokens[self.pos];
        SourceSpan {
            start_byte: token.span.start_byte,
            end_byte: first_line_end(self.source, token.span.start_byte),
            line: token.span.line,
            column: token.span.column,
        }
    }

    /// Consume a balanced `INDENT … DEDENT` run starting at the current `INDENT`,
    /// returning the exclusive index just past the matching `DEDENT`.
    pub(super) fn consume_block(&mut self) -> usize {
        self.consume_balanced_block(0)
    }

    /// Consume the rest of an indented block whose opening `INDENT` was already
    /// advanced, stopping after its matching `DEDENT`.
    pub(super) fn skip_to_block_end(&mut self) {
        self.consume_balanced_block(1);
    }

    /// Consume tokens until the `INDENT`/`DEDENT` depth returns to zero, seeded at
    /// `open_depth` (zero when the opening `INDENT` is still ahead, one when it was
    /// already advanced). Returns the exclusive index just past the matching
    /// `DEDENT`, tolerating end-of-file before the block closes.
    fn consume_balanced_block(&mut self, open_depth: usize) -> usize {
        let mut depth = open_depth;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Indent => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::Dedent => {
                    self.advance();
                    depth -= 1;
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

    /// Report one "expected a top-level declaration" per content line of a stray
    /// indented region at the top level, each at its content position. Blank and
    /// comment-only lines produce no tokens and so raise nothing.
    pub(super) fn report_stray_indented_lines(&mut self) {
        let start = self.pos;
        let end = self.consume_block();
        let mut index = start;
        let mut at_line_start = true;
        while index < end {
            let token = self.tokens[index];
            match token.kind {
                TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline => at_line_start = true,
                TokenKind::Comment | TokenKind::DocComment => at_line_start = false,
                _ => {
                    if at_line_start {
                        let line_start = token.span.start_byte - (token.span.column as usize - 1);
                        let content_start = token.span.start_byte;
                        let span = SourceSpan {
                            start_byte: content_start,
                            end_byte: first_line_end(self.source, line_start),
                            line: token.span.line,
                            column: token.span.column,
                        };
                        self.error_span(
                            span,
                            ParseDiagnosticReason::Expected(ExpectedSyntax::Declaration),
                            "expected a top-level declaration",
                        );
                    }
                    at_line_start = false;
                }
            }
            index += 1;
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

    pub(super) fn identifier_is(&self, index: usize, text: &str) -> bool {
        self.tokens.get(index).is_some_and(|token| {
            token.kind == TokenKind::Identifier && token.text(self.source) == text
        })
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

    /// Whether the token after the current one is `keyword` followed by a space,
    /// the trailing-space rule a `pub fn`/`pub enum` header applies to each word.
    fn followed_by_keyword_space(&self, keyword: Keyword) -> bool {
        self.tokens.get(self.pos + 1).is_some_and(|token| {
            token.kind == TokenKind::Keyword(keyword) && self.space_after(*token)
        })
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
            code: PARSE_SYNTAX,
            reason: DiagnosticReason::Parser(reason),
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span,
        });
    }
}
