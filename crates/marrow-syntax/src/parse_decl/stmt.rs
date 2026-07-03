//! The statement parser: a recursive-descent parser for a function body over
//! the file-wide token stream. It frames compound statements (`if`, `while`,
//! `for`, `try`, `match`) and their nested blocks, keeping layout tokens so a
//! statement may span several physical lines inside open delimiters.

use super::head::arm_member_path;
use super::statement_lines::{
    parse_catch_header, parse_for_header, parse_if_const_head, parse_simple_statement,
};
use super::tokens::{
    comment_from_token, expr_of_after, first_line_end, is_line_comment, line_end, line_span_or,
};
use crate::ast::{
    Block, CatchClause, Comment, CommentMarker, CommentPlacement, ElseIf, Expression, MatchArm,
    Statement, TypeExpr,
};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, Severity,
    SourceSpan, UnsupportedSyntax,
};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};

enum IfHead {
    Expr(Option<Expression>),
    ConstBinding {
        name: String,
        ty: Option<TypeExpr>,
        value: Expression,
    },
}

/// A block-introducing keyword that has no statement of its own and only ever
/// appears as a clause of one (`else`, `catch`). Standing alone it
/// cannot be structured, so the statement parser swallows it and its nested
/// block, reporting the stray keyword so the following statements still parse.
/// The keywords with dedicated statement parsers (`if`, `while`, …) are matched
/// before this guard and never reach it.
fn is_stray_block_clause_keyword(keyword: Keyword) -> bool {
    matches!(keyword, Keyword::Else | Keyword::Catch)
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
}

impl<'a> StmtParser<'a> {
    pub(super) fn new(source: &'a str, tokens: &'a [Token]) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            comments: Vec::new(),
            diagnostics: Vec::new(),
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
    /// the current block and consume its trailing `NEWLINE`. The doc-comment
    /// decision is owned by `classify_line_comment`.
    fn take_own_line_comment(&mut self) {
        let token = self.advance();
        self.record_line_comment(token, CommentPlacement::OwnLine);
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    /// Sole owner of the line-comment decision: a `;;` doc comment in statement
    /// position has no declaration to attach to, so it is reported rather than
    /// retained — a swallowed doc comment is one the formatter cannot place,
    /// breaking the check-run-format round trip — while an ordinary `;` comment
    /// becomes trivia for the current block at the given placement. Returns the
    /// retained comment for callers that place it conditionally; `None` when the
    /// token was a doc comment that has been reported.
    fn classify_line_comment(
        &mut self,
        token: Token,
        placement: CommentPlacement,
    ) -> Option<Comment> {
        if token.kind == TokenKind::DocComment {
            self.report_doc_comment_without_target(token.span);
            None
        } else {
            Some(comment_from_token(
                self.source,
                token,
                placement,
                CommentMarker::Line,
            ))
        }
    }

    /// Classify a line-comment token and, when it is ordinary trivia, append it
    /// to the current block's comments at `placement`.
    fn record_line_comment(&mut self, token: Token, placement: CommentPlacement) {
        if let Some(comment) = self.classify_line_comment(token, placement) {
            self.comments.push(comment);
        }
    }

    fn report_doc_comment_without_target(&mut self, span: SourceSpan) {
        self.error_span_reason(
            span,
            ParseDiagnosticReason::DocCommentWithoutTarget,
            "a `;;` doc comment must precede a declaration, member, or parameter",
        );
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
                    self.skip_unexpected_indented_block();
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
        self.recover_removed_loop_label();

        match self.tokens[self.pos].kind {
            TokenKind::Keyword(Keyword::If) => return Some(self.if_stmt()),
            TokenKind::Keyword(Keyword::While) => return Some(self.while_stmt()),
            TokenKind::Keyword(Keyword::For) => return self.for_stmt(),
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
            let span = line_span_or(
                &self.tokens[self.pos..content_end],
                self.tokens[self.pos].span,
            );
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
            self.record_line_comment(self.tokens[line_end - 1], CommentPlacement::Trailing);
            line_end - 1
        } else {
            line_end
        }
    }

    fn recover_removed_loop_label(&mut self) {
        let Some(name) = self.tokens.get(self.pos).copied() else {
            return;
        };
        if name.kind != TokenKind::Identifier
            || self.peek_at(1) != Some(TokenKind::Colon)
            || !matches!(
                self.peek_at(2),
                Some(TokenKind::Keyword(Keyword::While | Keyword::For))
            )
        {
            return;
        }
        let colon = self.tokens[self.pos + 1];
        self.error_span_reason(
            join_spans(name.span, colon.span),
            ParseDiagnosticReason::Unsupported(UnsupportedSyntax::LoopLabels),
            "loop labels were removed",
        );
        if let Some(diagnostic) = self.diagnostics.last_mut() {
            diagnostic.help =
                Some("extract a function and use return to leave nested loops".to_string());
        }
        self.advance();
        self.advance();
    }

    fn peek_at(&self, offset: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + offset).map(|token| token.kind)
    }

