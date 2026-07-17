//! The shared declaration-body frame: the one `{ … }` trivia skeleton that the
//! resource, store, and enum bodies drive their member loops from. Each body
//! advances its opening `{`, then repeatedly asks for the next line: the frame
//! consumes blank lines, own-line comments, and stray nested blocks, and reports a
//! member header for the caller to parse. The caller supplies the doc-comment
//! accumulator every member attaches to and the diagnostic a stray nested block
//! reports.

use super::tokens::{comment_from_token, is_line_comment};
use super::{DeclParser, ParseError};
use crate::ast::{Comment, CommentMarker, CommentPlacement};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::token::{Token, TokenKind};

/// The classification of the next line of a `{ … }` declaration body, after the
/// shared trivia (closing brace, blank lines, comments, stray nested blocks) has
/// been handled.
pub(super) enum BodyLine {
    /// The block closed on its `}` (or end of input); stop the loop.
    End,
    /// A trivia line was consumed; continue without parsing an item.
    Trivia,
    /// A member header is in place for the caller to parse.
    Item,
}

impl<'a> DeclParser<'a> {
    /// Classify and consume the next line of a `{ … }` declaration body. The
    /// caller supplies the opening `{` span (to anchor an unclosed-block diagnostic),
    /// its own-line comment accumulator (`docs` collects `///` doc comments to attach
    /// to the next member), and the diagnostic to report for a stray nested block; an
    /// `Item` result leaves the member header in place.
    ///
    /// The token stream carries the lexer's `Eof` sentinel after the last content
    /// token, so end of input reaches here as `Eof` rather than `None`; both close
    /// the loop, but only a real `}` is consumed, and reaching the sentinel reports
    /// the block as unclosed.
    pub(super) fn next_body_line(
        &mut self,
        open: SourceSpan,
        docs: &mut Vec<Token>,
        comments: &mut Vec<Comment>,
        stray: &ParseError,
    ) -> BodyLine {
        match self.peek() {
            Some(TokenKind::RightBrace) => {
                self.advance();
                BodyLine::End
            }
            None | Some(TokenKind::Eof) => {
                self.report_unclosed_block(open);
                BodyLine::End
            }
            Some(TokenKind::Newline) => {
                self.advance();
                BodyLine::Trivia
            }
            Some(kind) if is_line_comment(kind) => {
                self.take_body_comment(docs, comments);
                BodyLine::Trivia
            }
            Some(TokenKind::LeftBrace) => {
                self.consume_stray_block(stray);
                BodyLine::Trivia
            }
            Some(_) => BodyLine::Item,
        }
    }

    /// Report a `{ … }` declaration body that reached end of input without its
    /// matching `}`, anchored at the opening brace.
    fn report_unclosed_block(&mut self, open: SourceSpan) {
        self.error_span(
            open,
            ParseDiagnosticReason::Expected(ExpectedSyntax::CloseBrace),
            "expected `}` to close this block",
        );
    }

    /// Consume one own-line comment token and its trailing `NEWLINE`. A `///` doc
    /// comment accumulates into `docs` to attach to the next member; an ordinary
    /// `//` line comment is retained as own-line trivia.
    fn take_body_comment(&mut self, docs: &mut Vec<Token>, comments: &mut Vec<Comment>) {
        if matches!(self.peek(), Some(TokenKind::DocComment)) {
            self.push_pending_doc(docs, comments);
        } else {
            let token = self.advance();
            let comment = comment_from_token(
                self.source,
                token,
                CommentPlacement::OwnLine,
                CommentMarker::Line,
            );
            comments.push(comment);
        }
        self.consume_trailing_newline();
    }

    fn consume_trailing_newline(&mut self) {
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    /// Consume a stray nested block opening at the current `{`, reporting `error`
    /// at the first content line when the block is non-empty. A member with a body
    /// of its own (a resource group) opens it right after its header, before the
    /// frame sees the next line, so a block reaching here is stray.
    fn consume_stray_block(&mut self, error: &ParseError) {
        self.advance(); // `{`
        self.skip_newlines();
        if self
            .peek()
            .is_some_and(|kind| !matches!(kind, TokenKind::RightBrace | TokenKind::Eof))
        {
            let span = self.content_span();
            self.error_span(span, error.reason.clone(), error.message.clone());
        }
        self.skip_to_block_end();
    }
}
