//! The `evolve` declaration body: the indented run of evolution steps
//! (`rename`, `default`, `retire`, `transform`) and their target-path and value
//! expressions.

use super::DeclParser;
use super::tokens::{find_arrow, find_top_level_equal};
use crate::ast::{Block, EvolveDecl, EvolveStep, Expression};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::parse_expr::ExprParser;
use crate::token::{Token, TokenKind};

impl<'a> DeclParser<'a> {
    /// Parse an `evolve` block: the bare keyword header, then an indented run of
    /// evolution steps. Each step is dispatched on its contextual lead word
    /// (`rename`/`default`/`retire`/`transform`); a `transform` carries a nested
    /// statement block, which the statement parser frames.
    pub(super) fn parse_evolve(&mut self) -> EvolveDecl {
        let span = self.header_span();
        self.take_header_line(); // `evolve`
        let steps = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_evolve_steps()
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::EvolveBody),
                "expected an indented evolve body",
            );
            Vec::new()
        };
        EvolveDecl { steps, span }
    }

    /// Parse the `INDENT … DEDENT` block of evolution steps. A `transform` step
    /// owns the indented statement block that follows its header; the other steps
    /// are single header lines.
    fn parse_evolve_steps(&mut self) -> Vec<EvolveStep> {
        let mut steps = Vec::new();
        self.advance(); // INDENT
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Dedent => {
                    self.advance();
                    break;
                }
                TokenKind::Newline => {
                    self.advance();
                }
                TokenKind::Comment | TokenKind::DocComment => {
                    self.advance();
                }
                // Only a transform owns an indented body, which it consumes right
                // after parsing its header. An indented block reaching here sits
                // under a rename, default, or retire step, where it is a mistake.
                TokenKind::Indent => {
                    let span = self.content_span();
                    self.error_span(
                        span,
                        ParseDiagnosticReason::UnexpectedIndentation,
                        "unexpected indented block under an evolve step; only `transform` has a body",
                    );
                    self.advance();
                    self.skip_to_block_end();
                }
                _ => {
                    if let Some(step) = self.parse_evolve_step() {
                        steps.push(step);
                    }
                }
            }
        }
        steps
    }

    /// Parse one evolution step from its header line. The lead word selects the
    /// step kind; an unknown lead is reported and its line dropped so the
    /// following steps still parse.
    fn parse_evolve_step(&mut self) -> Option<EvolveStep> {
        let span = self.header_span();
        let err = self.content_span();
        let lead = self.tokens[self.pos];
        let lead_word = (lead.kind == TokenKind::Identifier).then(|| lead.text(self.source));
        match lead_word {
            Some("rename") => self.parse_evolve_rename(span),
            Some("default") => self.parse_evolve_default(span),
            Some("retire") => self.parse_evolve_retire(span),
            Some("transform") => self.parse_evolve_transform(span),
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
    fn parse_evolve_rename(&mut self, span: SourceSpan) -> Option<EvolveStep> {
        let err = self.content_span();
        let header = self.take_header_line();
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
    fn parse_evolve_default(&mut self, span: SourceSpan) -> Option<EvolveStep> {
        let err = self.content_span();
        let header = self.take_header_line();
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
    fn parse_evolve_retire(&mut self, span: SourceSpan) -> Option<EvolveStep> {
        let err = self.content_span();
        let header = self.take_header_line();
        let target = self.evolve_path(&header[1..], err)?;
        Some(EvolveStep::Retire { target, span })
    }

    /// Parse `transform <target>` followed by an indented statement block.
    fn parse_evolve_transform(&mut self, span: SourceSpan) -> Option<EvolveStep> {
        let err = self.content_span();
        let header = self.take_header_line();
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

    /// Parse `tokens` as one complete expression. When it does not parse and the
    /// expression parser raised nothing more specific, report `reason`/`message`
    /// against `err`. The diagnostic-count guard suppresses this generic fallback
    /// whenever an inline syntax rule (a keyword field name, a reserved form)
    /// already explained the failure, so each position reports once.
    pub(super) fn parse_expr_with_fallback(
        &mut self,
        tokens: &[Token],
        err: SourceSpan,
        reason: ParseDiagnosticReason,
        message: &'static str,
    ) -> Option<Expression> {
        let before = self.diagnostics.len();
        let parsed = ExprParser::new(self.source, tokens).parse_complete(&mut self.diagnostics);
        if parsed.is_none() && self.diagnostics.len() == before {
            self.error_span(err, reason, message);
        }
        parsed
    }
}
