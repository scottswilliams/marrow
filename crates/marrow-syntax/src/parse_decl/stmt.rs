//! The statement parser: a recursive-descent parser for a function body over
//! the file-wide token stream. It frames compound statements (`if`, `while`,
//! `for`, `try`, `match`) and their `{ … }` blocks; statements end at a `NEWLINE`
//! or `}`, and a trailing clause (`else`, `on more`, a checked arm, a match arm)
//! takes either a braced block or a single inline statement.

use super::head::arm_pattern;
use super::statement_lines::{parse_for_header, parse_if_const_head, parse_simple_statement};
use super::tokens::{
    comment_from_token, expr_of, expr_of_after, find_top_level_equal, first_line_end,
    is_line_comment, line_end, line_span_or, parse_type, push_parse_error,
};
use crate::ast::{
    ArmBinding, Block, CheckedBind, Comment, CommentMarker, CommentPlacement, ElseIf, Expression,
    IfConstBinding, MatchArm, Statement, TraversalBound, TypeExpr,
};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, Severity,
    SourceSpan, UnsupportedSyntax,
};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};

enum IfHead {
    Expr(Expression),
    ConstBinding {
        name: String,
        ty: Option<TypeExpr>,
        value: Expression,
    },
    /// B5: `if const a = e1 and const b = e2 and cond` — a chain of existence
    /// bindings joined by `and`, with an optional trailing bare condition.
    Chain {
        bindings: Vec<IfConstBinding>,
        condition: Option<Expression>,
    },
}

/// Which fault a checked arm handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckedFault {
    OutOfRange,
    ZeroDivisor,
}

/// A block-introducing keyword that has no statement of its own and only ever
/// appears as a clause of one (`else`). Standing alone it cannot be structured, so
/// the statement parser swallows it and its nested block, reporting the stray
/// keyword so the following statements still parse. The keywords with dedicated
/// statement parsers (`if`, `while`, …) are matched before this guard and never
/// reach it.
fn is_stray_block_clause_keyword(keyword: Keyword) -> bool {
    matches!(keyword, Keyword::Else)
}

