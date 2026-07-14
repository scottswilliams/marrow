//! The shared declaration-body frame: the one `INDENT … DEDENT` trivia skeleton
//! that the resource, store, enum, and evolve bodies drive their member
//! loops from. Each body advances its opening `INDENT`, then repeatedly asks for
//! the next line: the frame consumes blank lines, own-line comments, and stray
//! nested blocks, and reports a member header for the caller to parse. The two
//! axes on which the bodies genuinely differ — what an own-line doc comment does,
//! and where a stray nested block is reported — are the frame's typed inputs.

use super::tokens::{comment_from_token, is_line_comment};
use super::{DeclParser, ParseError};
use crate::ast::{Comment, CommentMarker, CommentPlacement};
use crate::token::{Token, TokenKind};

/// The classification of the next line of an indented declaration body, after the
/// shared trivia (dedent, blank lines, comments, stray nested blocks) has been
/// handled.
pub(super) enum BodyLine {
    /// The block closed on its `DEDENT` (or end of input); stop the loop.
    End,
    /// A trivia line was consumed; continue without parsing an item.
    Trivia,
    /// A member header is in place for the caller to parse.
    Item,
}

/// What a declaration body does with an own-line `;;` doc comment: resource and
/// enum members carry docs, so it accumulates to attach to the next member;
/// evolve steps carry none, so it is retained as trivia.
pub(super) enum DocComments<'a> {
    /// Accumulate into `docs` to attach to the next member; any doc comment left
    /// unattached when the block closes is reported by `flush_docs_as_comments`.
    AttachToItem(&'a mut Vec<Token>),
    /// Retain as trivia. `keep_marker` renders `;;` as a doc comment; otherwise it
    /// is folded into an ordinary line comment.
    Retain { keep_marker: bool },
}

/// Where a stray nested block inside a body is reported. A body member has no
/// nested block of its own here (a resource group or a `transform` opens its
/// block right after its header, before the frame sees the next line).
pub(super) enum StrayBlock {
    /// Consume the stray `INDENT`, then report `error` at the first content line
    /// when the block is non-empty (resource, store, enum bodies).
    AtContent(ParseError),
    /// Report `error` at the stray `INDENT` itself, unconditionally, then consume
    /// the block (evolve steps, where the step keyword owns the diagnostic).
    AtBlock(ParseError),
}

impl<'a> DeclParser<'a> {
    /// Classify and consume the next line of an indented declaration body. The
    /// caller supplies its doc-comment and stray-block policies and its own-line
    /// comment accumulator; an `Item` result leaves the member header in place.
    pub(super) fn next_body_line(
        &mut self,
        docs: DocComments<'_>,
        comments: &mut Vec<Comment>,
        stray: &StrayBlock,
    ) -> BodyLine {
        match self.peek() {
            None | Some(TokenKind::Dedent) => {
                if matches!(self.peek(), Some(TokenKind::Dedent)) {
                    self.advance();
                }
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
            Some(TokenKind::Indent) => {
                self.consume_stray_block(stray);
                BodyLine::Trivia
            }
            Some(_) => BodyLine::Item,
        }
    }

    /// Consume one own-line comment token and route it per the doc-comment policy,
    /// then its trailing `NEWLINE`. An ordinary `;` line comment is retained as
    /// trivia regardless of policy.
    fn take_body_comment(&mut self, docs: DocComments<'_>, comments: &mut Vec<Comment>) {
        if matches!(self.peek(), Some(TokenKind::DocComment)) {
            match docs {
                DocComments::AttachToItem(pending) => {
                    self.push_pending_doc(pending, comments);
                    self.consume_trailing_newline();
                    return;
                }
                DocComments::Retain { keep_marker } => {
                    let token = self.advance();
                    let marker = if keep_marker {
                        CommentMarker::Doc
                    } else {
                        CommentMarker::Line
                    };
                    comments.push(self.own_line_comment(token, marker));
                }
            }
        } else {
            let token = self.advance();
            comments.push(self.own_line_comment(token, CommentMarker::Line));
        }
        self.consume_trailing_newline();
    }

    fn own_line_comment(&self, token: Token, marker: CommentMarker) -> Comment {
        comment_from_token(self.source, token, CommentPlacement::OwnLine, marker)
    }

    fn consume_trailing_newline(&mut self) {
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    /// Consume a stray nested block opening at the current `INDENT` and report it
    /// per the stray-block policy.
    fn consume_stray_block(&mut self, stray: &StrayBlock) {
        match stray {
            StrayBlock::AtContent(error) => {
                self.advance(); // INDENT
                if self.peek().is_some_and(|kind| {
                    !matches!(
                        kind,
                        TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
                    )
                }) {
                    let span = self.content_span();
                    self.error_span(span, error.reason.clone(), error.message.clone());
                }
                self.skip_to_block_end();
            }
            StrayBlock::AtBlock(error) => {
                let span = self.content_span();
                self.error_span(span, error.reason.clone(), error.message.clone());
                self.advance(); // INDENT
                self.skip_to_block_end();
            }
        }
    }
}