    fn while_stmt(&mut self) -> Statement {
        let keyword = self.advance(); // `while`
        let condition = self.header_expression(keyword.span);
        let body = self.block_body();
        Statement::While {
            condition,
            span: join_spans(keyword.span, body.span),
            body,
        }
    }

    fn for_stmt(&mut self) -> Option<Statement> {
        let keyword = self.advance(); // `for`
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let header = &self.tokens[self.pos..content_end];
        let header_span = line_span_or(header, keyword.span);
        let parsed = parse_for_header(self.source, header, &mut self.diagnostics);
        self.pos = (newline + 1).min(self.tokens.len());
        let body = self.block_body();

        match parsed {
            Some((binding, iterable, step)) => Some(Statement::For {
                binding,
                iterable,
                step,
                span: join_spans(keyword.span, body.span),
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

    /// Parse `try ... catch ...`. A try block without a catch is retained for
    /// recovery but receives a parser diagnostic.
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
        if let Some(finally_span) = self.consume_removed_finally_clause() {
            end = finally_span;
        }

        if catch.is_none() {
            self.error_span_reason(
                start,
                ParseDiagnosticReason::Expected(ExpectedSyntax::Statement),
                "`try` requires a `catch` clause",
            );
            if let Some(diagnostic) = self.diagnostics.last_mut() {
                diagnostic.help =
                    Some("catch, clean up, then rethrow when cleanup is needed".to_string());
            }
        }

        Statement::Try {
            body,
            catch,
            span: join_spans(start, end),
        }
    }

    fn consume_removed_finally_clause(&mut self) -> Option<SourceSpan> {
        let token = self.tokens.get(self.pos).copied()?;
        if token.kind != TokenKind::Identifier || token.text(self.source) != "finally" {
            return None;
        }
        let line_end = self.find_line_end();
        let has_trailing_comment =
            line_end > self.pos && is_line_comment(self.tokens[line_end - 1].kind);
        let content_end = if has_trailing_comment {
            line_end - 1
        } else {
            line_end
        };
        if content_end != self.pos + 1 {
            return None;
        }
        let after_header = match self.tokens.get(line_end).map(|token| token.kind) {
            Some(TokenKind::Newline) => line_end + 1,
            _ => line_end,
        };
        if self.tokens.get(after_header).map(|token| token.kind) != Some(TokenKind::Indent) {
            return None;
        }
        self.error_span_reason(
            token.span,
            ParseDiagnosticReason::Unsupported(UnsupportedSyntax::Finally),
            "`finally` blocks were removed",
        );
        if let Some(diagnostic) = self.diagnostics.last_mut() {
            diagnostic.help =
                Some("catch, clean up, then rethrow when cleanup is needed".to_string());
        }
        self.consume_header_line();
        let end = self.skip_block();
        Some(join_spans(token.span, end))
    }

    fn catch_clause(&mut self) -> CatchClause {
        let keyword = self.advance(); // `catch`
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let header = &self.tokens[self.pos..content_end];
        let (name, ty) = match parse_catch_header(self.source, header) {
            Ok(parsed) => parsed,
            Err(error) => {
                let (span, reason, message) = error.locate(line_span_or(header, keyword.span));
                self.error_span_reason(span, reason, message);
                (String::new(), None)
            }
        };
        self.pos = (newline + 1).min(self.tokens.len());
        let block = self.block_body();
        CatchClause { name, ty, block }
    }

    fn if_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `if`
        let head = self.if_head(start);
        let then_block = self.block_body();
        let mut end = then_block.span;
        let mut else_ifs = Vec::new();
        let mut else_block = None;

        while matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Else))) {
            self.advance(); // `else`
            if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::If))) {
                let if_keyword = self.advance(); // `if`
                let condition = self.header_expression(if_keyword.span);
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

        match head {
            IfHead::Expr(condition) => Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                span: join_spans(start, end),
            },
            IfHead::ConstBinding { name, ty, value } => Statement::IfConst {
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block,
                span: join_spans(start, end),
            },
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
        let scrutinee = self.header_expression(start);
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
        let span = line_span_or(header, self.tokens[self.pos].span);
        let Some((path, path_spans)) = arm_member_path(self.source, header) else {
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
            path_spans,
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
    /// and including its `NEWLINE`. `keyword` is the header keyword
    /// (`while`/`if`/`match`) already consumed; an empty header reports the
    /// missing expression at the gap just past it, never the start of input.
    /// Returns `None`, after raising a syntax error, when the header does not
    /// parse as a complete expression.
    fn header_expression(&mut self, keyword: SourceSpan) -> Option<Expression> {
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let before = self.diagnostics.len();
        let expr = expr_of_after(self.source, line, keyword, &mut self.diagnostics);
        // Suppress the generic fallback when an inline syntax rule already reported.
        if expr.is_none() && self.diagnostics.len() == before {
            self.error_span_reason(
                line_span_or(&self.tokens[self.pos..content_end], keyword),
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected an expression",
            );
        }
        self.pos = (newline + 1).min(self.tokens.len());
        expr
    }

    fn if_head(&mut self, keyword: SourceSpan) -> IfHead {
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let before = self.diagnostics.len();
        let head = if matches!(
            line.first().map(|token| token.kind),
            Some(TokenKind::Keyword(Keyword::Const))
        ) {
            parse_if_const_head(self.source, line, &mut self.diagnostics)
                .map_or(IfHead::Expr(None), |(name, ty, value)| {
                    IfHead::ConstBinding { name, ty, value }
                })
        } else {
            IfHead::Expr(expr_of_after(
                self.source,
                line,
                keyword,
                &mut self.diagnostics,
            ))
        };
        if matches!(head, IfHead::Expr(None)) && self.diagnostics.len() == before {
            self.error_span_reason(
                line_span_or(&self.tokens[self.pos..content_end], keyword),
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected an expression",
            );
        }
        self.pos = (newline + 1).min(self.tokens.len());
        head
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
                    self.record_line_comment(token, CommentPlacement::Trailing);
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
    fn parse_nested_block(&mut self) -> Block {
        let start = self.advance().span; // `INDENT`
        let outer = std::mem::take(&mut self.comments);
        let statements = self.statements();
        let comments = std::mem::replace(&mut self.comments, outer);
        let end = if matches!(self.peek(), Some(TokenKind::Dedent)) {
            self.advance().span
        } else {
            statements.last().map_or(start, Statement::span)
        };
        Block {
            statements,
            comments,
            span: join_spans(start, end),
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
            code: reason.code(),
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

    fn skip_unexpected_indented_block(&mut self) -> SourceSpan {
        let mut depth = 0usize;
        let mut end = self.tokens[self.pos].span;
        let mut line_has_content = false;
        let mut comments = Vec::new();
        let mut has_statement_tokens = false;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Indent => {
                    depth += 1;
                    line_has_content = false;
                    end = self.advance().span;
                }
                TokenKind::Dedent => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    line_has_content = false;
                    end = self.advance().span;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Newline => {
                    line_has_content = false;
                    end = self.advance().span;
                }
                kind if is_line_comment(kind) => {
                    let token = self.advance();
                    end = token.span;
                    if let Some(comment) =
                        self.classify_line_comment(token, CommentPlacement::OwnLine)
                        && !line_has_content
                    {
                        comments.push(comment);
                    }
                }
                _ => {
                    has_statement_tokens = true;
                    line_has_content = true;
                    end = self.advance().span;
                }
            }
        }
        if !has_statement_tokens {
            self.comments.extend(comments);
        }
        end
    }
}
