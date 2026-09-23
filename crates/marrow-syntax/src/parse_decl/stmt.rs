//! Recursive-descent parser for a function body over the file-wide token stream. It
//! frames compound statements and their `{ … }` blocks; statements end at a `NEWLINE`
//! or `}`, and a trailing clause (`else`, `on more`, a checked arm, a match arm) takes
//! either a braced block or a single inline statement.

use super::head::arm_pattern;
use super::statement_lines::{
    parse_entry_head, parse_for_header, parse_if_const_head, parse_simple_statement,
};
use super::tokens::{
    comment_from_token, expr_of, expr_of_after, find_top_level_equal, first_line_end,
    is_line_comment, line_end, line_span_or, parse_type, push_parse_error,
};
use crate::ast::{
    ArmBinding, Block, CheckedBind, Comment, CommentMarker, CommentPlacement, ElseIf, Expression,
    IfConstBinding, MatchArm, Statement, TraversalBound, TypeExpr,
};
use crate::diagnostic::{
    DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, SourceSpan,
    SyntaxError, SyntaxSink, UnsupportedSyntax, nesting_limit,
};
use crate::parse_expr::join_spans;
use crate::token::{ContextualKeyword, Keyword, Token, TokenKind};

enum IfHead {
    Expr(Expression),
    ConstBinding {
        name: String,
        name_span: SourceSpan,
        ty: Option<TypeExpr>,
        value: Expression,
    },
    /// `if const a = e1 and const b = e2 and cond` — a chain of existence bindings
    /// joined by `and`, with an optional trailing bare condition.
    Chain {
        bindings: Vec<IfConstBinding>,
        condition: Option<Expression>,
    },
}

/// Which fault a checked arm handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckedFault {
    OutOfRange,
    ZeroDivisor,
}

impl CheckedFault {
    /// Every checked arm, in the order the formatter renders them.
    pub(crate) const ALL: [Self; 2] = [Self::OutOfRange, Self::ZeroDivisor];

    /// The contextual word naming this arm after `on`.
    pub(crate) const fn spelling(self) -> ContextualKeyword {
        match self {
            Self::OutOfRange => ContextualKeyword::OutOfRange,
            Self::ZeroDivisor => ContextualKeyword::ZeroDivisor,
        }
    }
}

/// A block-introducing keyword that has no statement of its own and only appears as a
/// clause of one (`else`). Standing alone it cannot be structured, so the parser
/// swallows it and its nested block, reporting the stray keyword so the following
/// statements still parse. Keywords with dedicated statement parsers are matched first
/// and never reach this guard.
fn is_stray_block_clause_keyword(keyword: Keyword) -> bool {
    matches!(keyword, Keyword::Else)
}

/// Whether an over-deep nest inside a region the parser skips without descending still
/// needs [`crate::NESTING_DEPTH_LIMIT`] reported.
#[derive(Clone, Copy)]
enum SkipReport {
    /// The region is skipped for a grammar reason, so a nest inside it past the limit
    /// is this scan's to report, once.
    Pending,
    /// The region is the refused descent itself, already reported at its own `{`.
    Reported,
}

/// Parses the statements of a function body over a token slice (the tokens
/// strictly inside the enclosing `{ … }`). It frames nested `{ … }` blocks itself
/// and delegates expression parsing to `ExprParser`.
pub(super) struct StmtParser<'a, 'c> {
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
    /// Line comments for the block currently being parsed, in source order.
    /// Each nested block swaps in a fresh accumulator (see `parse_nested_block`)
    /// so a comment lands in the block it appears in.
    comments: Vec<Comment>,
    /// The declaration parser's scoped sink, reborrowed for the body's duration
    /// so a malformed statement line reports directly to the one live collector.
    sink: &'a mut SyntaxSink<'c>,
    /// How many statement bodies deep the descent currently sits, counting the
    /// enclosing function or test body as the first. A braced block and a trailing
    /// clause's single inline statement (`else`\n`if …`, `b => match …`) each cost one
    /// frame, so a nest that never opens a brace is bounded on the same terms as one
    /// that does. Every descent goes through [`StmtParser::descend`], which stops at
    /// [`crate::NESTING_DEPTH_LIMIT`] and is the sole owner of which bodies the tree
    /// holds; a region skipped without descending counts its braces from here on the
    /// same terms.
    depth: usize,
}

