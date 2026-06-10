//! The statement parser: a recursive-descent parser for a function body over
//! the file-wide token stream. It frames compound statements (`if`, `while`,
//! `for`, `try`, `match`) and their nested blocks, keeping layout tokens so a
//! statement may span several physical lines inside open delimiters.

use super::head::arm_member_path;
use super::statement_lines::{parse_catch_header, parse_for_header, parse_simple_statement};
use super::tokens::{
    comment_from_token, expr_of, first_line_end, is_line_comment, line_end, line_span,
};
use crate::ast::{
    Block, CatchClause, Comment, CommentMarker, CommentPlacement, ElseIf, Expression, MatchArm,
    Statement,
};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, Severity,
    SourceSpan,
};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};
use crate::{NESTING_DEPTH_LIMIT, NESTING_LIMIT, PARSE_SYNTAX};

/// A block-introducing keyword that has no statement of its own and only ever
/// appears as a clause of one (`else`, `catch`, `finally`). Standing alone it
/// cannot be structured, so the statement parser swallows it and its nested
/// block, reporting the stray keyword so the following statements still parse.
/// The keywords with dedicated statement parsers (`if`, `while`, …) are matched
/// before this guard and never reach it.
fn is_stray_block_clause_keyword(keyword: Keyword) -> bool {
    matches!(keyword, Keyword::Else | Keyword::Catch | Keyword::Finally)
}

/// Parses the statements of a function body over the file-wide token stream.
/// It keeps layout tokens (`NEWLINE`, `INDENT`, `DEDENT`) so statements that
/// span several physical lines inside open delimiters are one statement, and
/// delegates expression parsing to `ExprParser`.
pub(super) struct StmtParser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
    /// Line comments for the block currently being parsed, in source order.
    /// Each nested block swaps in a fresh accumulator (see `parse_nested_block`)
    /// so a comment lands in the block it appears in.
    comments: Vec<Comment>,
    /// Parse errors for statement lines the body parser cannot structure, so a
    /// malformed statement becomes a deterministic diagnostic instead of being
    /// silently accepted.
    diagnostics: Vec<Diagnostic>,
    /// How many nested blocks deep the parser currently is. Each compound
    /// statement's indented body descends one level; exceeding
    /// [`NESTING_DEPTH_LIMIT`] stops the descent with a located [`NESTING_LIMIT`]
    /// error before the native stack can overflow.
    depth: usize,
}

