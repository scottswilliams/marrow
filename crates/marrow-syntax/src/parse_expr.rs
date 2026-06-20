//! The expression parser: a recursive-descent parser for a single Marrow
//! expression over a token slice, covering primary, postfix, unary, and binary
//! precedence levels including calls and saved paths.

use crate::token::is_trivia;
use crate::{
    Argument, BinaryOp, Diagnostic, DiagnosticReason, Expression, InterpolationPart, Keyword,
    LiteralKind, NESTING_DEPTH_LIMIT, NESTING_LIMIT, PARSE_SYNTAX, ParseDiagnosticReason, Severity,
    SourceSpan, Token, TokenKind, UnaryOp, UnsupportedSyntax, is_expression_callable_keyword,
    is_expression_path_segment_keyword,
};

/// The remedy shared by the comparison and equality non-associative levels: the
/// spec directs the author to parenthesize when comparing boolean results. It
/// rides in the diagnostic message so it survives the checker's parse-diagnostic
/// lowering and renders in `marrow check`, where `help` is dropped.
const COMPARE_NONASSOC_REMEDY: &str = "use parentheses to compare boolean results";

/// A value the parser does not fully structure yields `None`, which the caller
/// turns into a syntax diagnostic.
pub(crate) struct ExprParser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    /// How deep the recursive descent currently is. Each parenthesized,
    /// unary-operand, or interpolated sub-expression descends one level;
    /// exceeding [`NESTING_DEPTH_LIMIT`] stops the recursion with a located
    /// [`NESTING_LIMIT`] error before the native stack can overflow.
    depth: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> ExprParser<'a> {
    pub(crate) fn new(source: &'a str, tokens: &[Token]) -> Self {
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
            diagnostics: Vec::new(),
        }
    }

    /// Returns `None` unless the whole slice parses as one expression. Syntax
    /// diagnostics raised while parsing are drained into the caller's vec.
    pub(crate) fn parse_complete(
        mut self,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Expression> {
        if self.tokens.is_empty() {
            return None;
        }
        let expr = self.expression();
        // A `=` left where the expression should have ended is the `=`-for-`==`
        // mistake: `=` is assignment only, so it never appears mid-expression.
        if expr.is_some() {
            self.report_stray_equals();
        }
        diagnostics.append(&mut self.diagnostics);
        let expr = expr?;
        (self.pos == self.tokens.len()).then_some(expr)
    }

    /// If the next unconsumed token is `=`, report the `=`-vs-`==` mistake at that
    /// token and return `true`. `=` is assignment only, so a `=` reached in
    /// expression position is this common error rather than a generic one.
    fn report_stray_equals(&mut self) -> bool {
        let Some(token) = self.tokens.get(self.pos).copied() else {
            return false;
        };
        if token.kind != TokenKind::Equal {
            return false;
        }
        self.error(
            token.span,
            ParseDiagnosticReason::EqualsInExpression,
            "`=` is assignment, not equality; use `==` for equality".to_string(),
            None,
        );
        true
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

    fn expression(&mut self) -> Option<Expression> {
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
    fn descend(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Option<Expression>,
    ) -> Option<Expression> {
        self.depth += 1;
        if self.depth > NESTING_DEPTH_LIMIT {
            self.depth -= 1;
            self.nesting_limit_error();
            return None;
        }
        let result = parse(self);
        self.depth -= 1;
        result
    }

    /// Account for one more level of left-associated nesting built by a binary
    /// operator chain or a postfix field/call chain. A flat `1 + 1 + …` line or
    /// `a.f.f…` chain builds an AST as deep as it is long without recursing here,
    /// so each accumulation step must count toward the same limit as a
    /// parenthesis. Returns `false` once the limit is crossed, after reporting the
    /// overflow, so the chain stops growing. The caller unwinds the levels it
    /// added with [`Self::leave_chain`] before returning, since the finished chain
    /// sits at its caller's depth.
    fn enter_chain_level(&mut self) -> bool {
        self.depth += 1;
        if self.depth > NESTING_DEPTH_LIMIT {
            self.depth -= 1;
            self.nesting_limit_error();
            return false;
        }
        true
    }

    fn leave_chain(&mut self, levels: usize) {
        self.depth -= levels;
    }

    /// Report the nesting-limit overflow at the next unconsumed token (the deepest
    /// construct the parser reached), or at end of input when none remains.
    fn nesting_limit_error(&mut self) {
        let span = self
            .tokens
            .get(self.pos)
            .map_or_else(SourceSpan::default, |token| token.span);
        self.diagnostics.push(Diagnostic {
            code: NESTING_LIMIT,
            reason: DiagnosticReason::Parser(ParseDiagnosticReason::NestingLimit),
            severity: Severity::Error,
            message: format!("expression nests deeper than the limit of {NESTING_DEPTH_LIMIT}"),
            help: None,
            span,
        });
    }

    /// Parse a left-associated chain of one operator precedence: an `operand`,
    /// then each `operator operand` repetition the operator table accepts. Each
    /// repetition deepens the AST by one node, so it counts toward the nesting
    /// limit; the levels added are unwound once the chain is complete, since the
    /// finished chain sits at its caller's depth.
    fn binary_chain(
        &mut self,
        operand: impl Fn(&mut Self) -> Option<Expression>,
        operator: impl Fn(TokenKind) -> Option<BinaryOp>,
    ) -> Option<Expression> {
        let mut left = operand(self)?;
        let mut levels = 0;
        while let Some(op) = self.peek().and_then(&operator) {
            if !self.enter_chain_level() {
                self.leave_chain(levels);
                return None;
            }
            levels += 1;
            self.advance();
            let right = operand(self)?;
            left = binary_expr(op, left, right);
        }
        self.leave_chain(levels);
        Some(left)
    }

    fn or_expr(&mut self) -> Option<Expression> {
        self.binary_chain(Self::and_expr, |kind| {
            matches!(kind, TokenKind::Keyword(Keyword::Or)).then_some(BinaryOp::Or)
        })
    }

    fn and_expr(&mut self) -> Option<Expression> {
        self.binary_chain(Self::is_expr, |kind| {
            matches!(kind, TokenKind::Keyword(Keyword::And)).then_some(BinaryOp::And)
        })
    }

    /// `is` sits one level looser than equality and tighter than `and`, on its own
    /// non-associative level: a single `is`, never chained (`a is X is Y` is
    /// rejected). The right operand is parsed as an equality-level expression;
    /// any narrower member-path restriction is enforced by the checker.
    fn is_expr(&mut self) -> Option<Expression> {
        let left = self.equality_expr()?;
        if !matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Is))) {
            return Some(left);
        }
        self.advance();
        let right = self.equality_expr()?;
        if matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Is))) {
            return self
                .reject_chained_operator("is", "use parentheses to group the subtree tests");
        }
        Some(binary_expr(BinaryOp::Is, left, right))
    }

    fn equality_expr(&mut self) -> Option<Expression> {
        let left = self.comparison_expr()?;
        let (op, text) = match self.peek() {
            Some(TokenKind::EqualEqual) => (BinaryOp::Equal, "=="),
            Some(TokenKind::BangEqual) => (BinaryOp::NotEqual, "!="),
            _ => return Some(left),
        };
        self.advance();
        let right = self.comparison_expr()?;
        if matches!(
            self.peek(),
            Some(TokenKind::EqualEqual | TokenKind::BangEqual)
        ) {
            return self.reject_chained_operator(text, COMPARE_NONASSOC_REMEDY);
        }
        Some(binary_expr(op, left, right))
    }

    fn comparison_expr(&mut self) -> Option<Expression> {
        let left = self.range_expr()?;
        let (op, text) = match self.peek() {
            Some(TokenKind::Less) => (BinaryOp::Less, "<"),
            Some(TokenKind::LessEqual) => (BinaryOp::LessEqual, "<="),
            Some(TokenKind::Greater) => (BinaryOp::Greater, ">"),
            Some(TokenKind::GreaterEqual) => (BinaryOp::GreaterEqual, ">="),
            _ => return Some(left),
        };
        self.advance();
        let right = self.range_expr()?;
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
        Some(binary_expr(op, left, right))
    }

    /// Report a second operator on a non-associative level (`==`/`!=`, a
    /// comparison, `is`, or `??`) at that operator's own token, mirroring the
    /// coalesce diagnostic, and abort the expression. `text` is the operator
    /// spelling and `remedy` the spec fix; both ride in the message so the remedy
    /// survives the checker's parse-diagnostic lowering and renders in
    /// `marrow check`, where the `help` field is dropped. Returning `None` after
    /// reporting suppresses the statement parser's generic fallback, so the only
    /// diagnostic is this operator-spanned one rather than a misplaced "expected a
    /// statement" at the line start.
    fn reject_chained_operator(&mut self, text: &str, remedy: &str) -> Option<Expression> {
        let operator = self.tokens[self.pos];
        self.error(
            operator.span,
            ParseDiagnosticReason::NonAssociativeOperator,
            format!("`{text}` does not chain; {remedy}"),
            None,
        );
        None
    }

    fn range_expr(&mut self) -> Option<Expression> {
        if matches!(
            self.peek(),
            Some(TokenKind::DotDot | TokenKind::DotDotEqual)
        ) {
            let op = self.advance();
            let end = if self.peek().is_some_and(starts_expression) {
                Some(Box::new(self.coalesce_expr()?))
            } else {
                None
            };
            let step = self.range_step()?;
            let span = match (&end, &step) {
                (_, Some(step)) => join_spans(op.span, step.span()),
                (Some(end), None) => join_spans(op.span, end.span()),
                (None, None) => op.span,
            };
            return Some(Expression::Range {
                start: None,
                end,
                inclusive_end: op.kind == TokenKind::DotDotEqual,
                step,
                span,
            });
        }
        let left = self.coalesce_expr()?;
        let op = match self.peek() {
            Some(TokenKind::DotDot) => BinaryOp::RangeExclusive,
            Some(TokenKind::DotDotEqual) => BinaryOp::RangeInclusive,
            _ => return Some(left),
        };
        let range_token = self.advance();
        if !self.peek().is_some_and(starts_expression) {
            let step = self.range_step()?;
            let span = match &step {
                Some(step) => join_spans(left.span(), step.span()),
                None => join_spans(left.span(), range_token.span),
            };
            return Some(Expression::Range {
                start: Some(Box::new(left)),
                end: None,
                inclusive_end: matches!(op, BinaryOp::RangeInclusive),
                step,
                span,
            });
        }
        let right = self.coalesce_expr()?;
        let step = self.range_step()?;
        if let Some(step) = step {
            let span = join_spans(left.span(), step.span());
            return Some(Expression::Range {
                start: Some(Box::new(left)),
                end: Some(Box::new(right)),
                inclusive_end: matches!(op, BinaryOp::RangeInclusive),
                step: Some(step),
                span,
            });
        }
        Some(binary_expr(op, left, right))
    }

    fn range_step(&mut self) -> Option<Option<Box<Expression>>> {
        if !self.peek_is_contextual("by") {
            return Some(None);
        }
        self.advance();
        Some(Some(Box::new(self.coalesce_expr()?)))
    }

    /// `??` sits one level tighter than ranges and looser than addition, on its
    /// own non-associative level: `count ?? 0 < 5` parses as
    /// `(count ?? 0) < 5`, and `a ?? b ?? c` is rejected (never chained).
    fn coalesce_expr(&mut self) -> Option<Expression> {
        let left = self.additive_expr()?;
        if !matches!(self.peek(), Some(TokenKind::QuestionQuestion)) {
            return Some(left);
        }
        self.advance();
        let right = self.additive_expr()?;
        // `??` is non-associative: a second `??` at the same level is a grammar
        // error rather than a left- or right-folded chain, so reject it here
        // instead of leaving the trailing operand for the statement parser.
        if matches!(self.peek(), Some(TokenKind::QuestionQuestion)) {
            return self.reject_chained_operator("??", "write one `??` per read");
        }
        Some(binary_expr(BinaryOp::Coalesce, left, right))
    }

    fn additive_expr(&mut self) -> Option<Expression> {
        self.binary_chain(Self::multiplicative_expr, |kind| match kind {
            TokenKind::Plus => Some(BinaryOp::Add),
            TokenKind::Minus => Some(BinaryOp::Subtract),
            _ => None,
        })
    }

    fn multiplicative_expr(&mut self) -> Option<Expression> {
        self.binary_chain(Self::unary_expr, |kind| match kind {
            TokenKind::Star => Some(BinaryOp::Multiply),
            TokenKind::Slash => Some(BinaryOp::Divide),
            TokenKind::Percent => Some(BinaryOp::Remainder),
            _ => None,
        })
    }

    fn unary_expr(&mut self) -> Option<Expression> {
        let op = match self.peek() {
            Some(TokenKind::Minus) => UnaryOp::Neg,
            Some(TokenKind::Keyword(Keyword::Not)) => UnaryOp::Not,
            _ => return self.postfix_expr(),
        };
        let op_token = self.advance();
        let operand = self.descend(Self::unary_expr)?;
        let span = join_spans(op_token.span, operand.span());
        Some(Expression::Unary {
            op,
            operand: Box::new(operand),
            span,
        })
    }

    fn postfix_expr(&mut self) -> Option<Expression> {
        let mut expr = self.primary_expr()?;
        let mut levels = 0;
        loop {
            // A `.f`, `?.f`, or `(…)` postfix each wraps the current expression in
            // one more node, so a long `a.f.f…` or `a()()…` chain deepens the AST
            // by its length and counts toward the nesting limit.
            if matches!(
                self.peek(),
                Some(TokenKind::LeftParen | TokenKind::Dot | TokenKind::QuestionDot)
            ) {
                if !self.enter_chain_level() {
                    self.leave_chain(levels);
                    return None;
                }
                levels += 1;
            }
            match self.peek() {
                Some(TokenKind::LeftParen) => {
                    let open = self.advance();
                    let parsed_args = self.arguments()?;
                    if !matches!(self.peek(), Some(TokenKind::RightParen)) {
                        return None;
                    }
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
                    let (name, quoted, name_span) = self.field_segment()?;
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
                // short-circuits to absent rather than failing if the base or
                // field is missing.
                Some(TokenKind::QuestionDot) => {
                    let (name, quoted, name_span) = self.field_segment()?;
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
        Some(expr)
    }

    /// Parse the identifier segment after `.` or `?.`, consuming both tokens.
    fn field_segment(&mut self) -> Option<(String, bool, SourceSpan)> {
        let op = self.advance();
        let segment = *self.tokens.get(self.pos)?;
        let text = segment.text(self.source);
        let (name, quoted) = match segment.kind {
            TokenKind::Identifier => (text.to_string(), false),
            TokenKind::String => {
                self.error(
                    join_spans(op.span, segment.span),
                    ParseDiagnosticReason::Unsupported(UnsupportedSyntax::QuotedFieldSegments),
                    "quoted field segments are not part of expression grammar".to_string(),
                    None,
                );
                return None;
            }
            // A reserved word is never a valid field name; report it with both
            // tokens in view.
            TokenKind::Keyword(_) => {
                self.error(
                    join_spans(op.span, segment.span),
                    ParseDiagnosticReason::KeywordFieldName,
                    format!("`{text}` is a keyword and cannot be used as a field name"),
                    None,
                );
                return None;
            }
            _ => return None,
        };
        self.advance();
        Some((name, quoted, segment.span))
    }

    fn arguments(&mut self) -> Option<ParsedArguments> {
        let mut args = Vec::new();
        if matches!(self.peek(), Some(TokenKind::RightParen)) {
            return Some(ParsedArguments {
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
        Some(ParsedArguments {
            args,
            trailing_comma,
        })
    }

    fn argument(&mut self) -> Option<Argument> {
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
        let value = self.expression()?;
        Some(Argument { name, value })
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

    fn primary_expr(&mut self) -> Option<Expression> {
        let token = *self.tokens.get(self.pos)?;
        let literal = |kind| {
            Some(Expression::Literal {
                kind,
                text: token.text(self.source).to_string(),
                span: token.span,
            })
        };
        match token.kind {
            TokenKind::Integer => {
                self.advance();
                literal(LiteralKind::Integer)
            }
            TokenKind::Decimal => {
                self.advance();
                literal(LiteralKind::Decimal)
            }
            TokenKind::Duration => {
                self.advance();
                literal(LiteralKind::Duration)
            }
            TokenKind::String => {
                self.advance();
                literal(LiteralKind::String)
            }
            TokenKind::Bytes => {
                self.advance();
                literal(LiteralKind::Bytes)
            }
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                self.advance();
                literal(LiteralKind::Bool)
            }
            TokenKind::Identifier => self.name_expr(),
            // A path segment keyword leading `::` starts a name path
            // (`bytes::length`). Keyword call recovery still treats keyword call
            // heads as callable only for single-token calls.
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
                Some(Expression::Name {
                    segments: vec![token.text(self.source).to_string()],
                    segment_spans: vec![token.span],
                    span: token.span,
                })
            }
            TokenKind::Caret => {
                self.advance();
                let name = *self.tokens.get(self.pos)?;
                if name.kind != TokenKind::Identifier {
                    return None;
                }
                self.advance();
                Some(Expression::SavedRoot {
                    name: name.text(self.source).to_string(),
                    span: join_spans(token.span, name.span),
                })
            }
            TokenKind::LeftParen => {
                self.advance();
                let inner = self.expression()?;
                if matches!(self.peek(), Some(TokenKind::RightParen)) {
                    self.advance();
                    Some(inner)
                } else {
                    // A `=` before the closing `)` is the `=`-for-`==` mistake;
                    // report it pointedly rather than as an unstructured group.
                    self.report_stray_equals();
                    None
                }
            }
            TokenKind::InterpolationStart => self.interpolation_expr(),
            TokenKind::Keyword(_) => {
                let keyword = token.text(self.source);
                self.error(
                    token.span,
                    ParseDiagnosticReason::KeywordExpression,
                    format!("`{keyword}` is a keyword and cannot be used as an expression"),
                    Some("choose an identifier that is not reserved".to_string()),
                );
                None
            }
            _ => None,
        }
    }

    fn interpolation_expr(&mut self) -> Option<Expression> {
        let start = self.advance();
        let mut parts = Vec::new();
        loop {
            let token = *self.tokens.get(self.pos)?;
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
                    let expr = self.expression()?;
                    if !matches!(self.peek(), Some(TokenKind::InterpolationExprEnd)) {
                        return None;
                    }
                    self.advance();
                    parts.push(InterpolationPart::Expr(expr));
                }
                TokenKind::InterpolationEnd => {
                    self.advance();
                    return Some(Expression::Interpolation {
                        parts,
                        span: join_spans(start.span, token.span),
                    });
                }
                _ => return None,
            }
        }
    }

    fn name_expr(&mut self) -> Option<Expression> {
        let first = self.advance();
        let mut segments = vec![first.text(self.source).to_string()];
        let mut segment_spans = vec![first.span];
        let mut end = first.span;
        while matches!(self.peek(), Some(TokenKind::DoubleColon)) {
            self.advance();
            let segment = *self.tokens.get(self.pos)?;
            // A path segment is an identifier or an allowed keyword used as a
            // name, such as the `bytes` in `std::bytes::length`.
            let is_segment = match segment.kind {
                TokenKind::Identifier => true,
                TokenKind::Keyword(keyword) => is_expression_path_segment_keyword(keyword),
                _ => false,
            };
            if !is_segment {
                return None;
            }
            self.advance();
            segments.push(segment.text(self.source).to_string());
            segment_spans.push(segment.span);
            end = segment.span;
        }
        Some(Expression::Name {
            segments,
            segment_spans,
            span: join_spans(first.span, end),
        })
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