impl<'a, 'c> StmtParser<'a, 'c> {
    pub(super) fn new(source: &'a str, tokens: &'a [Token], sink: &'a mut SyntaxSink<'c>) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            comments: Vec::new(),
            sink,
            depth: 1,
        }
    }

    /// Run `body` one statement-body level deeper, or refuse without running it when that
    /// level would pass [`crate::NESTING_DEPTH_LIMIT`]. A refusal is reported here, at
    /// the `{` or inline statement the descent declines to open; the caller only skips
    /// the refused region, so an over-deep nest reports once rather than per level.
    ///
    /// The sole place the descent deepens; a caller that reaches a nested body any other
    /// way is unbounded by construction.
    fn descend<T>(&mut self, body: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if self.depth >= crate::NESTING_DEPTH_LIMIT {
            let span = self.tokens[self.pos].span;
            self.sink.push(nesting_limit(span));
            return None;
        }
        self.depth += 1;
        let value = body(self);
        self.depth -= 1;
        Some(value)
    }

    /// Account one `{` a skip walks into, `depth` braces below the block being parsed:
    /// the first one past the limit is reported unless the region was refused and
    /// reported by the descent already, so a skipped nest and a parsed nest report the
    /// limit at the same brace.
    fn skip_into(&mut self, depth: usize, report: &mut SkipReport) {
        if matches!(report, SkipReport::Pending) && self.depth + depth > crate::NESTING_DEPTH_LIMIT
        {
            let span = self.tokens[self.pos].span;
            self.sink.push(nesting_limit(span));
            *report = SkipReport::Reported;
        }
    }

    pub(super) fn parse_block(mut self) -> (Vec<Statement>, Vec<Comment>) {
        let statements = self.statements();
        (statements, std::mem::take(&mut self.comments))
    }

    /// Record an own-line comment token for the current block and consume its trailing
    /// `NEWLINE`. The doc-comment decision is owned by `classify_line_comment`.
    fn take_own_line_comment(&mut self) {
        let token = self.advance();
        self.record_line_comment(token, CommentPlacement::OwnLine);
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    /// Sole owner of the line-comment decision. A `///` doc comment in statement
    /// position has no declaration to attach to, so it is reported rather than retained:
    /// a swallowed doc comment is one the formatter cannot place, breaking the
    /// check-run-format round trip. An ordinary `//` comment becomes trivia for the
    /// current block and is returned for callers that place it conditionally.
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

    /// Classify a line-comment token and, when it is ordinary trivia, append it to the
    /// current block's comments at `placement`.
    fn record_line_comment(&mut self, token: Token, placement: CommentPlacement) {
        if let Some(comment) = self.classify_line_comment(token, placement) {
            self.comments.push(comment);
        }
    }

    /// Detach a comment trailing this construct's header and hand it back reclassified
    /// as an own-line comment, so the block that follows adopts it as its first leading
    /// comment and the `{`-cuddled, next-line-`{`, and own-line spellings of a header
    /// comment all parse to one tree and format to one fixed point. A trailing comment
    /// on an earlier sibling starts before `header_start` and is left in place.
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
            "a `///` doc comment must precede a declaration, member, or parameter",
        );
    }

    /// Parse statements up to the enclosing `}` or the end of the body. The list is
    /// grown by pushing and kept as built: shrinking it would reallocate, and the
    /// growth slack is part of the published per-source-byte parse charge.
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
    fn at_word_past_newlines(&self, word: ContextualKeyword) -> bool {
        self.tokens[self.pos..]
            .iter()
            .find(|token| token.kind != TokenKind::Newline)
            .is_some_and(|token| token.is_contextual(self.source, word))
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
            TokenKind::Keyword(Keyword::Require) => return Some(self.require_stmt()),
            TokenKind::Keyword(Keyword::Match) => return Some(self.match_stmt()),
            TokenKind::Keyword(Keyword::Assert) => return Some(self.assert_stmt()),
            TokenKind::Keyword(keyword) if is_stray_block_clause_keyword(keyword) => {
                self.skip_compound();
                return None;
            }
            // `throw`/`catch`/`finally` are ordinary identifiers; a statement that
            // begins with one is the removed throw/catch form. Report it as
            // unsupported and point at `Result`, keeping the parse total.
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
        // header token is not a distinguishing keyword. It must be detected on the
        // header line, before the generic line-based simple-statement path, because its
        // `on` arms live on following lines that `take_line` does not see.
        if self.at_checked_form() {
            return Some(self.checked_stmt());
        }

        if self.tokens[self.pos].kind == TokenKind::Keyword(Keyword::Ref) {
            return Some(self.entry_binding_stmt());
        }

        if self.at_let_else() {
            return Some(self.let_else_stmt());
        }

        let start = self.tokens[self.pos].span;
        let line = self.take_line();
        let error_span = line_span_or(line, start);
        let statement = parse_simple_statement(self.source, line, self.sink);
        // Total parsing: a line that did not structure already reported its own
        // diagnostic and becomes an error node carrying its span, so the body is never
        // silently short a statement.
        Some(statement.unwrap_or(Statement::Error { span: error_span }))
    }

    /// Take the current statement or header line: the content tokens up to the token
    /// that ends the line, with any trailing comment recorded as block trivia. A
    /// terminating `NEWLINE` is consumed; a `{`/`}` is left in place for the caller to
    /// frame the body. The returned slice borrows the whole-file token stream, so it
    /// outlives the advance and a caller may parse it after the cursor has moved.
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

    /// If the token just before `line_end` — the `NEWLINE`/`{`/`}`/`EOF` ending the
    /// current line — is a trailing comment, record it as block trivia and return the
    /// index that excludes it; otherwise return `line_end` unchanged.
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
        // The remedy is part of the finding, finalized before submission: no site
        // reaches back into the collector to amend a submitted row.
        let reason = ParseDiagnosticReason::Unsupported(UnsupportedSyntax::LoopLabels);
        self.sink.push(SyntaxError::new(
            DiagnosticReason::Parser(reason),
            "loop labels were removed",
            Some("extract a function and use return to leave nested loops".to_string()),
            join_spans(name.span, colon.span),
        ));
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
                // A trailing `on more` block is consumed whenever it appears, so it
                // never desyncs into a bogus following statement. With `at most` in the
                // head it rides the `TraversalBound`; without one the head is unbounded
                // and the block is dropped. Either way the checker reports the fault.
                let on_more = self.take_on_more_block();
                if let Some(block) = &on_more {
                    end = block.span;
                }
                let bound = bound_head.map(|(limit, from)| {
                    Box::new(TraversalBound {
                        limit,
                        from,
                        on_more,
                    })
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

    /// Consume a trailing `on more` clause: the contextual keywords cuddling the loop
    /// body's `}` or on the next line, then a braced or inline diverging body. Returns
    /// `None` and restores the cursor when the next tokens are not that phrase, so a
    /// following sibling statement parses.
    fn take_on_more_block(&mut self) -> Option<Block> {
        let save = self.pos;
        self.skip_newlines();
        let is_on = self
            .tokens
            .get(self.pos)
            .is_some_and(|token| token.is_contextual(self.source, ContextualKeyword::On));
        let is_more = self
            .tokens
            .get(self.pos + 1)
            .is_some_and(|token| token.is_contextual(self.source, ContextualKeyword::More));
        if !(is_on && is_more) {
            self.pos = save;
            return None;
        }
        self.advance(); // `on`
        self.advance(); // `more`
        Some(self.parse_clause_body())
    }

    /// Parse a statement that begins with `try`. Prefix `try <expr>` propagates a
    /// `Result<T, E>`'s `err` out of the enclosing `Result`-returning function. The
    /// removed block form (`try` opening a body, with `catch`/`finally`) is reported as
    /// unsupported and its blocks are skipped so the parse stays total.
    fn try_statement(&mut self) -> Option<Statement> {
        let start = self.advance().span; // `try`
        let header = self.take_line();
        // The removed block form opens a `{ … }` body after the header.
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.skip_block(SkipReport::Pending);
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
        let inner =
            expr_of_after(self.source, header, start, self.sink).unwrap_or(Expression::Error {
                span: error_span,
                recovery: None,
            });
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
                self.skip_block(SkipReport::Pending);
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
            self.skip_block(SkipReport::Pending);
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

        // A trailing `else`/`else if` cuddles the then-block's `}` or sits on the next
        // line. Restore the cursor when no `else` follows, so the newline still ends
        // the statement.
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
            IfHead::ConstBinding {
                name,
                name_span,
                ty,
                value,
            } => Statement::IfConst {
                name,
                name_span,
                ty: ty.map(Box::new),
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
    /// the pattern a member path relative to the scrutinee enum with optional payload
    /// bindings. Exhaustiveness and member validity are checker rules.
    fn match_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `match`
        let scrutinee = self.header_expression(start);
        // A comment trailing the `match` header becomes an own-line comment leading the
        // first arm, the one owner `match` arms share.
        self.own_header_comment_in_place(start.start_byte);
        let (arms, end) = self.match_body();
        Statement::Match {
            scrutinee,
            arms,
            span: join_spans(start, end),
        }
    }

    /// Parse a `match` statement's `{ <arms> }` and return its arms and closing span.
    /// A match body is a brace-delimited region like any other, so it costs the same
    /// frame a block does.
    fn match_body(&mut self) -> (Vec<MatchArm>, SourceSpan) {
        if !matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            let gap = self.gap();
            self.report_missing_block(gap);
            return (Vec::new(), gap);
        }
        self.descend(Self::match_arms)
            .unwrap_or_else(|| (Vec::new(), self.skipped_block().span))
    }

    /// Parse the arms of a match body. Runs one level inside [`StmtParser::descend`];
    /// the cursor is at the `{`.
    fn match_arms(&mut self) -> (Vec<MatchArm>, SourceSpan) {
        let mut end = self.advance().span; // `{`
        let mut arms = Vec::new();
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
                    self.skip_block(SkipReport::Pending);
                }
                _ => {
                    if let Some(arm) = self.match_arm() {
                        end = arm.block.span;
                        arms.push(arm);
                    }
                }
            }
        }
        (arms, end)
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
        debug_assert_eq!(
            pattern.path.capacity(),
            pattern.path.len(),
            "an arm path boxes exactly"
        );
        Some(MatchArm {
            path: pattern.path.into_boxed_slice(),
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
            self.skip_block(SkipReport::Pending);
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

    fn entry_binding_stmt(&mut self) -> Statement {
        let start = self.tokens[self.pos].span;
        let line_end = self.find_line_end();
        let Some(offset) = find_top_level_else(&self.tokens[self.pos..line_end]) else {
            let line = self.take_line();
            let span = line_span_or(line, start);
            self.error_span_reason(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::Statement),
                "an entry reference requires `ref name = address else { ... }`",
            );
            return Statement::Error { span };
        };
        let binding_end = self.pos + offset;
        let head = parse_entry_head(self.source, &self.tokens[self.pos..binding_end], self.sink);
        self.pos = binding_end + 1;
        let else_block = self.parse_clause_body();
        let span = join_spans(start, else_block.span);
        let Some((name, name_span, address)) = head else {
            return Statement::Error { span };
        };
        Statement::EntryBinding {
            name,
            name_span,
            address,
            else_block,
            span,
        }
    }

    /// Whether the current line is a let-else: a `const`/`var` binding whose header line
    /// carries a top-level `else` diverging tail. Inspects the line without consuming it.
    fn at_let_else(&self) -> bool {
        let line = &self.tokens[self.pos..self.find_line_end()];
        let is_binding = matches!(
            line.first().map(|token| token.kind),
            Some(TokenKind::Keyword(Keyword::Const | Keyword::Var))
        );
        is_binding && find_top_level_else(line).is_some()
    }

    /// Parse a let-else: `const`/`var name [: ty] = value else <diverging>`. The binding
    /// before `else` is parsed by the simple-statement parser; the tail is a braced or
    /// inline diverging body.
    fn let_else_stmt(&mut self) -> Statement {
        let start = self.tokens[self.pos].span;
        let line_end = self.find_line_end();
        let else_offset = find_top_level_else(&self.tokens[self.pos..line_end])
            .expect("let-else detected before dispatch");
        let binding_end = self.pos + else_offset;
        let binding =
            parse_simple_statement(self.source, &self.tokens[self.pos..binding_end], self.sink);
        self.pos = binding_end + 1; // past the `else`
        let else_block = self.parse_clause_body();
        let (is_var, name, name_span, ty, value) = match binding {
            Some(Statement::Const {
                name,
                name_span,
                ty,
                value,
                ..
            }) => (false, name, name_span, ty, value),
            Some(Statement::Var {
                name,
                name_span,
                ty,
                value,
                ..
            }) => (
                true,
                name,
                name_span,
                ty,
                value.unwrap_or(Expression::Error {
                    span: start,
                    recovery: None,
                }),
            ),
            _ => (
                false,
                String::new(),
                start,
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
            name_span,
            ty,
            value,
            span: join_spans(start, else_block.span),
            else_block,
        }
    }

    /// Parse `require <condition> else <value>`: a bool condition up to the first
    /// top-level `else`, then the bare failure value running to the end of the line. The
    /// value is an expression, never a statement — the `err(...)` wrap and the return
    /// are implicit — and the checker types it against the function's `Result` error.
    fn require_stmt(&mut self) -> Statement {
        let keyword = self.advance(); // `require`
        let line = self.take_line();
        let error_span = line_span_or(line, keyword.span);
        let Some(else_offset) = find_top_level_else(line) else {
            self.error_span_reason(
                error_span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::RequireElse),
                "a `require` guard is `require <condition> else <value>`",
            );
            return Statement::Error { span: error_span };
        };
        let condition = expr_of_after(self.source, &line[..else_offset], keyword.span, self.sink)
            .unwrap_or(Expression::Error {
                span: line_span_or(&line[..else_offset], keyword.span),
                recovery: None,
            });
        let else_span = line[else_offset].span;
        let value = expr_of_after(self.source, &line[else_offset + 1..], else_span, self.sink)
            .unwrap_or(Expression::Error {
                span: line_span_or(&line[else_offset + 1..], else_span),
                recovery: None,
            });
        let span = join_spans(keyword.span, value.span());
        Statement::Require {
            condition,
            value,
            span,
        }
    }

    /// Parse a checked-arithmetic form: the binding prefix and single operation on the
    /// header line, then the trailing `on out_of_range`/`on zero_divisor` arms. The
    /// checker owns which arms an operation requires and that each arm diverges.
    fn checked_stmt(&mut self) -> Statement {
        let start = self.tokens[self.pos].span;
        let header = self.take_line();
        let checked_index = header
            .iter()
            .position(|token| token.kind == TokenKind::Keyword(Keyword::Checked))
            .expect("checked form detected before dispatch");
        let checked_span = header[checked_index].span;
        let bind = parse_checked_bind(self.source, &header[..checked_index], self.sink);
        let op_tokens = &header[checked_index + 1..];
        let op_error_span = line_span_or(op_tokens, checked_span);
        let op = expr_of_after(self.source, op_tokens, checked_span, self.sink).unwrap_or(
            Expression::Error {
                span: op_error_span,
                recovery: None,
            },
        );
        let (out_of_range, zero_divisor, end) = self.checked_arms(start);
        Statement::Checked {
            bind,
            op,
            out_of_range,
            zero_divisor,
            span: join_spans(start, end),
        }
    }

    /// Consume the trailing `on <faultkind>` arms of a checked form, each cuddling the
    /// previous arm's `}` or on its own line. Returns the arm blocks by kind, regardless
    /// of source order, and the last arm's span. No arm at all is a `CheckedBody` error.
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
            if !self.at_word_past_newlines(ContextualKeyword::On) {
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

    /// Parse one checked arm: an `on out_of_range` / `on zero_divisor` header, then its
    /// braced or inline diverging body. Any other header is a `CheckedArm` parse error
    /// whose body is skipped so it does not leak. The cursor is at the `on` identifier.
    fn checked_arm(&mut self) -> Option<(CheckedFault, Block)> {
        let start = self.tokens[self.pos].span;
        let on = self.tokens.get(self.pos);
        let kind = self.tokens.get(self.pos + 1);
        let fault = match (on, kind) {
            (Some(on), Some(kind))
                if on.is_contextual(self.source, ContextualKeyword::On)
                    && kind.kind == TokenKind::Identifier =>
            {
                CheckedFault::ALL
                    .into_iter()
                    .find(|fault| kind.is_contextual(self.source, fault.spelling()))
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

    /// Parse the expression that ends the current header line, consuming up to and
    /// including its `NEWLINE`. `keyword` is the already-consumed header keyword; an
    /// empty header reports the missing expression at the gap just past it, never at
    /// the start of input.
    fn header_expression(&mut self, keyword: SourceSpan) -> Expression {
        let line = self.take_line();
        let error_span = line_span_or(line, keyword);
        let expr = expr_of_after(self.source, line, keyword, self.sink);
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
        // A `const` head is an existence binding; a `const … and …` head is a chain of
        // bindings and an optional trailing condition; any other head is a condition
        // expression. Each reports its own failure and falls back to `Expression::Error`,
        // so the head is always present.
        let head = if starts_const && top_level_and_starts(line).len() > 1 {
            Some(self.parse_if_const_chain(line))
        } else if starts_const {
            parse_if_const_head(self.source, line, self.sink).map(|(name, name_span, ty, value)| {
                IfHead::ConstBinding {
                    name,
                    name_span,
                    ty,
                    value,
                }
            })
        } else {
            expr_of_after(self.source, line, keyword, self.sink).map(IfHead::Expr)
        };
        head.unwrap_or(IfHead::Expr(Expression::Error {
            span: error_span,
            recovery: None,
        }))
    }

    /// Parse an `if const` chain head: parts split on top-level `and`, where each
    /// leading `const …` part is an existence binding and the remainder (from the first
    /// non-`const` part) is the trailing condition. Parse-only; the checker rejects the
    /// chain until it is adopted.
    fn parse_if_const_chain(&mut self, line: &[Token]) -> IfHead {
        let starts = top_level_and_starts(line);
        let mut bindings = Vec::new();
        let mut condition_from = None;
        for (index, &start) in starts.iter().enumerate() {
            // The part runs to just before the `and` opening the next part — which sits
            // one token before that part's start — or to end of line.
            let part_end = starts.get(index + 1).map_or(line.len(), |next| next - 1);
            let part = &line[start..part_end];
            if part.first().map(|token| token.kind) == Some(TokenKind::Keyword(Keyword::Const)) {
                if let Some((name, name_span, ty, value)) =
                    parse_if_const_head(self.source, part, self.sink)
                {
                    bindings.push(IfConstBinding {
                        name,
                        name_span,
                        ty,
                        value,
                    });
                }
            } else {
                // Keep the slice from the first non-`const` part to the end, so a
                // multi-part `cond1 and cond2` rejoins as one condition.
                condition_from = Some(start);
                break;
            }
        }
        let condition = condition_from.and_then(|from| {
            // `from` is the start of a part that follows a top-level `and`, so
            // `line[from - 1]` is that `and`: a guaranteed-valid anchor for an empty
            // trailing condition (`... and` with nothing after it).
            let anchor = line[from - 1].span;
            expr_of(self.source, &line[from..], anchor, self.sink)
        });
        IfHead::Chain {
            bindings,
            condition,
        }
    }

    /// Consume the rest of a header line up to and including its `NEWLINE`, or up to but
    /// not including a block-opening `{`. For headers with no expression
    /// (`transaction`), so stray tokens before the body do not leak into the block.
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

    /// Parse the mandatory `{ … }` block following a compound-statement header whose
    /// keyword starts at `header_start`. A comment trailing the header moves into the
    /// block as its first leading comment. A missing `{` is reported at the gap the
    /// block would open at, and an empty block stands there so the statements that
    /// follow still parse as siblings.
    fn block_body(&mut self, header_start: usize) -> Block {
        let leading = self.detach_header_comment(header_start);
        if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            let mut block = self.parse_braced_block();
            if let Some(comment) = leading {
                block.comments.insert(0, comment);
            }
            block
        } else {
            let point = self.gap();
            self.report_missing_block(point);
            Block {
                statements: Vec::new(),
                comments: leading.into_iter().collect(),
                span: point,
            }
        }
    }

    /// The zero-width span where a block would open: the next token's start, or the
    /// end of the last consumed token at end of input. An empty body occupies no
    /// source, so it is anchored here rather than on a whole token: otherwise the
    /// enclosing statement's span would extend over a following sibling comment or
    /// statement and mis-claim it, dropping that sibling when the block is formatted.
    fn gap(&self) -> SourceSpan {
        match self.tokens.get(self.pos) {
            Some(token) => SourceSpan {
                end_byte: token.span.start_byte,
                ..token.span
            },
            // A trailing `NEWLINE` anchors at its own start, the end of its line; any
            // other last token anchors just past its end, on the same line.
            None => match self.tokens.get(self.pos.saturating_sub(1)) {
                Some(token) if token.kind == TokenKind::Newline => SourceSpan {
                    end_byte: token.span.start_byte,
                    ..token.span
                },
                Some(token) => {
                    let width = (token.span.end_byte - token.span.start_byte) as u32;
                    SourceSpan {
                        start_byte: token.span.end_byte,
                        end_byte: token.span.end_byte,
                        line: token.span.line,
                        column: token.span.column + width,
                    }
                }
                None => SourceSpan::default(),
            },
        }
    }

    fn report_missing_block(&mut self, gap: SourceSpan) {
        self.error_span_reason(
            gap,
            ParseDiagnosticReason::Expected(ExpectedSyntax::Block),
            "expected a `{ … }` block",
        );
    }

    /// Parse `{ statement* }`, tolerating a missing trailing `}` at the end of the
    /// body token slice. A fresh comment accumulator is swapped in for the duration
    /// so this nested block's comments do not leak into the parent block.
    fn parse_braced_block(&mut self) -> Block {
        self.descend(Self::braced_block)
            .unwrap_or_else(|| self.skipped_block())
    }

    /// Parse `{ statement* }` at the `{` under the cursor. Runs one level inside
    /// [`StmtParser::descend`].
    fn braced_block(&mut self) -> Block {
        let start = self.advance().span; // `{`
        let outer = std::mem::take(&mut self.comments);
        let statements = self.statements();
        let comments = std::mem::replace(&mut self.comments, outer);
        let end = if matches!(self.peek(), Some(TokenKind::RightBrace)) {
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

    /// Consume a `{ … }` the parser refuses to structure and stand an empty block in its
    /// place, so the refusal costs the enclosing list one statement and the body's
    /// remaining tokens still parse. The cursor is at the opening `{`.
    fn skipped_block(&mut self) -> Block {
        let start = self.tokens[self.pos].span;
        let end = self.skip_block(SkipReport::Reported);
        Block {
            statements: Vec::new(),
            comments: Vec::new(),
            span: join_spans(start, end),
        }
    }

    /// Parse a trailing-clause body: a `{ … }` block, or a single inline statement
    /// as a one-statement block (the inline diverging form of `else`, `on more`, a
    /// checked arm, or a match arm), which the formatter writes as a block.
    fn parse_clause_body(&mut self) -> Block {
        // Comments between a clause keyword and its body belong to that body,
        // regardless of whether the opening brace cuddles the keyword.
        let mut leading = Vec::new();
        loop {
            self.skip_newlines();
            if !self.peek().is_some_and(is_line_comment) {
                break;
            }
            let token = self.advance();
            if let Some(comment) = self.classify_line_comment(token, CommentPlacement::OwnLine) {
                leading.push(comment);
            }
        }
        let mut block = if matches!(self.peek(), Some(TokenKind::LeftBrace)) {
            self.parse_braced_block()
        } else {
            self.inline_statement_block()
        };
        block.comments.splice(0..0, leading);
        block
    }

    /// Parse one inline statement as a one-statement block. A clause with neither a
    /// block nor a statement is reported at the gap, and an empty block anchored there
    /// stands in so the enclosing statement does not over-claim a sibling.
    ///
    /// This is the descent that opens no brace, so the frame bound is the only thing
    /// standing between it and the native stack. Past the limit the clause is left
    /// unstructured and its tokens are not consumed, so the statements that follow are
    /// structured as siblings of the enclosing body rather than its descendants: the
    /// parse stays total and terminating while the tree stays bounded.
    fn inline_statement_block(&mut self) -> Block {
        let anchor = self.gap();
        let outer = std::mem::take(&mut self.comments);
        let statement = if matches!(
            self.peek(),
            None | Some(TokenKind::Newline | TokenKind::RightBrace)
        ) {
            self.report_missing_block(anchor);
            None
        } else {
            self.descend(Self::statement).flatten()
        };
        let comments = std::mem::replace(&mut self.comments, outer);
        let span = statement.as_ref().map_or(anchor, Statement::span);
        // Pushed like every other statement list, so one growth rule describes them all.
        let mut statements = Vec::new();
        statements.extend(statement);
        Block {
            statements,
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
            end = self.skip_block(SkipReport::Pending);
        }
        self.error_span_reason(join_spans(start, end), reason, message);
    }

    fn error_span_reason(
        &mut self,
        span: SourceSpan,
        reason: ParseDiagnosticReason,
        message: impl Into<String>,
    ) {
        self.sink.push(SyntaxError::new(
            DiagnosticReason::Parser(reason),
            message,
            None,
            span,
        ));
    }

    /// Skip a malformed `{ … }` block, returning the span of the last token
    /// consumed. The cursor is at the opening `{`. On an unmatched leading `}` it
    /// breaks without consuming the token, leaving the enclosing block's close for
    /// the caller instead of swallowing it — `}` is the hard sync anchor.
    fn skip_block(&mut self, mut report: SkipReport) -> SourceSpan {
        let mut depth = 0usize;
        let mut end = self.tokens[self.pos].span;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::LeftBrace => {
                    depth += 1;
                    self.skip_into(depth, &mut report);
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
        let mut report = SkipReport::Pending;
        let mut end = self.tokens[self.pos].span;
        let mut line_has_content = false;
        let mut comments = Vec::new();
        let mut has_statement_tokens = false;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::LeftBrace => {
                    depth += 1;
                    self.skip_into(depth, &mut report);
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

/// Index of the first top-level `else` keyword (bracket depth 0) in a header line — the
/// let-else and `require` separator — or `None` when none is present.
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
fn parse_checked_bind(source: &str, prefix: &[Token], sink: &mut SyntaxSink<'_>) -> CheckedBind {
    match prefix.first().map(|token| token.kind) {
        Some(TokenKind::Keyword(Keyword::Return)) => CheckedBind::Return,
        Some(TokenKind::Keyword(Keyword::Var)) => {
            let (name, name_span, ty) = parse_checked_binding_name(source, prefix, true, sink);
            CheckedBind::Var {
                name,
                name_span,
                ty: ty.map(Box::new),
            }
        }
        // `const`, and the fallback detection makes unreachable, both bind a fresh
        // const so the node is well-formed.
        _ => {
            let (name, name_span, ty) = parse_checked_binding_name(source, prefix, false, sink);
            CheckedBind::Const {
                name,
                name_span,
                ty: ty.map(Box::new),
            }
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
    sink: &mut SyntaxSink<'_>,
) -> (String, SourceSpan, Option<TypeExpr>) {
    let (name, name_span) = match prefix.get(1) {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        other => {
            let (span, expected) = (
                other.map_or(prefix[0].span, |token| token.span),
                if is_var {
                    ExpectedSyntax::VariableName
                } else {
                    ExpectedSyntax::ConstName
                },
            );
            let reason = ParseDiagnosticReason::Expected(expected);
            sink.push(SyntaxError::new(
                DiagnosticReason::Parser(reason),
                "expected a name for the checked binding",
                None,
                span,
            ));
            (String::new(), span)
        }
    };

    let mut ty = None;
    if prefix.get(2).map(|token| token.kind) == Some(TokenKind::Colon) {
        // The type runs from after the colon to the binding `=`: types carry no `=`,
        // so the top-level one is the binding's.
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
                Err(error) => push_parse_error(sink, line_span_or(prefix, prefix[0].span), error),
            }
        }
    }
    (name, name_span, ty)
}
