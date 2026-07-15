//! The expression parser: a recursive-descent parser for a single Marrow
//! expression over a token slice, covering primary, postfix, unary, and binary
//! precedence levels including calls and saved paths.
//!
//! Parsing is total: every entry yields an [`Expression`], and a failure yields
//! [`Expression::Error`] carrying the span it could not structure. One diagnostic
//! is reported at the failure token; a failed sub-expression collapses to that
//! error node as it unwinds, so no ancestor reports a second, cascading
//! diagnostic on top of it.

use crate::token::is_trivia;
use crate::{
    Argument, BinaryOp, CompoundAssignOp, Diagnostic, DiagnosticReason, ExpectedSyntax, Expression,
    InterpolationPart, Keyword, LiteralKind, NESTING_DEPTH_LIMIT, NESTING_LIMIT, PARSE_SYNTAX,
    ParseDiagnosticReason, Severity, SourceSpan, Token, TokenKind, UnaryOp, UnsupportedSyntax,
    is_expression_callable_keyword, is_expression_path_segment_keyword,
};

/// The remedy shared by the comparison and equality non-associative levels: the
/// spec directs the author to parenthesize when comparing boolean results. It
/// rides in the diagnostic message so it survives the checker's parse-diagnostic
/// lowering and renders in `marrow check`, where `help` is dropped.
const COMPARE_NONASSOC_REMEDY: &str = "use parentheses to compare boolean results";

/// The outcome of parsing a token slice as one complete expression.
pub(crate) enum ParseComplete {
    /// The whole slice parsed as one expression.
    Complete(Expression),
    /// The slice did not parse; a single diagnostic was reported at the failure
    /// token and the parsed value is [`Expression::Error`].
    Reported,
    /// A complete expression is followed by tokens that are not part of it. No
    /// diagnostic was reported; the span names the first trailing token so the
    /// caller reports the failure in its own context (a statement, a header).
    Incomplete(SourceSpan),
}