impl<'a> StmtParser<'a> {
    pub(super) fn new(source: &'a str, tokens: &'a [Token]) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            comments: Vec::new(),
            diagnostics: Vec::new(),
            depth: 0,
        }
    }

    pub(super) fn parse_block(mut self) -> (Vec<Statement>, Vec<Comment>, Vec<Diagnostic>) {
        if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.advance();
        }
        let statements = self.statements();
        (
            statements,
            std::mem::take(&mut self.comments),
            std::mem::take(&mut self.diagnostics),
        )
    }

    /// Record an own-line comment token (a leading or standalone comment) for
    /// the current block and consume its trailing `NEWLINE`.
    fn take_own_line_comment(&mut self) {
        let token = self.advance();
        self.comments.push(comment_from_token(
            self.source,
            token,
            CommentPlacement::OwnLine,
            CommentMarker::Line,
        ));
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    fn statements(&mut self) -> Vec<Statement> {
        let mut statements = Vec::new();
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Eof => break,
                TokenKind::Dedent => break,
                TokenKind::Newline => {
                    self.advance();
                }
                kind if is_line_comment(kind) => self.take_own_line_comment(),
                TokenKind::Indent => {
                    self.report_unexpected_indented_block();
                    self.skip_block();
                }
                _ => statements.extend(self.statement()),
            }
        }
        statements
    }

    pub(super) fn peek(&self) -> Option<TokenKind> {
        self.tokens.get(self.pos).map(|token| token.kind)
    }

    pub(super) fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos];
        self.pos += 1;
        token
    }

    /// Parse one statement, or `None` when the line does not form a statement.
    /// A line the grammar cannot structure raises a diagnostic and is dropped,
    /// so the following statements still parse.
    fn statement(&mut self) -> Option<Statement> {
        // A loop label (`outer:`) precedes a `while` or `for`. `try_loop_label`
        // only consumes the label when one of those keywords follows, so the
        // `_` arm is necessarily `while`.
        if let Some((label, label_span)) = self.try_loop_label() {
            return match self.peek() {
                Some(TokenKind::Keyword(Keyword::For)) => {
                    self.for_stmt(Some(label), Some(label_span))
                }
                _ => Some(self.while_stmt(Some(label), Some(label_span))),
            };
        }

        match self.tokens[self.pos].kind {
            TokenKind::Keyword(Keyword::If) => return Some(self.if_stmt()),
            TokenKind::Keyword(Keyword::While) => return Some(self.while_stmt(None, None)),
            TokenKind::Keyword(Keyword::For) => return self.for_stmt(None, None),
            TokenKind::Keyword(Keyword::Transaction) => return Some(self.transaction_stmt()),
            TokenKind::Keyword(Keyword::Lock) => {
                self.skip_reserved_compound("lock");
                return None;
            }
            TokenKind::Keyword(Keyword::Try) => return Some(self.try_stmt()),
            TokenKind::Keyword(Keyword::Match) => return Some(self.match_stmt()),
            TokenKind::Keyword(keyword) if is_stray_block_clause_keyword(keyword) => {
                self.skip_compound();
                return None;
            }
            _ => {}
        }

        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let before = self.diagnostics.len();
        let statement = parse_simple_statement(self.source, line, &mut self.diagnostics);
        // Suppress the generic fallback when an inline syntax rule already reported.
        if statement.is_none() && self.diagnostics.len() == before {
            let span = line_span(&self.tokens[self.pos..content_end]);
            self.error_span_reason(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::Statement),
                "expected a statement",
            );
        }
        self.pos = (newline + 1).min(self.tokens.len());
        statement
    }

    /// If the token just before `line_end` is a trailing comment, record it as
    /// a `Trailing` comment for the current block and return the index that
    /// excludes it; otherwise return `line_end` unchanged. `line_end` is the
    /// index of the `NEWLINE`/`INDENT`/`DEDENT` that ends the current line.
    fn split_trailing_comment(&mut self, line_end: usize) -> usize {
        if line_end > self.pos && is_line_comment(self.tokens[line_end - 1].kind) {
            let token = self.tokens[line_end - 1];
            self.comments.push(comment_from_token(
                self.source,
                token,
                CommentPlacement::Trailing,
                CommentMarker::Line,
            ));
            line_end - 1
        } else {
            line_end
        }
    }

    /// If the upcoming tokens are `identifier ":" ("while" | "for")`, consume
    /// the label and colon and return the label name and its span.
    fn try_loop_label(&mut self) -> Option<(String, SourceSpan)> {
        let name = self.tokens.get(self.pos)?;
        if name.kind != TokenKind::Identifier
            || self.peek_at(1) != Some(TokenKind::Colon)
            || !matches!(
                self.peek_at(2),
                Some(TokenKind::Keyword(Keyword::While | Keyword::For))
            )
        {
            return None;
        }
        let label = name.text(self.source).to_string();
        let span = name.span;
        self.advance(); // label identifier
        self.advance(); // `:`
        Some((label, span))
    }

    fn peek_at(&self, offset: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + offset).map(|token| token.kind)
    }

    fn while_stmt(&mut self, label: Option<String>, label_span: Option<SourceSpan>) -> Statement {
        let keyword = self.advance(); // `while`
        let start = label_span.unwrap_or(keyword.span);
        let condition = self.header_expression();
        let body = self.block_body();
        Statement::While {
            label,
            condition,
            span: join_spans(start, body.span),
            body,
        }
    }

    fn for_stmt(
        &mut self,
        label: Option<String>,
        label_span: Option<SourceSpan>,
    ) -> Option<Statement> {
        let keyword = self.advance(); // `for`
        let start = label_span.unwrap_or(keyword.span);
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let header = &self.tokens[self.pos..content_end];
        let header_span = line_span(header);
        let parsed = parse_for_header(self.source, header, &mut self.diagnostics);
        self.pos = (newline + 1).min(self.tokens.len());
        let body = self.block_body();

        match parsed {
            Some((binding, iterable, step)) => Some(Statement::For {
                label,
                binding,
                iterable,
                step,
                span: join_spans(start, body.span),
                body,
            }),
            None => {
                self.error_span_reason(
                    header_span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Statement),
                    "expected `for <binding> in <iterable>`",
                );
                None
            }
        }
    }

    /// Parse `try ... [catch ...] [finally ...]`. The grammar requires at least
    /// one of catch/finally, and `return`/`break`/`continue` are forbidden
    /// inside `finally`; both are semantic rules left to the checker, which has
    /// the loop/label scope needed to apply the `finally` rule correctly.
    fn try_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `try`
        self.consume_header_line();
        let body = self.block_body();
        let mut end = body.span;

        let catch = if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Catch))) {
            let clause = self.catch_clause();
            end = clause.block.span;
            Some(clause)
        } else {
            None
        };

        let finally = if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Finally))) {
            self.advance(); // `finally`
            self.consume_header_line();
            let block = self.block_body();
            end = block.span;
            Some(block)
        } else {
            None
        };

        Statement::Try {
            body,
            catch,
            finally,
            span: join_spans(start, end),
        }
    }

    fn catch_clause(&mut self) -> CatchClause {
        self.advance(); // `catch`
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let header = &self.tokens[self.pos..content_end];
        let (name, ty) = parse_catch_header(self.source, header);
        self.pos = (newline + 1).min(self.tokens.len());
        let block = self.block_body();
        CatchClause { name, ty, block }
    }

    fn if_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `if`
        let condition = self.header_expression();
        let then_block = self.block_body();
        let mut end = then_block.span;
        let mut else_ifs = Vec::new();
        let mut else_block = None;

        while matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Else))) {
            self.advance(); // `else`
            if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::If))) {
                self.advance(); // `if`
                let condition = self.header_expression();
                let block = self.block_body();
                end = block.span;
                else_ifs.push(ElseIf { condition, block });
            } else {
                self.consume_header_line();
                let block = self.block_body();
                end = block.span;
                else_block = Some(block);
                break;
            }
        }

        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            span: join_spans(start, end),
        }
    }

    fn transaction_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `transaction`
        self.consume_header_line();
        let body = self.block_body();
        Statement::Transaction {
            span: join_spans(start, body.span),
            body,
        }
    }

    /// Parse `match <scrutinee>` followed by an indented block of arms. Each arm is
    /// a member path on its own line (`bengal`, `tiger::bengal`, or a category
    /// `tiger`), then an indented arm block — the scrutinee supplies the enum, so an
    /// arm names a member relative to it, not `Enum::member`. A local enum's `match`
    /// has no wildcard arm; exhaustiveness and member validity are checker rules, so
    /// the parser only structures the arms.
    fn match_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `match`
        let scrutinee = self.header_expression();
        let mut end = start;
        let mut arms = Vec::new();
        if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.advance(); // INDENT
            while let Some(kind) = self.peek() {
                match kind {
                    TokenKind::Dedent => {
                        end = self.advance().span;
                        break;
                    }
                    TokenKind::Newline => {
                        self.advance();
                    }
                    kind if is_line_comment(kind) => self.take_own_line_comment(),
                    // A stray nested block under an arm header is skipped rather
                    // than mis-parsed; the arm header itself opens its own block.
                    TokenKind::Indent => {
                        self.skip_block();
                    }
                    _ => {
                        if let Some(arm) = self.match_arm() {
                            end = arm.block.span;
                            arms.push(arm);
                        }
                    }
                }
            }
        } else {
            self.error_span_reason(
                start,
                ParseDiagnosticReason::Expected(ExpectedSyntax::MatchBody),
                "expected an indented match body",
            );
        }
        Statement::Match {
            scrutinee,
            arms,
            enum_name: None,
            enum_module: None,
            span: join_spans(start, end),
        }
    }

    /// Parse one `match` arm: a member-path header line (`bengal`, `tiger::bengal`,
    /// or a category `tiger`) relative to the scrutinee enum, then its indented
    /// block. An arm header that is not a `::`-separated run of identifiers is a
    /// parse error.
    fn match_arm(&mut self) -> Option<MatchArm> {
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let header = &self.tokens[self.pos..content_end];
        let span = line_span(header);
        let Some(path) = arm_member_path(self.source, header) else {
            self.error_span_reason(
                span,
                ParseDiagnosticReason::MatchArmMemberPath,
                "a match arm is a member path relative to the enum",
            );
            self.pos = (newline + 1).min(self.tokens.len());
            self.skip_block_if_indented();
            return None;
        };
        self.pos = (newline + 1).min(self.tokens.len());
        let block = self.block_body();
        Some(MatchArm {
            path,
            span: join_spans(span, block.span),
            block,
        })
    }

    /// Skip an indented block if one immediately follows, used to recover after a
    /// malformed arm header so its body does not leak into the surrounding arms.
    fn skip_block_if_indented(&mut self) {
        if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.skip_block();
        }
    }

    /// Parse the expression that ends the current header line, consuming up to
    /// and including its `NEWLINE`.
    /// Returns `None`, after raising a syntax error, when the header does not
    /// parse as a complete expression.
    fn header_expression(&mut self) -> Option<Expression> {
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let before = self.diagnostics.len();
        let expr = expr_of(self.source, line, &mut self.diagnostics);
        // Suppress the generic fallback when an inline syntax rule already reported.
        if expr.is_none() && self.diagnostics.len() == before {
            self.error_span_reason(
                line_span(&self.tokens[self.pos..content_end]),
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected an expression",
            );
        }
        self.pos = (newline + 1).min(self.tokens.len());
        expr
    }

    /// Consume the rest of a header line up to and including its `NEWLINE`.
    /// Used for headers with no expression (`transaction`, `else`), so any
    /// stray tokens before the newline do not leak into the block body.
    fn consume_header_line(&mut self) {
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Newline => {
                    self.advance();
                    break;
                }
                TokenKind::Indent | TokenKind::Dedent => break,
                kind if is_line_comment(kind) => {
                    let token = self.advance();
                    self.comments.push(comment_from_token(
                        self.source,
                        token,
                        CommentPlacement::Trailing,
                        CommentMarker::Line,
                    ));
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// Parse an indented block that follows a compound-statement header. If no
    /// `INDENT` is present (a malformed empty body), returns an empty block.
    fn block_body(&mut self) -> Block {
        if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_nested_block()
        } else {
            let span = self
                .tokens
                .get(self.pos)
                .map(|token| token.span)
                .unwrap_or_default();
            Block {
                statements: Vec::new(),
                comments: Vec::new(),
                span,
            }
        }
    }

    /// Parse `INDENT statement* DEDENT`, tolerating a missing trailing `DEDENT`
    /// at the end of the body token slice. A fresh comment accumulator is swapped
    /// in for the duration so this nested block's comments do not leak into the
    /// parent block.
    ///
    /// Each nested block descends one level. Past [`NESTING_DEPTH_LIMIT`] the body
    /// is skipped with a located [`NESTING_LIMIT`] error rather than recursed into,
    /// so deeply nested compound statements fail closed instead of overflowing the
    /// stack.
    fn parse_nested_block(&mut self) -> Block {
        self.depth += 1;
        if self.depth > NESTING_DEPTH_LIMIT {
            self.depth -= 1;
            return self.nesting_limited_block();
        }
        let start = self.advance().span; // `INDENT`
        let outer = std::mem::take(&mut self.comments);
        let statements = self.statements();
        let comments = std::mem::replace(&mut self.comments, outer);
        let end = if matches!(self.peek(), Some(TokenKind::Dedent)) {
            self.advance().span
        } else {
            statements.last().map_or(start, Statement::span)
        };
        self.depth -= 1;
        Block {
            statements,
            comments,
            span: join_spans(start, end),
        }
    }

    /// Skip an over-deep block whole and report the nesting overflow at its
    /// opening token, returning an empty block in its place. Skipping rather than
    /// recursing keeps the parser off the stack while still consuming the tokens
    /// so the surrounding statements parse.
    fn nesting_limited_block(&mut self) -> Block {
        let start = self.tokens[self.pos].span;
        let end = self.skip_block();
        let span = join_spans(start, end);
        self.diagnostics.push(Diagnostic {
            code: NESTING_LIMIT,
            reason: DiagnosticReason::Parser(ParseDiagnosticReason::NestingLimit),
            severity: Severity::Error,
            message: format!("statements nest deeper than the limit of {NESTING_DEPTH_LIMIT}"),
            help: None,
            span,
        });
        Block {
            statements: Vec::new(),
            comments: Vec::new(),
            span,
        }
    }

    /// Index of the `NEWLINE` (or layout token) that ends the current line.
    fn find_line_end(&self) -> usize {
        line_end(self.tokens, self.pos)
    }

    fn report_unexpected_indented_block(&mut self) {
        if let Some(span) = self.first_indented_content_span() {
            self.error_span_reason(
                span,
                ParseDiagnosticReason::UnexpectedIndentation,
                "unexpected indentation; only compound statements introduce nested blocks",
            );
        }
    }

    fn first_indented_content_span(&self) -> Option<SourceSpan> {
        let mut depth = 0usize;
        let mut index = self.pos;
        while let Some(token) = self.tokens.get(index) {
            match token.kind {
                TokenKind::Indent => depth += 1,
                TokenKind::Dedent => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return None;
                    }
                }
                TokenKind::Newline | TokenKind::Comment | TokenKind::DocComment => {}
                TokenKind::Eof => return None,
                _ if depth > 0 => {
                    let line_start = token.span.start_byte - (token.span.column as usize - 1);
                    return Some(SourceSpan {
                        start_byte: token.span.start_byte,
                        end_byte: first_line_end(self.source, line_start),
                        line: token.span.line,
                        column: token.span.column,
                    });
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    /// A block-introducing keyword (such as a stray `else`) appearing where it
    /// cannot be structured. Report it and consume its header and nested block
    /// so the following statements still parse.
    fn skip_compound(&mut self) {
        self.swallow_block_statement(
            ParseDiagnosticReason::Expected(ExpectedSyntax::Statement),
            "expected a statement",
        );
    }

    /// A reserved block-shaped word that is not part of the v0.1 statement
    /// grammar. Consume the header and nested block so its body does not leak
    /// into the surrounding statement list.
    fn skip_reserved_compound(&mut self, word: &str) {
        self.swallow_block_statement(
            ParseDiagnosticReason::Reserved(ReservedSyntax::LockStatement),
            format!("`{word}` is reserved and is not a v0.1 statement"),
        );
    }

    /// Consume a block-shaped statement that cannot be structured — its header up
    /// to the `NEWLINE` and any immediately following indented block — and report
    /// the given diagnostic over the whole span so following statements parse.
    fn swallow_block_statement(
        &mut self,
        reason: ParseDiagnosticReason,
        message: impl Into<String>,
    ) {
        let start = self.tokens[self.pos].span;
        let mut end = start;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Newline => {
                    end = self.advance().span;
                    break;
                }
                TokenKind::Indent | TokenKind::Dedent => break,
                _ => end = self.advance().span,
            }
        }
        if matches!(self.peek(), Some(TokenKind::Indent)) {
            end = self.skip_block();
        }
        self.error_span_reason(join_spans(start, end), reason, message);
    }

    fn error_span_reason(
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

    /// Skip a malformed statement block, returning the span of the last token
    /// consumed. This stays a separate owner from `consume_balanced_block`: it
    /// tracks and returns the closing span the caller needs, and on an unmatched
    /// leading `DEDENT` it breaks without consuming the token, leaving the
    /// enclosing block's close for the caller instead of swallowing it.
    fn skip_block(&mut self) -> SourceSpan {
        let mut depth = 0usize;
        let mut end = self.tokens[self.pos].span;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Indent => {
                    depth += 1;
                    end = self.advance().span;
                }
                TokenKind::Dedent => {
                    if depth == 0 {
                        // An unmatched DEDENT closes the enclosing block; leave
                        // it for the caller rather than underflow `depth`.
                        break;
                    }
                    depth -= 1;
                    end = self.advance().span;
                    if depth == 0 {
                        break;
                    }
                }
                _ => end = self.advance().span,
            }
        }
        end
    }
}
