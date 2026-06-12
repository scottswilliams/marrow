//! The expression parser: a recursive-descent parser for a single Marrow
//! expression over a token slice, covering primary, postfix, unary, and binary
//! precedence levels including calls and saved paths.

use crate::token::is_trivia;
use crate::{
    Argument, BinaryOp, Diagnostic, DiagnosticReason, Expression, InterpolationPart, Keyword,
    LiteralKind, NESTING_DEPTH_LIMIT, NESTING_LIMIT, PARSE_SYNTAX, ParseDiagnosticReason, Severity,
    SourceSpan, Token, TokenKind, UnaryOp, UnsupportedSyntax,
};

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
        diagnostics.append(&mut self.diagnostics);
        let expr = expr?;
        (self.pos == self.tokens.len()).then_some(expr)
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

    fn or_expr(&mut self) -> Option<Expression> {
        let mut left = self.and_expr()?;
        while matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Or))) {
            self.advance();
            let right = self.and_expr()?;
            left = binary_expr(BinaryOp::Or, left, right);
        }
        Some(left)
    }

    fn and_expr(&mut self) -> Option<Expression> {
        let mut left = self.is_expr()?;
        while matches!(self.peek(), Some(TokenKind::Keyword(Keyword::And))) {
            self.advance();
            let right = self.is_expr()?;
            left = binary_expr(BinaryOp::And, left, right);
        }
        Some(left)
    }

    /// `is` sits one level looser than equality and tighter than `and`, on its own
    /// non-associative level: a single `is`, never chained (`a is X is Y` is
    /// rejected). The right operand is a member-path expression (`Cat::tiger`).
    fn is_expr(&mut self) -> Option<Expression> {
        let left = self.equality_expr()?;
        if !matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Is))) {
            return Some(left);
        }
        self.advance();
        let right = self.equality_expr()?;
        Some(binary_expr(BinaryOp::Is, left, right))
    }

    fn equality_expr(&mut self) -> Option<Expression> {
        let left = self.comparison_expr()?;
        let op = match self.peek() {
            Some(TokenKind::EqualEqual) => BinaryOp::Equal,
            Some(TokenKind::BangEqual) => BinaryOp::NotEqual,
            _ => return Some(left),
        };
        self.advance();
        let right = self.comparison_expr()?;
        Some(binary_expr(op, left, right))
    }

    fn comparison_expr(&mut self) -> Option<Expression> {
        let left = self.range_expr()?;
        let op = match self.peek() {
            Some(TokenKind::Less) => BinaryOp::Less,
            Some(TokenKind::LessEqual) => BinaryOp::LessEqual,
            Some(TokenKind::Greater) => BinaryOp::Greater,
            Some(TokenKind::GreaterEqual) => BinaryOp::GreaterEqual,
            _ => return Some(left),
        };
        self.advance();
        let right = self.range_expr()?;
        Some(binary_expr(op, left, right))
    }

    fn range_expr(&mut self) -> Option<Expression> {
        let left = self.coalesce_expr()?;
        let op = match self.peek() {
            Some(TokenKind::DotDot) => BinaryOp::RangeExclusive,
            Some(TokenKind::DotDotEqual) => BinaryOp::RangeInclusive,
            _ => return Some(left),
        };
        self.advance();
        let right = self.coalesce_expr()?;
        Some(binary_expr(op, left, right))
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
        Some(binary_expr(BinaryOp::Coalesce, left, right))
    }

    fn additive_expr(&mut self) -> Option<Expression> {
        let mut left = self.multiplicative_expr()?;
        loop {
            let op = match self.peek() {
                Some(TokenKind::Plus) => BinaryOp::Add,
                Some(TokenKind::Minus) => BinaryOp::Subtract,
                _ => break,
            };
            self.advance();
            let right = self.multiplicative_expr()?;
            left = binary_expr(op, left, right);
        }
        Some(left)
    }

    fn multiplicative_expr(&mut self) -> Option<Expression> {
        let mut left = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Some(TokenKind::Star) => BinaryOp::Multiply,
                Some(TokenKind::Slash) => BinaryOp::Divide,
                Some(TokenKind::Percent) => BinaryOp::Remainder,
                _ => break,
            };
            self.advance();
            let right = self.unary_expr()?;
            left = binary_expr(op, left, right);
        }
        Some(left)
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
        loop {
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
                    let (name, quoted, end) = self.field_segment()?;
                    let span = join_spans(expr.span(), end);
                    expr = Expression::Field {
                        base: Box::new(expr),
                        name,
                        quoted,
                        span,
                    };
                }
                // `base?.name`: the same field segment as `.`, but the read
                // short-circuits to absent rather than failing if the base or
                // field is missing.
                Some(TokenKind::QuestionDot) => {
                    let (name, quoted, end) = self.field_segment()?;
                    let span = join_spans(expr.span(), end);
                    expr = Expression::OptionalField {
                        base: Box::new(expr),
                        name,
                        quoted,
                        span,
                    };
                }
                _ => break,
            }
        }
        Some(expr)
    }

    /// Parse the identifier segment after `.` or `?.`, consuming both tokens.
    /// The returned boolean keeps the AST shape aligned with maintenance-only
    /// paths that can still carry quoted field metadata.
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
                    "quoted field segments are not part of ordinary expression grammar; operator maintenance mode is their decided home".to_string(),
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
                    Some(
                        "operator maintenance mode owns non-ordinary saved field names".to_string(),
                    ),
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
            // A type keyword leading `::` starts a name path (`bytes::length`).
            TokenKind::Keyword(keyword)
                if is_callable_keyword(keyword)
                    && matches!(self.peek_at(1), Some(TokenKind::DoubleColon)) =>
            {
                self.name_expr()
            }
            // A type keyword is only a value when called (`int(...)`, `Error(...)`).
            TokenKind::Keyword(keyword)
                if is_callable_keyword(keyword)
                    && matches!(self.peek_at(1), Some(TokenKind::LeftParen)) =>
            {
                self.advance();
                Some(Expression::Name {
                    segments: vec![token.text(self.source).to_string()],
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
        let mut end = first.span;
        while matches!(self.peek(), Some(TokenKind::DoubleColon)) {
            self.advance();
            let segment = *self.tokens.get(self.pos)?;
            // A path segment is an identifier or a type keyword used as a name,
            // such as the `bytes` in `std::bytes::length`.
            let is_segment = match segment.kind {
                TokenKind::Identifier => true,
                TokenKind::Keyword(keyword) => is_callable_keyword(keyword),
                _ => false,
            };
            if !is_segment {
                return None;
            }
            self.advance();
            segments.push(segment.text(self.source).to_string());
            end = segment.span;
        }
        Some(Expression::Name {
            segments,
            span: join_spans(first.span, end),
        })
    }
}

struct ParsedArguments {
    args: Vec<Argument>,
    trailing_comma: bool,
}

/// Type keywords and `Error` that can begin a value when immediately called as
/// a conversion or resource constructor.
fn is_callable_keyword(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::Int
            | Keyword::Decimal
            | Keyword::Bool
            | Keyword::String
            | Keyword::Bytes
            | Keyword::Date
            | Keyword::Instant
            | Keyword::Duration
            | Keyword::ErrorCode
            | Keyword::Error
    )
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