/// Parses the statements of a function body over a token slice (the tokens
/// strictly inside the enclosing `{ … }`). It frames nested `{ … }` blocks itself
/// and delegates expression parsing to `ExprParser`.
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
    /// Nested `{ … }` block depth. The lexer reports the nesting-limit diagnostic;
    /// this second layer stops the recursive descent at [`NESTING_DEPTH_LIMIT`] so a
    /// pathologically deep brace nest skips its body rather than overflowing the
    /// native stack, keeping the AST (and every later walk over it) bounded.
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

    /// Detach a comment that trails this construct's header — recorded while taking
    /// the header line, so it is the most recent comment and starts after
    /// `header_start` — and hand it back reclassified as an own-line comment. The
    /// block that follows adopts it as its first leading comment, so the `{`-cuddled,
    /// next-line-`{`, and own-line spellings of a header comment all parse to one tree
    /// and format to one fixed point. A trailing comment on an earlier sibling starts
    /// before `header_start` and is left in place.
    fn detach_header_comment(&mut self, header_start: usize) -> Option<Comment> {
        if !self.header_comment_pending(header_start) {
            return None;
        }
        let mut comment = self.comments.pop().expect("checked above");
        comment.placement = CommentPlacement::OwnLine;
        Some(comment)
    }

    /// Reclassify a pending header-trailing comment as an own-line comment in place,
    /// for `match`, whose arms have no single owning block: it then renders as the
    /// first leading arm comment rather than cuddled after `{`.
    fn own_header_comment_in_place(&mut self, header_start: usize) {
        if self.header_comment_pending(header_start)
            && let Some(comment) = self.comments.last_mut()
        {
            comment.placement = CommentPlacement::OwnLine;
        }
    }

    fn header_comment_pending(&self, header_start: usize) -> bool {
        matches!(
            self.comments.last(),
            Some(comment)
                if comment.placement == CommentPlacement::Trailing
                    && comment.span.start_byte > header_start
        )
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
                TokenKind::Eof | TokenKind::RightBrace => break,
                TokenKind::Newline => {
                    self.advance();
                }
                kind if is_line_comment(kind) => self.take_own_line_comment(),
                TokenKind::LeftBrace => {
                    self.report_unexpected_indented_block();
                    self.skip_unexpected_indented_block();
                }
                _ => statements.extend(self.statement()),
            }
        }
        statements
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    /// Whether the next significant token after any `NEWLINE`s is the contextual
    /// identifier `word` (an `on`, `more`, `out_of_range`, ...).
    fn at_word_past_newlines(&self, word: &str) -> bool {
        self.tokens[self.pos..]
            .iter()
            .find(|token| token.kind != TokenKind::Newline)
            .is_some_and(|token| {
                token.kind == TokenKind::Identifier && token.text(self.source) == word
            })
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
            TokenKind::Keyword(Keyword::Try) => return self.try_statement(),
            TokenKind::Keyword(Keyword::Match) => return Some(self.match_stmt()),
            TokenKind::Keyword(Keyword::Assert) => return Some(self.assert_stmt()),
            TokenKind::Keyword(keyword) if is_stray_block_clause_keyword(keyword) => {
                self.skip_compound();
                return None;
            }
            // `throw`/`catch` are no longer keywords: the throw/catch channel was
            // removed. A statement that begins with one is the removed form; report
            // it as unsupported and point at `Result`, keeping the parse total.
            TokenKind::Identifier if self.tokens[self.pos].text(self.source) == "throw" => {
                return self.recover_removed_throw();
            }
            TokenKind::Identifier if self.tokens[self.pos].text(self.source) == "catch" => {
                return self.recover_removed_clause(
                    UnsupportedSyntax::CatchClause,
                    "`catch` was removed; match a `Result<T, E>` on its `ok`/`err` members instead",
                );
            }
            TokenKind::Identifier if self.tokens[self.pos].text(self.source) == "finally" => {
                return self.recover_removed_clause(
                    UnsupportedSyntax::Finally,
                    "`finally` blocks were removed; return a `Result<T, E>` and clean up on its `err` member",
                );
            }
            _ => {}
        }

        // The checked-arithmetic form binds through `const`/`var`/`return`, so its
        // header token is not a distinguishing keyword. Detect it on the header line
        // — its `on` arms live on the following indented lines that `take_line` does
        // not see — before the generic line-based simple-statement path.
        if self.at_checked_form() {
            return Some(self.checked_stmt());
        }

        // B6 let-else: a `const`/`var` binding whose line carries a top-level
        // `else` diverging tail. Parse-only; the checker rejects it until adopted.
        if self.at_let_else() {
            return Some(self.let_else_stmt());
        }

        let start = self.tokens[self.pos].span;
        let line = self.take_line();
        let error_span = line_span_or(line, start);
        let statement = parse_simple_statement(self.source, line, &mut self.diagnostics);
        // Total parsing: a line that did not structure reported its own diagnostic
        // and becomes an error node carrying its span, so the body is never silently
        // short a statement.
        Some(statement.unwrap_or(Statement::Error { span: error_span }))
    }

    /// Take the current statement or header line: the content tokens up to the
    /// token that ends the line, with any trailing comment recorded as block
    /// trivia. A terminating `NEWLINE` is consumed; a block-opening/closing `{`/`}`
    /// is left in place for the caller to frame the body. The returned slice
    /// outlives the advance (it borrows the whole-file token stream), so a caller
    /// parses it after the cursor has moved past the line content.
    fn take_line(&mut self) -> &'a [Token] {
        let end = self.find_line_end();
        let content_end = self.split_trailing_comment(end);
        let line = &self.tokens[self.pos..content_end];
        self.pos = if self.tokens.get(end).map(|token| token.kind) == Some(TokenKind::Newline) {
            end + 1
        } else {
            end
        };
        line
    }

    /// If the token just before `line_end` is a trailing comment, record it as
    /// a `Trailing` comment for the current block and return the index that
    /// excludes it; otherwise return `line_end` unchanged. `line_end` is the
    /// index of the `NEWLINE`/`{`/`}`/`EOF` that ends the current line.
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
        let body = self.block_body(keyword.span.start_byte);
        Statement::While {
            condition,
            span: join_spans(keyword.span, body.span),
            body,
        }
    }

    fn for_stmt(&mut self) -> Option<Statement> {
        let keyword = self.advance(); // `for`
        let header = self.take_line();
        let header_span = line_span_or(header, keyword.span);
        let parsed = parse_for_header(self.source, header);
        let body = self.block_body(keyword.span.start_byte);

        match parsed {
            Some((binding, order, iterable, step, bound_head)) => {
                let mut end = body.span;
                // A durable traversal takes a mandatory `on more` block dedented like
                // `else`; consume it whenever it trails the body so it never desyncs
                // into a bogus following statement. When the head carried `at most` the
                // block rides its `TraversalBound` (the checker reports a missing arm);
                // when it did not, the head is unbounded and the checker reports that at
                // the head — the trailing block is consumed and dropped either way.
                let on_more = self.take_on_more_block();
                if let Some(block) = &on_more {
                    end = block.span;
                }
                let bound = bound_head.map(|(limit, from)| TraversalBound {
                    limit,
                    from,
                    on_more,
                });
                Some(Statement::For {
                    binding,
                    order,
                    iterable,
                    step,
                    bound,
                    span: join_spans(keyword.span, end),
                    body,
                })
            }
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

    /// Consume a trailing `on more` clause, if present: the contextual `on more`
    /// keywords cuddling the loop body's `}` (or on the next line) followed by a
    /// braced or inline diverging body. Returns `None` when the next tokens are not
    /// that phrase, restoring the cursor so a following sibling statement parses.
    fn take_on_more_block(&mut self) -> Option<Block> {
        let save = self.pos;
        self.skip_newlines();
        let is_on = self.tokens.get(self.pos).is_some_and(|token| {
            token.kind == TokenKind::Identifier && token.text(self.source) == "on"
        });
        let is_more = self.tokens.get(self.pos + 1).is_some_and(|token| {
            token.kind == TokenKind::Identifier && token.text(self.source) == "more"
        });
        if !(is_on && is_more) {
            self.pos = save;
            return None;
        }
        self.advance(); // `on`
        self.advance(); // `more`
        Some(self.parse_clause_body())
    }

    /// Parse a statement that begins with `try`. Prefix `try <expr>` is a value
    /// form: it propagates a `Result<T, E>`'s `err` out of the enclosing
    /// `Result`-returning function, yielding the `ok` value. The removed block form
    /// (`try` opening an indented body, with `catch`/`finally`) is reported as
    /// unsupported and its blocks are skipped so the parse stays total.
    fn try_statement(&mut self) -> Option<Statement> {
        let start = self.advance().span; // `try`
        let header = self.take_line();
        // The removed block form opens a `{ … }` body after the header.
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.skip_block();
            self.consume_removed_try_clauses();
            self.error_span_reason(
                start,
                ParseDiagnosticReason::Unsupported(UnsupportedSyntax::TryCatchBlock),
                "block-form `try`/`catch` was removed; return a `Result<T, E>` and propagate it with prefix `try <expr>`",
            );
            return None;
        }
        if header.is_empty() {
            self.error_span_reason(
                start,
                ParseDiagnosticReason::Unsupported(UnsupportedSyntax::TryCatchBlock),
                "prefix `try` needs a `Result<T, E>` expression, as `try <expr>`",
            );
            return None;
        }
        let error_span = line_span_or(header, start);
        let inner = expr_of_after(self.source, header, start, &mut self.diagnostics).unwrap_or(
            Expression::Error {
                span: error_span,
                recovery: None,
            },
        );
        let span = join_spans(start, inner.span());
        Some(Statement::Expr {
            value: Expression::Try {
                inner: Box::new(inner),
                span,
            },
            span,
        })
    }

    /// Consume the `catch`/`finally` clauses that followed a removed block `try`,
    /// so their headers and bodies do not leak into the surrounding statements.
    fn consume_removed_try_clauses(&mut self) {
        loop {
            let Some(token) = self.tokens.get(self.pos).copied() else {
                return;
            };
            let is_clause = token.kind == TokenKind::Identifier
                && matches!(token.text(self.source), "catch" | "finally");
            if !is_clause {
                return;
            }
            self.consume_header_line();
            if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
                self.skip_block();
            }
        }
    }

    /// Recover a removed `throw <expr>` statement: consume its line and report the
    /// throw/catch channel as unsupported, pointing at `Result`.
    fn recover_removed_throw(&mut self) -> Option<Statement> {
        let start = self.tokens[self.pos].span;
        let line = self.take_line();
        let span = line_span_or(line, start);
        self.error_span_reason(
            span,
            ParseDiagnosticReason::Unsupported(UnsupportedSyntax::ThrowStatement),
            "`throw` was removed; return a `Result<T, E>` with `err(...)` and propagate it with `try`",
        );
        None
    }

    /// Recover a stray removed block clause (`catch`/`finally`): consume its header
    /// line and any indented block, reporting `reason` at its header.
    fn recover_removed_clause(
        &mut self,
        reason: UnsupportedSyntax,
        message: &'static str,
    ) -> Option<Statement> {
        let start = self.tokens[self.pos].span;
        self.consume_header_line();
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.skip_block();
        }
        self.error_span_reason(start, ParseDiagnosticReason::Unsupported(reason), message);
        None
    }

    fn if_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `if`
        let head = self.if_head(start);
        let then_block = self.block_body(start.start_byte);
        let mut end = then_block.span;
        let mut else_ifs = Vec::new();
        let mut else_block = None;

        // A trailing `else`/`else if` cuddles the then-block's `}` (`} else {`) or
        // sits on the next line; look past the separating newlines and restore the
        // cursor when no `else` follows so the newline still ends the statement.
        loop {
            let save = self.pos;
            self.skip_newlines();
            if !matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Else))) {
                self.pos = save;
                break;
            }
            self.advance(); // `else`
            if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::If))) {
                let if_keyword = self.advance(); // `if`
                let condition = self.header_expression(if_keyword.span);
                let block = self.block_body(if_keyword.span.start_byte);
                end = block.span;
                else_ifs.push(ElseIf { condition, block });
            } else {
                let block = self.parse_clause_body();
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
            IfHead::Chain {
                bindings,
                condition,
            } => Statement::IfConstChain {
                bindings,
                condition,
                then_block,
                else_ifs,
                else_block,
                span: join_spans(start, end),
            },
        }
    }

    /// Parse `assert <expr>`: the header keyword, then a bool condition running to
    /// the end of the line. The checker owns the rule that `assert` is legal only in
    /// a `test` body; the parser only structures it.
    fn assert_stmt(&mut self) -> Statement {
        let keyword = self.advance().span; // `assert`
        let value = self.header_expression(keyword);
        let span = join_spans(keyword, value.span());
        Statement::Assert { value, span }
    }

    fn transaction_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `transaction`
        self.consume_header_line();
        let body = self.block_body(start.start_byte);
        Statement::Transaction {
            span: join_spans(start, body.span),
            body,
        }
    }

    /// Parse `match <scrutinee> { <arms> }`. Each arm is `pattern => stmt|{ block }`,
    /// where the pattern is a member path relative to the scrutinee enum (`bengal`,
    /// `tiger::bengal`, or a category `tiger`) with optional payload bindings. A
    /// local enum's `match` has no wildcard arm; exhaustiveness and member validity
    /// are checker rules, so the parser only structures the arms.
    fn match_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `match`
        let scrutinee = self.header_expression(start);
        // A comment trailing the `match` header before its `{` becomes an own-line
        // comment leading the first arm, the one owner `match` arms share.
        self.own_header_comment_in_place(start.start_byte);
        let mut end = start;
        let mut arms = Vec::new();
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.advance(); // `{`
            loop {
                match self.peek() {
                    None | Some(TokenKind::RightBrace) => {
                        if matches!(self.peek(), Some(TokenKind::RightBrace)) {
                            end = self.advance().span;
                        }
                        break;
                    }
                    Some(TokenKind::Newline) => {
                        self.advance();
                    }
                    Some(kind) if is_line_comment(kind) => self.take_own_line_comment(),
                    // A stray nested block where an arm header was expected is skipped
                    // rather than mis-parsed.
                    Some(TokenKind::LeftBrace) => {
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
                "expected a `{ … }` match body",
            );
        }
        Statement::Match {
            scrutinee,
            arms,
            span: join_spans(start, end),
        }
    }

    /// Parse one `match` arm: `pattern => stmt|{ block }`. The pattern is a member
    /// path relative to the scrutinee enum; a header that is not a `::`-separated
    /// run of identifiers, or one with no `=>`, is a parse error.
    fn match_arm(&mut self) -> Option<MatchArm> {
        let start = self.tokens[self.pos].span;
        let line_end = self.find_line_end();
        let arrow =
            (self.pos..line_end).find(|&index| self.tokens[index].kind == TokenKind::FatArrow);
        let Some(arrow) = arrow else {
            let header = &self.tokens[self.pos..line_end];
            let span = line_span_or(header, start);
            self.error_span_reason(
                span,
                ParseDiagnosticReason::MatchArmMemberPath,
                "a match arm is `pattern => statement`, the pattern a member path relative to the enum",
            );
            self.pos = line_end;
            self.skip_block_if_braced();
            return None;
        };
        let pattern_tokens = &self.tokens[self.pos..arrow];
        let span = line_span_or(pattern_tokens, start);
        self.pos = arrow + 1; // past `=>`
        let Some(pattern) = arm_pattern(self.source, pattern_tokens) else {
            self.error_span_reason(
                span,
                ParseDiagnosticReason::MatchArmMemberPath,
                "a match arm is a member path relative to the enum, with optional payload bindings",
            );
            let _ = self.parse_clause_body();
            return None;
        };
        let block = self.parse_clause_body();
        Some(MatchArm {
            path: pattern.path,
            path_spans: pattern.path_spans,
            bindings: pattern
                .bindings
                .into_iter()
                .map(|(name, span)| ArmBinding { name, span })
                .collect(),
            span: join_spans(span, block.span),
            block,
        })
    }

    /// Skip a `{ … }` block if one immediately follows (across any newlines), used
    /// to recover after a malformed arm header so its body does not leak.
    fn skip_block_if_braced(&mut self) {
        let save = self.pos;
        self.skip_newlines();
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.skip_block();
        } else {
            self.pos = save;
        }
    }

    /// Whether the current header line is a checked-arithmetic form: a `const`/`var`
    /// whose value slot after `=` is `checked`, or a `return checked`. Inspects the
    /// header line without consuming it.
    fn at_checked_form(&self) -> bool {
        let line = &self.tokens[self.pos..self.find_line_end()];
        let Some(first) = line.first() else {
            return false;
        };
        let checked = TokenKind::Keyword(Keyword::Checked);
        match first.kind {
            TokenKind::Keyword(Keyword::Return) => {
                line.get(1).map(|token| token.kind) == Some(checked)
            }
            TokenKind::Keyword(Keyword::Const | Keyword::Var) => {
                find_top_level_equal(line)
                    .and_then(|equal| line.get(equal + 1))
                    .map(|token| token.kind)
                    == Some(checked)
            }
            _ => false,
        }
    }

    /// Whether the current line is a B6 let-else: a `const`/`var` binding whose
    /// header line carries a top-level `else` diverging tail. Inspects the line
    /// without consuming it.
    fn at_let_else(&self) -> bool {
        let line = &self.tokens[self.pos..self.find_line_end()];
        let is_binding = matches!(
            line.first().map(|token| token.kind),
            Some(TokenKind::Keyword(Keyword::Const | Keyword::Var))
        );
        is_binding && find_top_level_else(line).is_some()
    }

    /// Parse a B6 let-else: `const`/`var name [: ty] = value else <diverging>`. The
    /// binding before `else` is parsed by the simple-statement parser; the tail is a
    /// braced or inline diverging body. Parse-only; the checker rejects it.
    fn let_else_stmt(&mut self) -> Statement {
        let start = self.tokens[self.pos].span;
        let line_end = self.find_line_end();
        let else_offset = find_top_level_else(&self.tokens[self.pos..line_end])
            .expect("let-else detected before dispatch");
        let binding_end = self.pos + else_offset;
        let binding = parse_simple_statement(
            self.source,
            &self.tokens[self.pos..binding_end],
            &mut self.diagnostics,
        );
        self.pos = binding_end + 1; // past the `else`
        let else_block = self.parse_clause_body();
        let (is_var, name, ty, value) = match binding {
            Some(Statement::Const {
                name, ty, value, ..
            }) => (false, name, ty, value),
            Some(Statement::Var {
                name, ty, value, ..
            }) => (
                true,
                name,
                ty,
                value.unwrap_or(Expression::Error {
                    span: start,
                    recovery: None,
                }),
            ),
            _ => (
                false,
                String::new(),
                None,
                Expression::Error {
                    span: start,
                    recovery: None,
                },
            ),
        };
        Statement::LetElse {
            is_var,
            name,
            ty,
            value,
            span: join_spans(start, else_block.span),
            else_block,
        }
    }

    /// Parse a checked-arithmetic form: the binding prefix and single operation on
    /// the header line, then the trailing `on out_of_range`/`on zero_divisor` arms.
    /// The parser captures the operation and each arm block; the checker owns which
    /// arms an operation requires and that each arm diverges.
    fn checked_stmt(&mut self) -> Statement {
        let start = self.tokens[self.pos].span;
        let header = self.take_line();
        let checked_index = header
            .iter()
            .position(|token| token.kind == TokenKind::Keyword(Keyword::Checked))
            .expect("checked form detected before dispatch");
        let checked_span = header[checked_index].span;
        let bind = parse_checked_bind(self.source, &header[..checked_index], &mut self.diagnostics);
        let op_tokens = &header[checked_index + 1..];
        let op_error_span = line_span_or(op_tokens, checked_span);
        let op = expr_of_after(self.source, op_tokens, checked_span, &mut self.diagnostics)
            .unwrap_or(Expression::Error {
                span: op_error_span,
                recovery: None,
            });
        let (out_of_range, zero_divisor, end) = self.checked_arms(start);
        Statement::Checked {
            bind,
            op,
            out_of_range,
            zero_divisor,
            span: join_spans(start, end),
        }
    }

    /// Consume the trailing `on <faultkind>` arms of a checked form. Each arm
    /// cuddles the previous arm's `}` or sits on its own line. Returns the two
    /// optional arm blocks (by kind, regardless of source order) and the span of
    /// the last arm consumed. No arm at all is a `CheckedBody` error.
    fn checked_arms(
        &mut self,
        header_start: SourceSpan,
    ) -> (Option<Block>, Option<Block>, SourceSpan) {
        let mut out_of_range = None;
        let mut zero_divisor = None;
        let mut end = header_start;
        let mut saw_arm = false;
        loop {
            let save = self.pos;
            self.skip_newlines();
            if !self.at_word_past_newlines("on") {
                self.pos = save;
                break;
            }
            saw_arm = true;
            if let Some((fault, block)) = self.checked_arm() {
                end = block.span;
                let slot = match fault {
                    CheckedFault::OutOfRange => &mut out_of_range,
                    CheckedFault::ZeroDivisor => &mut zero_divisor,
                };
                if slot.is_some() {
                    self.error_span_reason(
                        block.span,
                        ParseDiagnosticReason::CheckedArm,
                        "this checked arm is already given",
                    );
                } else {
                    *slot = Some(block);
                }
            }
        }
        if !saw_arm {
            self.error_span_reason(
                header_start,
                ParseDiagnosticReason::Expected(ExpectedSyntax::CheckedBody),
                "expected `on out_of_range` / `on zero_divisor` arms",
            );
        }
        (out_of_range, zero_divisor, end)
    }

    /// Parse one checked arm: an `on out_of_range` / `on zero_divisor` header, then
    /// its braced or inline diverging body. A header that is not one of those two
    /// forms is a `CheckedArm` parse error, and its body is skipped so it does not
    /// leak. The cursor is at the `on` identifier.
    fn checked_arm(&mut self) -> Option<(CheckedFault, Block)> {
        let start = self.tokens[self.pos].span;
        let on = self.tokens.get(self.pos);
        let kind = self.tokens.get(self.pos + 1);
        let fault = match (on, kind) {
            (Some(on), Some(kind))
                if on.kind == TokenKind::Identifier
                    && on.text(self.source) == "on"
                    && kind.kind == TokenKind::Identifier =>
            {
                match kind.text(self.source) {
                    "out_of_range" => Some(CheckedFault::OutOfRange),
                    "zero_divisor" => Some(CheckedFault::ZeroDivisor),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(fault) = fault else {
            let line_end = self.find_line_end();
            let span = line_span_or(&self.tokens[self.pos..line_end], start);
            self.error_span_reason(
                span,
                ParseDiagnosticReason::CheckedArm,
                "a checked arm is `on out_of_range` or `on zero_divisor`",
            );
            self.pos = line_end;
            self.skip_block_if_braced();
            return None;
        };
        self.advance(); // `on`
        self.advance(); // fault kind
        let block = self.parse_clause_body();
        Some((fault, block))
    }

    /// Parse the expression that ends the current header line, consuming up to
    /// and including its `NEWLINE`. `keyword` is the header keyword
    /// (`while`/`if`/`match`) already consumed; an empty header reports the
    /// missing expression at the gap just past it, never the start of input.
    /// Returns `None`, after raising a syntax error, when the header does not
    /// parse as a complete expression.
    fn header_expression(&mut self, keyword: SourceSpan) -> Expression {
        let line = self.take_line();
        let error_span = line_span_or(line, keyword);
        let expr = expr_of_after(self.source, line, keyword, &mut self.diagnostics);
        // A failed header reported its own missing-expression diagnostic; the error
        // node stands in for the condition so the statement still parses.
        expr.unwrap_or(Expression::Error {
            span: error_span,
            recovery: None,
        })
    }

    fn if_head(&mut self, keyword: SourceSpan) -> IfHead {
        let line = self.take_line();
        let error_span = line_span_or(line, keyword);
        let starts_const = matches!(
            line.first().map(|token| token.kind),
            Some(TokenKind::Keyword(Keyword::Const))
        );
        // A `const` head is an existence binding; a `const … and …` head (B5) is a
        // chain of bindings and an optional trailing condition; any other head is a
        // condition expression. Each reports its own failure and falls back to
        // `Expression::Error`, so the head is always present.
        let head = if starts_const && top_level_and_starts(line).len() > 1 {
            Some(self.parse_if_const_chain(line))
        } else if starts_const {
            parse_if_const_head(self.source, line, &mut self.diagnostics)
                .map(|(name, ty, value)| IfHead::ConstBinding { name, ty, value })
        } else {
            expr_of_after(self.source, line, keyword, &mut self.diagnostics).map(IfHead::Expr)
        };
        head.unwrap_or(IfHead::Expr(Expression::Error {
            span: error_span,
            recovery: None,
        }))
    }

    /// Parse a B5 `if const` chain head: parts split on top-level `and`, where each
    /// leading `const …` part is an existence binding and the remainder (from the
    /// first non-`const` part) is the trailing condition. Parse-only; the checker
    /// rejects the chain until it is adopted.
    fn parse_if_const_chain(&mut self, line: &[Token]) -> IfHead {
        let starts = top_level_and_starts(line);
        let mut bindings = Vec::new();
        let mut condition_from = None;
        for (index, &start) in starts.iter().enumerate() {
            // The part runs to just before the `and` that opens the next part (that
            // `and` sits one token before the next part's start), or to end of line.
            let part_end = starts.get(index + 1).map_or(line.len(), |next| next - 1);
            let part = &line[start..part_end];
            if part.first().map(|token| token.kind) == Some(TokenKind::Keyword(Keyword::Const)) {
                if let Some((name, ty, value)) =
                    parse_if_const_head(self.source, part, &mut self.diagnostics)
                {
                    bindings.push(IfConstBinding { name, ty, value });
                }
            } else {
                // The first non-`const` part begins the trailing condition; keep the
                // slice from here to the end so a multi-part `cond1 and cond2` rejoins.
                condition_from = Some(start);
                break;
            }
        }
        let condition = condition_from.and_then(|from| {
            // `from` is the start of a part that follows a top-level `and`, so
            // `line[from - 1]` is that `and`: a guaranteed-valid anchor for an empty
            // trailing condition (`... and` with nothing after it).
            let anchor = line[from - 1].span;
            expr_of(self.source, &line[from..], anchor, &mut self.diagnostics)
        });
        IfHead::Chain {
            bindings,
            condition,
        }
    }

    /// Consume the rest of a header line up to and including its `NEWLINE`, or up
    /// to (but not including) a block-opening `{`. Used for headers with no
    /// expression (`transaction`), so any stray tokens before the body do not leak
    /// into the block.
    fn consume_header_line(&mut self) {
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Newline => {
                    self.advance();
                    break;
                }
                TokenKind::LeftBrace | TokenKind::RightBrace => break,
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

    /// Parse the mandatory `{ … }` block that follows a compound-statement header
    /// whose keyword starts at `header_start`. A comment trailing the header is moved
    /// into the block as its first leading comment (see [`detach_header_comment`]). If
    /// no `{` is present (a malformed empty body), returns an empty block; the missing
    /// brace is a formatter/checker concern.
    fn block_body(&mut self, header_start: usize) -> Block {
        let leading = self.detach_header_comment(header_start);
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            let mut block = self.parse_braced_block();
            if let Some(comment) = leading {
                block.comments.insert(0, comment);
            }
            block
        } else {
            // An empty body occupies no source. Anchor a zero-width span at the
            // point a body would start rather than adopting a whole token's span,
            // so the enclosing statement's span does not extend over a following
            // sibling comment or statement and mis-claim it — which would drop that
            // sibling when the block is formatted. The point is the next token's
            // start, or the end of the last consumed token at end of input. (An
            // empty token list would fall back to a zero span, but a body is only
            // parsed after its header keyword was consumed, so tokens is non-empty
            // whenever this runs.)
            let point = match self.tokens.get(self.pos) {
                Some(token) => SourceSpan {
                    end_byte: token.span.start_byte,
                    ..token.span
                },
                None => {
                    let end = self
                        .tokens
                        .get(self.pos.saturating_sub(1))
                        .map(|token| token.span)
                        .unwrap_or_default();
                    SourceSpan {
                        start_byte: end.end_byte,
                        end_byte: end.end_byte,
                        line: end.line,
                        column: end.column,
                    }
                }
            };
            Block {
                statements: Vec::new(),
                comments: leading.into_iter().collect(),
                span: point,
            }
        }
    }

    /// Parse `{ statement* }`, tolerating a missing trailing `}` at the end of the
    /// body token slice. A fresh comment accumulator is swapped in for the duration
    /// so this nested block's comments do not leak into the parent block.
    fn parse_braced_block(&mut self) -> Block {
        // Fail closed past the nesting limit: skip the block's tokens without
        // recursing (the lexer already reported the located nesting-limit finding),
        // so a deep brace nest cannot overflow the stack or grow the AST with depth.
        if self.depth >= crate::NESTING_DEPTH_LIMIT {
            let start = self.tokens[self.pos].span;
            let end = self.skip_block();
            return Block {
                statements: Vec::new(),
                comments: Vec::new(),
                span: join_spans(start, end),
            };
        }
        self.depth += 1;
        let start = self.advance().span; // `{`
        let outer = std::mem::take(&mut self.comments);
        let statements = self.statements();
        let comments = std::mem::replace(&mut self.comments, outer);
        let end = if matches!(self.peek(), Some(TokenKind::RightBrace)) {
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

    /// Parse a trailing-clause body: a `{ … }` block, or a single inline statement
    /// as a one-statement block (the inline diverging form of `else`, `on more`, a
    /// checked arm, or a match arm). Inline-vs-block enforcement is the formatter's.
    fn parse_clause_body(&mut self) -> Block {
        // The body may cuddle the clause keyword (`else return -1`, `else {`) or sit
        // on the next line (`else`\n`{`); skip the separating newlines either way.
        self.skip_newlines();
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.parse_braced_block()
        } else {
            self.inline_statement_block()
        }
    }

    /// Parse one inline statement as a one-statement block. A missing statement
    /// (nothing before the line break) yields an empty block whose span is anchored
    /// at the cursor, so the enclosing statement does not over-claim a sibling.
    fn inline_statement_block(&mut self) -> Block {
        let anchor = self.tokens.get(self.pos).map(|token| SourceSpan {
            end_byte: token.span.start_byte,
            ..token.span
        });
        let outer = std::mem::take(&mut self.comments);
        let statement = if matches!(
            self.peek(),
            None | Some(TokenKind::Newline | TokenKind::RightBrace)
        ) {
            None
        } else {
            self.statement()
        };
        let comments = std::mem::replace(&mut self.comments, outer);
        let span = statement
            .as_ref()
            .map(Statement::span)
            .or(anchor)
            .unwrap_or_default();
        Block {
            statements: statement.into_iter().collect(),
            comments,
            span,
        }
    }

    /// Index of the `NEWLINE` (or layout token) that ends the current line.
    fn find_line_end(&self) -> usize {
        line_end(self.tokens, self.pos)
    }

    /// Report a `{ … }` block where a statement was expected — a bare block has no
    /// statement form. Points at the opening `{`.
    fn report_unexpected_indented_block(&mut self) {
        let token = self.tokens[self.pos];
        let line_start = token.span.start_byte - (token.span.column as usize - 1);
        let span = SourceSpan {
            start_byte: token.span.start_byte,
            end_byte: first_line_end(self.source, line_start),
            line: token.span.line,
            column: token.span.column,
        };
        self.error_span_reason(
            span,
            ParseDiagnosticReason::UnexpectedBlock,
            "unexpected `{`; only compound statements introduce blocks",
        );
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
    /// to the `NEWLINE` or `{`, and any following `{ … }` block — and report the
    /// given diagnostic over the whole span so following statements parse.
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
                TokenKind::LeftBrace | TokenKind::RightBrace => break,
                _ => end = self.advance().span,
            }
        }
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
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

    /// Skip a malformed `{ … }` block, returning the span of the last token
    /// consumed. The cursor is at the opening `{`. On an unmatched leading `}` it
    /// breaks without consuming the token, leaving the enclosing block's close for
    /// the caller instead of swallowing it — `}` is the hard sync anchor.
    fn skip_block(&mut self) -> SourceSpan {
        let mut depth = 0usize;
        let mut end = self.tokens[self.pos].span;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::LeftBrace => {
                    depth += 1;
                    end = self.advance().span;
                }
                TokenKind::RightBrace => {
                    if depth == 0 {
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

    /// Skip a stray `{ … }` block where a statement was expected, retaining its
    /// own-line comments as trivia when the block held no statement tokens. The
    /// cursor is at the opening `{`.
    fn skip_unexpected_indented_block(&mut self) -> SourceSpan {
        let mut depth = 0usize;
        let mut end = self.tokens[self.pos].span;
        let mut line_has_content = false;
        let mut comments = Vec::new();
        let mut has_statement_tokens = false;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::LeftBrace => {
                    depth += 1;
                    line_has_content = false;
                    end = self.advance().span;
                }
                TokenKind::RightBrace => {
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

/// Index of the first top-level `else` keyword (bracket depth 0) in a header line,
/// the B6 let-else separator, or `None` when none is present.
fn find_top_level_else(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace => {
                depth = depth.saturating_sub(1)
            }
            TokenKind::Keyword(Keyword::Else) if depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

/// The start index of each part of a header split on top-level `and` keywords
/// (bracket depth 0). The first part starts at 0; each later part starts just after
/// its separating `and`. A single-element result means no top-level `and`.
fn top_level_and_starts(tokens: &[Token]) -> Vec<usize> {
    let mut starts = vec![0];
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace => {
                depth = depth.saturating_sub(1)
            }
            TokenKind::Keyword(Keyword::And) if depth == 0 => starts.push(index + 1),
            _ => {}
        }
    }
    starts
}

/// Parse the binding prefix of a checked form (everything before `checked`) into a
/// [`CheckedBind`]. The prefix is `return`, or `const`/`var NAME [: TYPE] =`. Stays
/// total: a malformed name or type reports one diagnostic and falls back to an empty
/// name so the statement node is still produced.
fn parse_checked_bind(
    source: &str,
    prefix: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> CheckedBind {
    match prefix.first().map(|token| token.kind) {
        Some(TokenKind::Keyword(Keyword::Return)) => CheckedBind::Return,
        Some(TokenKind::Keyword(Keyword::Var)) => {
            let (name, ty) = parse_checked_binding_name(source, prefix, true, diagnostics);
            CheckedBind::Var { name, ty }
        }
        // `const`, and the detection-guaranteed-unreachable fallback, both bind a
        // fresh const so the node is well-formed.
        _ => {
            let (name, ty) = parse_checked_binding_name(source, prefix, false, diagnostics);
            CheckedBind::Const { name, ty }
        }
    }
}

/// Parse the `NAME [: TYPE]` of a `const`/`var checked` binding prefix (which ends at
/// the binding `=`). Reports a keyword-in-name-position or malformed-type error and
/// falls back to an empty name / no annotation.
fn parse_checked_binding_name(
    source: &str,
    prefix: &[Token],
    is_var: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, Option<TypeExpr>) {
    let name = match prefix.get(1) {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        other => {
            let (span, expected) = (
                other.map_or(prefix[0].span, |token| token.span),
                if is_var {
                    ExpectedSyntax::VariableName
                } else {
                    ExpectedSyntax::ConstName
                },
            );
            diagnostics.push(Diagnostic {
                code: ParseDiagnosticReason::Expected(expected).code(),
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(expected)),
                severity: Severity::Error,
                message: "expected a name for the checked binding".to_string(),
                help: None,
                span,
            });
            String::new()
        }
    };

    let mut ty = None;
    if prefix.get(2).map(|token| token.kind) == Some(TokenKind::Colon) {
        // The type spans from after the colon up to the binding `=` that ends the
        // prefix (types carry no `=`, so the top-level `=` is the binding one).
        let type_start = 3;
        let type_end = find_top_level_equal(prefix).unwrap_or(prefix.len());
        if type_end > type_start {
            let (expected, message) = if is_var {
                (
                    ExpectedSyntax::ParameterType,
                    "expected variable type annotation",
                )
            } else {
                (ExpectedSyntax::ConstType, "expected const type annotation")
            };
            match parse_type(source, &prefix[type_start..type_end], expected, message) {
                Ok(parsed) => ty = Some(parsed),
                Err(error) => {
                    push_parse_error(diagnostics, line_span_or(prefix, prefix[0].span), error)
                }
            }
        }
    }
    (name, ty)
}