pub(crate) struct ExprParser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    /// How deep the recursive descent currently is. Each parenthesized,
    /// unary-operand, or interpolated sub-expression descends one level;
    /// exceeding [`NESTING_DEPTH_LIMIT`] stops the recursion with a located
    /// [`NESTING_LIMIT`] error before the native stack can overflow.
    depth: usize,
    /// The zero-width position to report a missing operand when nothing has been
    /// consumed yet — just past the `=`/keyword/operator the caller stripped, or
    /// just before the `=` of an empty assignment target. An empty slice has no
    /// consumed token to anchor to; this keeps the diagnostic on a real 1-based
    /// position rather than the line-0 default.
    gap: SourceSpan,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> ExprParser<'a> {
    /// Build a parser whose missing-leading-operand diagnostics anchor at `gap`.
    pub(crate) fn new(source: &'a str, tokens: &[Token], gap: SourceSpan) -> Self {
        let tokens = tokens
            .iter()
            .copied()
            .filter(|token| !is_trivia(token.kind))
            .collect();
        Self {
            source,
            tokens,
            pos: 0,
            depth: 0,
            gap,
            diagnostics: Vec::new(),
        }
    }

    /// Parse the whole slice as one expression, draining diagnostics into the
    /// caller's vec and classifying the outcome.
    pub(crate) fn parse_complete(mut self, diagnostics: &mut Vec<Diagnostic>) -> ParseComplete {
        let expr = self.expression();
        let result = if expr.is_error() {
            ParseComplete::Reported
        } else if self.report_stray_assignment_operator() {
            // A `=` left where the expression should have ended is the `=`-for-`==`
            // mistake, and a trailing compound-assign operator is a misplaced
            // assignment; either way this owns the single diagnostic for it.
            ParseComplete::Reported
        } else if self.pos < self.tokens.len() {
            ParseComplete::Incomplete(self.tokens[self.pos].span)
        } else {
            ParseComplete::Complete(expr)
        };
        diagnostics.append(&mut self.diagnostics);
        result
    }

    /// If the next unconsumed token begins a stray assignment operator in
    /// expression position, report it at that operator and return `true`. A bare
    /// `=` is the `=`-vs-`==` mistake, and a compound-assign operator (`+=`, `-=`,
    /// …) is a chained or misplaced assignment, which does not chain and is not an
    /// expression. Reporting here lands the diagnostic on the operator rather than
    /// bubbling to a generic failure at the statement keyword.
    fn report_stray_assignment_operator(&mut self) -> bool {
        let Some(token) = self.tokens.get(self.pos).copied() else {
            return false;
        };
        if token.kind == TokenKind::Equal {
            self.error(
                token.span,
                ParseDiagnosticReason::EqualsInExpression,
                "`=` is assignment, not equality; use `==` for equality".to_string(),
                None,
            );
            return true;
        }
        if let Some(op) = CompoundAssignOp::from_operator_token(token.kind) {
            self.error(
                token.span,
                ParseDiagnosticReason::CompoundAssignInExpression,
                format!(
                    "`{}` is assignment, not an expression; assignment does not chain",
                    op.symbol()
                ),
                None,
            );
            return true;
        }
        false
    }

    fn error(
        &mut self,
        span: SourceSpan,
        reason: ParseDiagnosticReason,
        message: String,
        help: Option<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            reason: DiagnosticReason::Parser(reason),
            severity: Severity::Error,
            message,
            help,
            span,
        });
    }

    /// Report a diagnostic at `span` and return the error node for it, the single
    /// place a leaf turns a failure into a reported [`Expression::Error`].
    fn error_expr(
        &mut self,
        span: SourceSpan,
        reason: ParseDiagnosticReason,
        message: String,
        help: Option<String>,
    ) -> Expression {
        self.error(span, reason, message, help);
        Expression::Error { span }
    }

    /// The zero-width position where a missing operand should be reported: just
    /// past the last consumed token, or the caller's `gap` anchor when nothing has
    /// been consumed. Always a valid 1-based position.
    fn gap_span(&self) -> SourceSpan {
        match self.pos.checked_sub(1) {
            Some(prev) => {
                let end = self.tokens[prev].span;
                SourceSpan {
                    start_byte: end.end_byte,
                    end_byte: end.end_byte,
                    line: end.line,
                    column: end.column,
                }
            }
            None => self.gap,
        }
    }

    fn peek(&self) -> Option<TokenKind> {
        self.peek_at(0)
    }

    fn peek_at(&self, offset: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + offset).map(|token| token.kind)
    }

    fn peek_is_contextual(&self, text: &str) -> bool {
        self.tokens.get(self.pos).is_some_and(|token| {
            token.kind == TokenKind::Identifier && token.text(self.source) == text
        })
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos];
        self.pos += 1;
        token
    }

    fn expression(&mut self) -> Expression {
        self.descend(Self::or_expr)
    }

    /// Run one recursive-descent level, bounding nesting depth. The binary
    /// precedence tower (`or_expr` … `unary_expr`) is a fixed-height chain, so a
    /// single `expression` call descends one logical level; the recursive entries
    /// — a parenthesized sub-expression, a unary operand, and an interpolated
    /// expression — each route through here, so deeply nested source stops with a
    /// located [`NESTING_LIMIT`] error at the offending token rather than
    /// overflowing the stack. The counter is decremented on the way back up so a
    /// wide-but-shallow expression is never penalized.
    fn descend(&mut self, parse: impl FnOnce(&mut Self) -> Expression) -> Expression {
        self.depth += 1;
        if self.depth > NESTING_DEPTH_LIMIT {
            self.depth -= 1;
            return self.nesting_limit_error();
        }
        let result = parse(self);
        self.depth -= 1;
        result
    }

    /// Account for one more level of left-associated nesting built by a binary
    /// operator chain or a postfix field/call chain. A flat `1 + 1 + …` line or
    /// `a.f.f…` chain builds an AST as deep as it is long without recursing here,
    /// so each accumulation step must count toward the same limit as a
    /// parenthesis. Returns `false` once the limit is crossed so the chain stops
    /// growing; the caller unwinds the levels it added with [`Self::leave_chain`]
    /// and reports the overflow.
    fn enter_chain_level(&mut self) -> bool {
        self.depth += 1;
        if self.depth > NESTING_DEPTH_LIMIT {
            self.depth -= 1;
            return false;
        }
        true
    }

    fn leave_chain(&mut self, levels: usize) {
        self.depth -= levels;
    }

    /// Report the nesting-limit overflow at the next unconsumed token (the deepest
    /// construct the parser reached), or at end of input when none remains, and
    /// return the error node for it.
    fn nesting_limit_error(&mut self) -> Expression {
        // Anchor at the next unconsumed token; at end of input fall back to the last
        // real token (a paren bomb consumes everything before hitting the limit).
        // A 1-based `(1, 1)` guards the theoretical empty-stream case so the reported
        // span is never the all-zero default, which violates the 1-based invariant.
        let span = self
            .tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .map_or(
                SourceSpan {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
                |token| token.span,
            );
        self.diagnostics.push(Diagnostic {
            code: NESTING_LIMIT,
            reason: DiagnosticReason::Parser(ParseDiagnosticReason::NestingLimit),
            severity: Severity::Error,
            message: format!("expression nests deeper than the limit of {NESTING_DEPTH_LIMIT}"),
            help: None,
            span,
        });
        Expression::Error { span }
    }

    /// Parse a left-associated chain of one operator precedence: an `operand`, then
    /// each `operator operand` repetition the operator table accepts. Each
    /// repetition deepens the AST by one node, so it counts toward the nesting
    /// limit; the levels added are unwound once the chain is complete.
    fn binary_chain(
        &mut self,
        operand: impl Fn(&mut Self) -> Expression,
        operator: impl Fn(TokenKind) -> Option<BinaryOp>,
    ) -> Expression {
        let mut left = operand(self);
        if left.is_error() {
            return left;
        }
        let mut levels = 0;
        while let Some(op) = self.peek().and_then(&operator) {
            if !self.enter_chain_level() {
                self.leave_chain(levels);
                return self.nesting_limit_error();
            }
            levels += 1;
            self.advance();
            let right = operand(self);
            if right.is_error() {
                self.leave_chain(levels);
                return right;
            }
            left = binary_expr(op, left, right);
        }
        self.leave_chain(levels);
        left
    }

    fn or_expr(&mut self) -> Expression {
        self.binary_chain(Self::and_expr, |kind| {
            matches!(kind, TokenKind::Keyword(Keyword::Or)).then_some(BinaryOp::Or)
        })
    }

    fn and_expr(&mut self) -> Expression {
        self.binary_chain(Self::is_expr, |kind| {
            matches!(kind, TokenKind::Keyword(Keyword::And)).then_some(BinaryOp::And)
        })
    }

    /// `is` sits one level looser than equality and tighter than `and`, on its own
    /// non-associative level: a single `is`, never chained (`a is X is Y` is
    /// rejected). The right operand is parsed as an equality-level expression; any
    /// narrower member-path restriction is enforced by the checker.
    fn is_expr(&mut self) -> Expression {
        let left = self.equality_expr();
        if left.is_error() || !matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Is))) {
            return left;
        }
        self.advance();
        let right = self.equality_expr();
        if right.is_error() {
            return right;
        }
        if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Is))) {
            return self.reject_chained_operator(
                "is",
                "each `is` yields a bool, so join the subtree tests with `and`/`or`",
            );
        }
        binary_expr(BinaryOp::Is, left, right)
    }

    fn equality_expr(&mut self) -> Expression {
        let left = self.comparison_expr();
        if left.is_error() {
            return left;
        }
        let (op, text) = match self.peek() {
            Some(TokenKind::EqualEqual) => (BinaryOp::Equal, "=="),
            Some(TokenKind::BangEqual) => (BinaryOp::NotEqual, "!="),
            _ => return left,
        };
        self.advance();
        let right = self.comparison_expr();
        if right.is_error() {
            return right;
        }
        if matches!(
            self.peek(),
            Some(TokenKind::EqualEqual | TokenKind::BangEqual)
        ) {
            return self.reject_chained_operator(text, COMPARE_NONASSOC_REMEDY);
        }
        binary_expr(op, left, right)
    }

    fn comparison_expr(&mut self) -> Expression {
        let left = self.range_expr();
        if left.is_error() {
            return left;
        }
        let (op, text) = match self.peek() {
            Some(TokenKind::Less) => (BinaryOp::Less, "<"),
            Some(TokenKind::LessEqual) => (BinaryOp::LessEqual, "<="),
            Some(TokenKind::Greater) => (BinaryOp::Greater, ">"),
            Some(TokenKind::GreaterEqual) => (BinaryOp::GreaterEqual, ">="),
            _ => return left,
        };
        self.advance();
        let right = self.range_expr();
        if right.is_error() {
            return right;
        }
        if matches!(
            self.peek(),
            Some(
                TokenKind::Less
                    | TokenKind::LessEqual
                    | TokenKind::Greater
                    | TokenKind::GreaterEqual
            )
        ) {
            return self.reject_chained_operator(text, COMPARE_NONASSOC_REMEDY);
        }
        binary_expr(op, left, right)
    }

    /// Report a second operator on a non-associative level (`==`/`!=`, a
    /// comparison, or `is`) at that operator's own token and abort the expression.
    /// `text` is the operator spelling and `remedy` the spec fix; both ride in the
    /// message so the remedy survives the checker's parse-diagnostic lowering and
    /// renders in `marrow check`, where the `help` field is dropped.
    fn reject_chained_operator(&mut self, text: &str, remedy: &str) -> Expression {
        let operator = self.tokens[self.pos];
        self.error_expr(
            operator.span,
            ParseDiagnosticReason::NonAssociativeOperator,
            format!("`{text}` does not chain; {remedy}"),
            None,
        )
    }

    fn range_expr(&mut self) -> Expression {
        if matches!(
            self.peek(),
            Some(TokenKind::DotDot | TokenKind::DotDotEqual)
        ) {
            let op = self.advance();
            let end = if self.peek().is_some_and(starts_expression) {
                let end = self.coalesce_expr();
                if end.is_error() {
                    return end;
                }
                Some(Box::new(end))
            } else {
                None
            };
            let step = match self.range_step() {
                Ok(step) => step,
                Err(error) => return error,
            };
            let span = match (&end, &step) {
                (_, Some(step)) => join_spans(op.span, step.span()),
                (Some(end), None) => join_spans(op.span, end.span()),
                (None, None) => op.span,
            };
            return Expression::Range {
                start: None,
                end,
                inclusive_end: op.kind == TokenKind::DotDotEqual,
                step,
                span,
            };
        }
        let left = self.coalesce_expr();
        if left.is_error() {
            return left;
        }
        let op = match self.peek() {
            Some(TokenKind::DotDot) => BinaryOp::RangeExclusive,
            Some(TokenKind::DotDotEqual) => BinaryOp::RangeInclusive,
            _ => return left,
        };
        let range_token = self.advance();
        if !self.peek().is_some_and(starts_expression) {
            let step = match self.range_step() {
                Ok(step) => step,
                Err(error) => return error,
            };
            let span = match &step {
                Some(step) => join_spans(left.span(), step.span()),
                None => join_spans(left.span(), range_token.span),
            };
            return Expression::Range {
                start: Some(Box::new(left)),
                end: None,
                inclusive_end: matches!(op, BinaryOp::RangeInclusive),
                step,
                span,
            };
        }
        let right = self.coalesce_expr();
        if right.is_error() {
            return right;
        }
        let step = match self.range_step() {
            Ok(step) => step,
            Err(error) => return error,
        };
        if let Some(step) = step {
            let span = join_spans(left.span(), step.span());
            return Expression::Range {
                start: Some(Box::new(left)),
                end: Some(Box::new(right)),
                inclusive_end: matches!(op, BinaryOp::RangeInclusive),
                step: Some(step),
                span,
            };
        }
        binary_expr(op, left, right)
    }

    /// Parse an optional `by <step>` range suffix. `Ok(None)` when no `by` follows;
    /// `Err(error)` propagates a failed step operand as the collapsing error node.
    fn range_step(&mut self) -> Result<Option<Box<Expression>>, Expression> {
        if !self.peek_is_contextual("by") {
            return Ok(None);
        }
        self.advance();
        let step = self.coalesce_expr();
        if step.is_error() {
            return Err(step);
        }
        Ok(Some(Box::new(step)))
    }

    /// `??` sits one level tighter than ranges and looser than addition:
    /// `count ?? 0 < 5` parses as `(count ?? 0) < 5`. It is right-associative, so
    /// `a ?? b ?? c` parses as `a ?? (b ?? c)`, each `??` defaulting the optional on
    /// its left and the chain typing under the coalesce rule.
    fn coalesce_expr(&mut self) -> Expression {
        let left = self.additive_expr();
        if left.is_error() || !matches!(self.peek(), Some(TokenKind::QuestionQuestion)) {
            return left;
        }
        self.advance();
        let right = self.descend(Self::coalesce_expr);
        if right.is_error() {
            return right;
        }
        binary_expr(BinaryOp::Coalesce, left, right)
    }

    fn additive_expr(&mut self) -> Expression {
        self.binary_chain(Self::multiplicative_expr, |kind| match kind {
            TokenKind::Plus => Some(BinaryOp::Add),
            TokenKind::Minus => Some(BinaryOp::Subtract),
            _ => None,
        })
    }

    fn multiplicative_expr(&mut self) -> Expression {
        self.binary_chain(Self::unary_expr, |kind| match kind {
            TokenKind::Star => Some(BinaryOp::Multiply),
            TokenKind::Slash => Some(BinaryOp::Divide),
            TokenKind::Percent => Some(BinaryOp::Remainder),
            _ => None,
        })
    }

    fn unary_expr(&mut self) -> Expression {
        let op = match self.peek() {
            Some(TokenKind::Minus) => UnaryOp::Neg,
            Some(TokenKind::Keyword(Keyword::Not)) => UnaryOp::Not,
            _ => return self.postfix_expr(),
        };
        let op_token = self.advance();
        let operand = self.descend(Self::unary_expr);
        if operand.is_error() {
            return operand;
        }
        let span = join_spans(op_token.span, operand.span());
        Expression::Unary {
            op,
            operand: Box::new(operand),
            span,
        }
    }

    fn postfix_expr(&mut self) -> Expression {
        let mut expr = self.primary_expr();
        if expr.is_error() {
            return expr;
        }
        let mut levels = 0;
        loop {
            // A `.f`, `?.f`, or `(…)` postfix each wraps the current expression in
            // one more node, so a long `a.f.f…` or `a()()…` chain deepens the AST by
            // its length and counts toward the nesting limit.
            if matches!(
                self.peek(),
                Some(TokenKind::LeftParen | TokenKind::Dot | TokenKind::QuestionDot)
            ) {
                if !self.enter_chain_level() {
                    self.leave_chain(levels);
                    return self.nesting_limit_error();
                }
                levels += 1;
            }
            match self.peek() {
                Some(TokenKind::LeftParen) => {
                    let open = self.advance();
                    let parsed_args = match self.arguments() {
                        Ok(args) => args,
                        Err(error) => {
                            self.leave_chain(levels);
                            return error;
                        }
                    };
                    let Some(TokenKind::RightParen) = self.peek() else {
                        // The argument list is unterminated: a token follows the last
                        // argument where a `,` or `)` was expected. Name the missing
                        // delimiter at the gap, then recover the call node with the
                        // arguments parsed so far so downstream analysis — callee
                        // contexts, signature help — still sees an incomplete call.
                        let (expected, message) = if self.peek().is_some_and(starts_expression) {
                            (ExpectedSyntax::Comma, "expected `,`")
                        } else {
                            (ExpectedSyntax::CloseParen, "expected `)`")
                        };
                        self.expected_delimiter_at_gap(expected, message);
                        let end = self.tokens[self.pos - 1].span;
                        let span = join_spans(expr.span(), end);
                        let multiline = parsed_args.trailing_comma || end.line > open.span.line;
                        self.leave_chain(levels);
                        return Expression::Call {
                            callee: Box::new(expr),
                            args: parsed_args.args,
                            multiline,
                            span,
                        };
                    };
                    let close = self.advance();
                    let span = join_spans(expr.span(), close.span);
                    let multiline = parsed_args.trailing_comma || close.span.line > open.span.line;
                    expr = Expression::Call {
                        callee: Box::new(expr),
                        args: parsed_args.args,
                        multiline,
                        span,
                    };
                }
                Some(TokenKind::Dot) => {
                    let (name, quoted, name_span) = match self.field_segment() {
                        Ok(segment) => segment,
                        Err(error) => {
                            self.leave_chain(levels);
                            return error;
                        }
                    };
                    let span = join_spans(expr.span(), name_span);
                    expr = Expression::Field {
                        base: Box::new(expr),
                        name,
                        name_span,
                        quoted,
                        span,
                    };
                }
                // `base?.name`: the same field segment as `.`, but the read
                // short-circuits to absent rather than failing if the base or field
                // is missing.
                Some(TokenKind::QuestionDot) => {
                    let (name, quoted, name_span) = match self.field_segment() {
                        Ok(segment) => segment,
                        Err(error) => {
                            self.leave_chain(levels);
                            return error;
                        }
                    };
                    let span = join_spans(expr.span(), name_span);
                    expr = Expression::OptionalField {
                        base: Box::new(expr),
                        name,
                        name_span,
                        quoted,
                        span,
                    };
                }
                _ => break,
            }
        }
        self.leave_chain(levels);
        expr
    }

    /// Parse the identifier segment after `.` or `?.`, consuming both tokens.
    /// `Err(error)` collapses a malformed segment into the reported error node.
    fn field_segment(&mut self) -> Result<(String, bool, SourceSpan), Expression> {
        let op = self.advance();
        let Some(segment) = self.tokens.get(self.pos).copied() else {
            return Err(self.error_expr(
                self.gap_span(),
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected a field name".to_string(),
                None,
            ));
        };
        let text = segment.text(self.source);
        let (name, quoted) = match segment.kind {
            TokenKind::Identifier => (text.to_string(), false),
            // `checked` is a keyword (the checked-arithmetic statement head), but in
            // field position it is the nominal-type range-test member `Age.checked(n)`.
            // The parser admits the spelling; which bases have such a member is a
            // checker rule.
            TokenKind::Keyword(Keyword::Checked) => (text.to_string(), false),
            TokenKind::String => {
                return Err(self.error_expr(
                    join_spans(op.span, segment.span),
                    ParseDiagnosticReason::Unsupported(UnsupportedSyntax::QuotedFieldSegments),
                    "quoted field segments are not part of expression grammar".to_string(),
                    None,
                ));
            }
            // A reserved word is never a valid field name; report it with both
            // tokens in view.
            TokenKind::Keyword(_) => {
                return Err(self.error_expr(
                    join_spans(op.span, segment.span),
                    ParseDiagnosticReason::KeywordFieldName,
                    format!("`{text}` is a keyword and cannot be used as a field name"),
                    None,
                ));
            }
            _ => {
                return Err(self.error_expr(
                    segment.span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                    "expected a field name".to_string(),
                    None,
                ));
            }
        };
        self.advance();
        Ok((name, quoted, segment.span))
    }

    /// Parse a comma-separated argument list up to the closing `)`. `Err(error)`
    /// collapses a failed argument into the reported error node.
    fn arguments(&mut self) -> Result<ParsedArguments, Expression> {
        let mut args = Vec::new();
        if matches!(self.peek(), Some(TokenKind::RightParen)) {
            return Ok(ParsedArguments {
                args,
                trailing_comma: false,
            });
        }
        let mut seen_named = false;
        let mut trailing_comma = false;
        loop {
            let arg = self.argument()?;
            // After the first named argument, every remaining argument must be
            // named: a plain positional one would silently back-fill an earlier
            // parameter.
            if seen_named && arg.name.is_none() {
                let span = arg.value.span();
                self.error(
                    span,
                    ParseDiagnosticReason::PositionalArgumentAfterNamed,
                    "a positional argument cannot follow a named argument".to_string(),
                    Some("name this argument or move it before the named arguments".to_string()),
                );
            }
            seen_named |= arg.name.is_some();
            args.push(arg);
            if !matches!(self.peek(), Some(TokenKind::Comma)) {
                break;
            }
            self.advance();
            if matches!(self.peek(), Some(TokenKind::RightParen)) {
                trailing_comma = true;
                break;
            }
        }
        Ok(ParsedArguments {
            args,
            trailing_comma,
        })
    }

    fn argument(&mut self) -> Result<Argument, Expression> {
        self.recover_removed_argument_mode();
        let name = if matches!(self.peek(), Some(TokenKind::Identifier))
            && matches!(self.peek_at(1), Some(TokenKind::Colon))
        {
            let identifier = self.advance();
            self.advance();
            Some(identifier.text(self.source).to_string())
        } else {
            None
        };
        let value = self.expression();
        if value.is_error() {
            return Err(value);
        }
        Ok(Argument { name, value })
    }

    fn recover_removed_argument_mode(&mut self) {
        let Some(token) = self.tokens.get(self.pos).copied() else {
            return;
        };
        if token.kind != TokenKind::Identifier
            || !self
                .peek_at(1)
                .is_some_and(starts_unambiguous_removed_mode_target)
        {
            return;
        }
        let text = token.text(self.source);
        if text != "inout" && text != "out" {
            return;
        }
        self.error(
            token.span,
            ParseDiagnosticReason::Unsupported(UnsupportedSyntax::ParameterModes),
            "parameter modes were removed; call arguments are ordinary values".to_string(),
            Some("return the new value and assign the returned value at the call site".to_string()),
        );
        self.advance();
    }

    fn primary_expr(&mut self) -> Expression {
        let Some(token) = self.tokens.get(self.pos).copied() else {
            return self.error_expr(
                self.gap_span(),
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected an expression".to_string(),
                None,
            );
        };
        let text = token.text(self.source);
        match token.kind {
            TokenKind::Integer => self.literal(token, LiteralKind::Integer),
            TokenKind::Decimal => self.literal(token, LiteralKind::Decimal),
            TokenKind::Duration => self.literal(token, LiteralKind::Duration),
            TokenKind::String => self.literal(token, LiteralKind::String),
            TokenKind::Bytes => self.literal(token, LiteralKind::Bytes),
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                self.literal(token, LiteralKind::Bool)
            }
            // `absent` is the empty-optional primary value, inert until resolved.
            TokenKind::Keyword(Keyword::Absent) => {
                self.advance();
                Expression::Absent { span: token.span }
            }
            TokenKind::Identifier => self.name_expr(),
            // A path segment keyword leading `::` starts a name path
            // (`bytes::length`).
            TokenKind::Keyword(keyword)
                if is_expression_path_segment_keyword(keyword)
                    && matches!(self.peek_at(1), Some(TokenKind::DoubleColon)) =>
            {
                self.name_expr()
            }
            // A keyword constructor is only a value when called (`int(...)`,
            // `Error(...)`, `Id(^root, ...)`).
            TokenKind::Keyword(keyword)
                if is_expression_callable_keyword(keyword)
                    && matches!(self.peek_at(1), Some(TokenKind::LeftParen)) =>
            {
                self.advance();
                Expression::Name {
                    segments: vec![text.to_string()],
                    segment_spans: vec![token.span],
                    span: token.span,
                }
            }
            TokenKind::Caret => {
                self.advance();
                let Some(name) = self.tokens.get(self.pos).copied() else {
                    return self.error_expr(
                        self.gap_span(),
                        ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                        "expected a saved-root name after `^`".to_string(),
                        None,
                    );
                };
                if name.kind != TokenKind::Identifier {
                    return self.error_expr(
                        name.span,
                        ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                        "expected a saved-root name after `^`".to_string(),
                        None,
                    );
                }
                self.advance();
                Expression::SavedRoot {
                    name: name.text(self.source).to_string(),
                    span: join_spans(token.span, name.span),
                }
            }
            TokenKind::LeftParen => {
                self.advance();
                let inner = self.expression();
                if inner.is_error() {
                    // The failed operand already reported at its own token; the
                    // missing `)` is a consequence of it, not a second fault.
                    return inner;
                }
                if matches!(self.peek(), Some(TokenKind::RightParen)) {
                    self.advance();
                    inner
                } else if self.report_stray_assignment_operator() {
                    // A stray `=`/compound-assign before the `)` is reported at that
                    // operator rather than as an unstructured group.
                    Expression::Error {
                        span: self.tokens[self.pos].span,
                    }
                } else {
                    self.expected_delimiter_at_gap(ExpectedSyntax::CloseParen, "expected `)`");
                    Expression::Error {
                        span: self.gap_span(),
                    }
                }
            }
            TokenKind::InterpolationStart => self.interpolation_expr(),
            TokenKind::LeftBracket => self.error_expr(
                token.span,
                ParseDiagnosticReason::Unsupported(UnsupportedSyntax::BracketCollectionLiterals),
                "bracket collection literals are not part of expression grammar".to_string(),
                Some(
                    "build a list with `var xs: List[T] = List()` and \
                    `xs = append(xs, value)`, or call a function that returns a list"
                        .to_string(),
                ),
            ),
            TokenKind::Keyword(_) => self.error_expr(
                token.span,
                ParseDiagnosticReason::KeywordExpression,
                format!("`{text}` is a keyword and cannot be used as an expression"),
                Some("choose an identifier that is not reserved".to_string()),
            ),
            _ => self.error_expr(
                token.span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected an expression".to_string(),
                None,
            ),
        }
    }

    fn literal(&mut self, token: Token, kind: LiteralKind) -> Expression {
        self.advance();
        Expression::Literal {
            kind,
            text: token.text(self.source).to_string(),
            span: token.span,
        }
    }

    fn interpolation_expr(&mut self) -> Expression {
        let start = self.advance();
        let mut parts = Vec::new();
        loop {
            let Some(token) = self.tokens.get(self.pos).copied() else {
                return self.error_expr(
                    self.gap_span(),
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                    "expected an expression".to_string(),
                    None,
                );
            };
            match token.kind {
                TokenKind::InterpolationText => {
                    self.advance();
                    parts.push(InterpolationPart::Text {
                        text: token.text(self.source).to_string(),
                        span: token.span,
                    });
                }
                TokenKind::InterpolationExprStart => {
                    self.advance();
                    let expr = self.expression();
                    if expr.is_error() {
                        // An empty `{}` or a dangling operator failed at its own
                        // token; skip to the closing brace so later text and holes
                        // still parse rather than aborting the whole string.
                        self.recover_to_interpolation_hole_end();
                    } else if matches!(self.peek(), Some(TokenKind::InterpolationExprEnd)) {
                        self.advance();
                        parts.push(InterpolationPart::Expr(expr));
                    } else {
                        // A complete operand followed by trailing tokens before the
                        // closing brace (`{a b}`) leaves the hole unclosed. Name the
                        // stray token and skip to the brace.
                        let span = self
                            .tokens
                            .get(self.pos)
                            .map_or(token.span, |token| token.span);
                        self.error(
                            span,
                            ParseDiagnosticReason::Expected(ExpectedSyntax::InterpolationHoleEnd),
                            "expected the end of the interpolation hole".to_string(),
                            Some("close the hole with `}`".to_string()),
                        );
                        self.recover_to_interpolation_hole_end();
                    }
                }
                TokenKind::InterpolationEnd => {
                    self.advance();
                    return Expression::Interpolation {
                        parts,
                        span: join_spans(start.span, token.span),
                    };
                }
                _ => {
                    return self.error_expr(
                        token.span,
                        ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                        "expected an expression".to_string(),
                        None,
                    );
                }
            }
        }
    }

    /// Skip a malformed interpolation hole up to and including its closing brace,
    /// so a reported missing operand does not abort the rest of the string.
    fn recover_to_interpolation_hole_end(&mut self) {
        while let Some(kind) = self.peek() {
            if kind == TokenKind::InterpolationEnd {
                break;
            }
            self.advance();
            if kind == TokenKind::InterpolationExprEnd {
                break;
            }
        }
    }

    fn name_expr(&mut self) -> Expression {
        let first = self.advance();
        let mut segments = vec![first.text(self.source).to_string()];
        let mut segment_spans = vec![first.span];
        let mut end = first.span;
        while matches!(self.peek(), Some(TokenKind::DoubleColon)) {
            self.advance();
            let Some(segment) = self.tokens.get(self.pos).copied() else {
                return self.error_expr(
                    self.gap_span(),
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                    "expected a name segment after `::`".to_string(),
                    None,
                );
            };
            // A path segment is an identifier or an allowed keyword used as a name,
            // such as the `bytes` in `std::bytes::length`.
            let is_segment = match segment.kind {
                TokenKind::Identifier => true,
                TokenKind::Keyword(keyword) => is_expression_path_segment_keyword(keyword),
                _ => false,
            };
            if !is_segment {
                return self.error_expr(
                    segment.span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                    "expected a name segment after `::`".to_string(),
                    None,
                );
            }
            self.advance();
            segments.push(segment.text(self.source).to_string());
            segment_spans.push(segment.span);
            end = segment.span;
        }
        Expression::Name {
            segments,
            segment_spans,
            span: join_spans(first.span, end),
        }
    }

    /// Report a missing call/group delimiter at the gap just past the last consumed
    /// token, when a complete operand sits inside an unterminated `(` or before a
    /// missing `,`. The span is the zero-width point after the operand, always a
    /// valid 1-based position.
    fn expected_delimiter_at_gap(&mut self, expected: ExpectedSyntax, message: &str) {
        let span = self.gap_span();
        self.error(
            span,
            ParseDiagnosticReason::Expected(expected),
            message.to_string(),
            None,
        );
    }
}

struct ParsedArguments {
    args: Vec<Argument>,
    trailing_comma: bool,
}

fn starts_expression(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Integer
        | TokenKind::Decimal
        | TokenKind::Duration
        | TokenKind::String
        | TokenKind::Bytes
        | TokenKind::Identifier
        | TokenKind::Caret
        | TokenKind::LeftParen
        | TokenKind::InterpolationStart
        | TokenKind::Minus
        | TokenKind::DotDot
        | TokenKind::DotDotEqual => true,
        TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Not) => true,
        TokenKind::Keyword(keyword) => is_expression_callable_keyword(keyword),
        _ => false,
    }
}

fn starts_unambiguous_removed_mode_target(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Identifier | TokenKind::Caret)
}

fn binary_expr(op: BinaryOp, left: Expression, right: Expression) -> Expression {
    let span = join_spans(left.span(), right.span());
    Expression::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
        span,
    }
}

pub(crate) fn join_spans(start: SourceSpan, end: SourceSpan) -> SourceSpan {
    SourceSpan {
        start_byte: start.start_byte,
        end_byte: end.end_byte,
        line: start.line,
        column: start.column,
    }
}
