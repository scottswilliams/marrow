//! The `evolve` declaration body: the indented run of evolution steps
//! (`rename`, `default`, `retire`, `transform`) and their target-path and value
//! expressions.

use super::body::{BodyLine, DocComments, StrayBlock};
use super::tokens::{find_arrow, find_top_level_equal};
use super::{DeclParser, ParseError};
use crate::ast::{Block, Comment, EvolveDecl, EvolveStep, Expression};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::parse_expr::{ExprParser, ParseComplete};
use crate::token::{Token, TokenKind};

impl<'a> DeclParser<'a> {
    /// Parse an `evolve` block: the bare keyword header, then an indented run of
    /// evolution steps. Each step is dispatched on its contextual lead word
    /// (`rename`/`default`/`retire`/`transform`); a `transform` carries a nested
    /// statement block, which the statement parser frames.
    pub(super) fn parse_evolve(&mut self) -> EvolveDecl {
        let span = self.header_span();
        self.take_header_line(); // `evolve`
        let (steps, comments) = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_evolve_steps()
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveBody),
                "expected an indented evolve body",
            );
            (Vec::new(), Vec::new())
        };
        EvolveDecl {
            steps,
            comments,
            span,
        }
    }

    /// Parse the `INDENT … DEDENT` block of evolution steps. A `transform` step
    /// owns the indented statement block that follows its header; the other steps
    /// are single header lines.
    fn parse_evolve_steps(&mut self) -> (Vec<EvolveStep>, Vec<Comment>) {
        let mut steps = Vec::new();
        let mut comments = Vec::new();
        self.advance(); // INDENT
        // Only a transform owns an indented body, which it consumes right after
        // parsing its header; any other indented block sits under a rename,
        // default, or retire step, where the step keyword owns the diagnostic.
        // Evolve steps carry no docs, so a `;;` line is retained as a comment.
        let stray = StrayBlock::AtBlock(ParseError::new(
            ParseDiagnosticReason::UnexpectedIndentation,
            "unexpected indented block under an evolve step; only `transform` has a body",
        ));
        while self.peek().is_some() {
            match self.next_body_line(
                DocComments::Retain { keep_marker: false },
                &mut comments,
                &stray,
            ) {
                BodyLine::End => break,
                BodyLine::Trivia => continue,
                BodyLine::Item => {
                    if let Some(step) = self.parse_evolve_step(&mut comments) {
                        steps.push(step);
                    }
                }
            }
        }
        (steps, comments)
    }

    /// Parse one evolution step from its header line. The lead word selects the
    /// step kind; an unknown lead is reported and its line dropped so the
    /// following steps still parse.
    fn parse_evolve_step(&mut self, comments: &mut Vec<Comment>) -> Option<EvolveStep> {
        let span = self.header_span();
        let err = self.content_span();
        let lead = self.tokens[self.pos];
        let lead_word = (lead.kind == TokenKind::Identifier).then(|| lead.text(self.source));
        match lead_word {
            Some("rename") => self.parse_evolve_rename(span, comments),
            Some("default") => self.parse_evolve_default(span, comments),
            Some("retire") => self.parse_evolve_retire(span, comments),
            Some("transform") => self.parse_evolve_transform(span, comments),
            _ => {
                self.take_header_line();
                self.error_span(
                    err,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveStep),
                    "expected an evolve step: `rename`, `default`, `retire`, or `transform`",
                );
                None
            }
        }
    }

    /// Parse `rename <from> -> <to>`. The arrow is the `-` `>` token pair the lexer
    /// emits for `->`.
    fn parse_evolve_rename(
        &mut self,
        span: SourceSpan,
        comments: &mut Vec<Comment>,
    ) -> Option<EvolveStep> {
        let err = self.content_span();
        let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
        comments.extend(trailing_comment);
        let body = &header[1..];
        let Some(arrow) = find_arrow(body) else {
            self.error_span(
                err,
                ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveStep),
                "expected `rename <from> -> <to>`",
            );
            return None;
        };
        let from = self.evolve_path(&body[..arrow], err)?;
        let to = self.evolve_path(&body[arrow + 2..], err)?;
        Some(EvolveStep::Rename { from, to, span })
    }

    /// Parse `default <target> = <expr>`.
    fn parse_evolve_default(
        &mut self,
        span: SourceSpan,
        comments: &mut Vec<Comment>,
    ) -> Option<EvolveStep> {
        let err = self.content_span();
        let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
        comments.extend(trailing_comment);
        let body = &header[1..];
        let Some(equal) = find_top_level_equal(body) else {
            self.error_span(
                err,
                ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveStep),
                "expected `default <target> = <value>`",
            );
            return None;
        };
        let target = self.evolve_path(&body[..equal], err)?;
        let value = self.evolve_value(&body[equal + 1..], err)?;
        Some(EvolveStep::Default {
            target,
            value,
            span,
        })
    }

    /// Parse `retire <target>`.
    fn parse_evolve_retire(
        &mut self,
        span: SourceSpan,
        comments: &mut Vec<Comment>,
    ) -> Option<EvolveStep> {
        let err = self.content_span();
        let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
        comments.extend(trailing_comment);
        let target = self.evolve_path(&header[1..], err)?;
        Some(EvolveStep::Retire { target, span })
    }

    /// Parse `transform <target>` followed by an indented statement block.
    fn parse_evolve_transform(
        &mut self,
        span: SourceSpan,
        comments: &mut Vec<Comment>,
    ) -> Option<EvolveStep> {
        let err = self.content_span();
        let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
        comments.extend(trailing_comment);
        let target = self.evolve_path(&header[1..], err);
        let body = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_function_body()
        } else {
            self.error_span(
                err,
                ParseDiagnosticReason::Expected(ExpectedSyntax::TransformBody),
                "expected an indented transform body",
            );
            Block::default()
        };
        let target = target?;
        Some(EvolveStep::Transform { target, body, span })
    }

    /// Parse an evolve target path expression from the tokens of one step segment,
    /// reporting a malformed path against `err`.
    fn evolve_path(&mut self, tokens: &[Token], err: SourceSpan) -> Option<Expression> {
        if tokens.is_empty() {
            self.error_span(
                err,
                ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveTargetPath),
                "expected an evolve target path",
            );
            return None;
        }
        self.parse_expr_with_fallback(
            tokens,
            err,
            ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveTargetPath),
            "expected an evolve target path",
        )
    }

    /// Parse the value expression of a `default` step, reporting against `err`.
    fn evolve_value(&mut self, tokens: &[Token], err: SourceSpan) -> Option<Expression> {
        if tokens.is_empty() {
            self.error_span(
                err,
                ParseDiagnosticReason::Expected(ExpectedSyntax::DefaultValue),
                "expected a default value after `=`",
            );
            return None;
        }
        self.parse_expr_with_fallback(
            tokens,
            err,
            ParseDiagnosticReason::Expected(ExpectedSyntax::DefaultValue),
            "expected a default value expression",
        )
    }

    /// Parse `tokens` as one complete expression. A failure the expression parser
    /// reports at its own token yields `None` directly; a complete expression
    /// followed by trailing tokens is reported once against `err` with
    /// `reason`/`message`, the evolve step's own account of the failure.
    pub(super) fn parse_expr_with_fallback(
        &mut self,
        tokens: &[Token],
        err: SourceSpan,
        reason: ParseDiagnosticReason,
        message: &'static str,
    ) -> Option<Expression> {
        let gap = tokens
            .first()
            .map_or_else(SourceSpan::default, |token| token.span);
        match ExprParser::new(self.source, tokens, gap).parse_complete(&mut self.diagnostics) {
            ParseComplete::Complete(expr) => Some(expr),
            ParseComplete::Reported => None,
            ParseComplete::Incomplete(_) => {
                self.error_span(err, reason, message);
                None
            }
        }
    }
}
