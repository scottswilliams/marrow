//! The lexer: turns Marrow source text into a flat token stream with
//! file-absolute spans. Blocks are delimited by `{`/`}`; a physical line ends in a
//! `NEWLINE` unless it is continued inside open `(`/`[` or after a trailing
//! header-continuation token (`and`/`or`/`,`/`=`). It records lexical diagnostics
//! as it goes.

use crate::token::{
    duration_unit_seconds, is_identifier_continue_char, is_identifier_start_char, keyword,
};
use crate::{
    Diagnostic, DiagnosticReason, Keyword, LexedSource, LexerDiagnosticReason, NESTING_DEPTH_LIMIT,
    NESTING_LIMIT, ObsoleteOperator, PARSE_SYNTAX, ParseDiagnosticReason, Severity, SourceSpan,
    Token, TokenKind,
};

pub fn lex_source(source: &str) -> LexedSource {
    Lexer::new(source).lex()
}

/// Why scanning an interpolation hole or nested interpolation literal for its
/// closing delimiter did not succeed.
enum InterpolationScanError {
    /// No closing `}`/`"` appeared before the line ended, or a bare `{` opened a
    /// hole with no expression — a genuinely unterminated interpolation.
    Unterminated,
    /// Interpolation nested past [`NESTING_DEPTH_LIMIT`]. Carries the byte offset
    /// of the over-deep opener so the diagnostic anchors at the offending depth,
    /// matching the `check.nesting_limit` contract every other construct reports.
    NestingLimit(usize),
}

/// Whether a range being lexed is an interpolation hole. Inside a hole a nested
/// string literal may write its quotes escaped (`\"..\"`), the spelling an author
/// reaches for within the enclosing `$"..."`; at top level that spelling is not a
/// string.
#[derive(Clone, Copy)]
enum HoleContext {
    TopLevel,
    InHole,
}

struct Lexer<'a> {
    source: &'a str,
    lines: Vec<Line<'a>>,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    /// Open `(`/`[` depth. A `NEWLINE` is suppressed while this is non-zero, so a
    /// call or bracket group spans several physical lines as one logical line.
    open_delimiters: usize,
    /// Open `{` block depth, the brace analogue of the removed indent stack. A `{`
    /// that would open a block deeper than [`NESTING_DEPTH_LIMIT`] reports
    /// [`NESTING_LIMIT`]; braces do not suppress `NEWLINE` (statements end at the
    /// line break inside a block).
    brace_depth: usize,
    /// Set once the brace nesting limit is first crossed, so a run of over-deep
    /// braces reports [`NESTING_LIMIT`] a single time rather than per brace.
    reported_nesting_limit: bool,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            lines: split_lines(source),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            open_delimiters: 0,
            brace_depth: 0,
            reported_nesting_limit: false,
        }
    }

    fn lex(mut self) -> LexedSource {
        // `Line` is `Copy`, so index the line stack rather than holding a borrow
        // across the mutating body. `self.lines` stays intact for `eof_span`.
        for index in 0..self.lines.len() {
            let line = self.lines[index];
            self.reject_line_tabs(line);

            if line.is_blank() {
                continue;
            }

            let is_doc_comment = line.doc_comment().is_some();
            if line.is_comment() || is_doc_comment {
                let kind = if is_doc_comment {
                    TokenKind::DocComment
                } else {
                    TokenKind::Comment
                };
                self.push(kind, line.span_at_content());
                self.push_line_break(line);
                continue;
            }

            self.lex_line(line);
            self.push_line_break(line);
        }

        self.push(TokenKind::Eof, self.eof_span());
        LexedSource {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    /// Emit the `NEWLINE` that ends a physical line, unless the line is continued.
    /// A line continues while inside open `(`/`[`, or when its last significant
    /// token is a header-continuation token (`and`/`or`/`,`/`=`); in either case a
    /// following physical line is part of the same logical line.
    fn push_line_break(&mut self, line: Line<'a>) {
        if self.open_delimiters == 0 && !self.continues_after_last_token() {
            self.push_newline(line);
        }
    }

    /// Whether the last significant token so far ends the line on a
    /// header-continuation token, so the `NEWLINE` is suppressed. Comments and
    /// prior newlines are not significant.
    fn continues_after_last_token(&self) -> bool {
        self.tokens
            .iter()
            .rev()
            .find(|token| {
                !matches!(
                    token.kind,
                    TokenKind::Comment | TokenKind::DocComment | TokenKind::Newline
                )
            })
            .is_some_and(|token| {
                matches!(
                    token.kind,
                    TokenKind::Keyword(Keyword::And)
                        | TokenKind::Keyword(Keyword::Or)
                        | TokenKind::Comma
                        | TokenKind::Equal
                )
            })
    }

    /// Report brace nesting past [`NESTING_DEPTH_LIMIT`] once, at the offending
    /// `{`. Suppressed while the over-deep region lasts so a run of deeper braces
    /// yields a single diagnostic. The braces are still tiled as tokens (lossless
    /// tiling); the recursive-descent parser bounds its own descent so a deep
    /// brace nest fails closed rather than overflowing the native stack.
    fn report_brace_nesting_limit(&mut self, span: SourceSpan) {
        if self.reported_nesting_limit {
            return;
        }
        self.reported_nesting_limit = true;
        self.diagnostics.push(Diagnostic {
            code: NESTING_LIMIT,
            reason: DiagnosticReason::Parser(ParseDiagnosticReason::NestingLimit),
            severity: Severity::Error,
            message: format!("source nests deeper than the limit of {NESTING_DEPTH_LIMIT}"),
            help: None,
            span,
        });
    }

    fn lex_line(&mut self, line: Line<'a>) {
        self.lex_range(
            line,
            line.start_byte + line.indent,
            line.end_byte,
            HoleContext::TopLevel,
        );
    }

    /// Lex `[start, end)` as expression tokens. When `context` is a hole, a nested
    /// string literal may be written with escaped quotes because it sits inside
    /// the enclosing `$"..."`.
    fn lex_range(&mut self, line: Line<'a>, start: usize, end: usize, context: HoleContext) {
        let mut index = start;
        while index < end {
            let Some(tail) = self.source.get(index..line.end_byte) else {
                break;
            };
            let Some(ch) = tail.chars().next() else {
                break;
            };

            if ch == ' ' || ch == '\t' {
                index += ch.len_utf8();
                continue;
            }

            if tail.starts_with("//") {
                let kind = if tail.starts_with("///") {
                    TokenKind::DocComment
                } else {
                    TokenKind::Comment
                };
                // A comment runs to the end of its lexical range, not the physical
                // line: at the top level `end` is the line end, but inside an
                // interpolation hole it is the hole boundary, so the comment stops
                // before the closing `}` and its tokens rather than overlapping
                // them and breaking the lossless token tiling.
                self.push(kind, self.span(line, index, end));
                break;
            }

            if matches!(context, HoleContext::InHole) && tail.starts_with("\\\"") {
                index = self.lex_escaped_hole_string(line, index, end);
                continue;
            }

            if ch == '"' {
                index = self.lex_string(line, index, 0, TokenKind::String);
                continue;
            }

            if tail.starts_with("b\"") {
                index = self.lex_string(line, index, 1, TokenKind::Bytes);
                continue;
            }

            if tail.starts_with("$\"") {
                index = self.lex_interpolation(line, index);
                continue;
            }

            if ch.is_ascii_digit() {
                index = self.lex_number(line, index);
                continue;
            }

            if is_identifier_start_char(ch) {
                index = self.lex_word(line, index);
                continue;
            }

            if let Some(end) = self.reject_obsolete_operator(line, index) {
                index = end;
                continue;
            }

            if let Some((kind, len)) = self.punctuation(index, line.end_byte) {
                self.push_punctuation(kind, self.span(line, index, index + len));
                index += len;
                continue;
            }

            let end = index + ch.len_utf8();
            let (reason, message) = if ch == '~' {
                (
                    LexerDiagnosticReason::ReservedTilde,
                    "`~` is reserved for future typed ephemeral roots".to_string(),
                )
            } else {
                (
                    LexerDiagnosticReason::UnexpectedCharacter(ch),
                    format!("unexpected character `{ch}`"),
                )
            };
            self.error_at(self.span(line, index, end), reason, message);
            index = end;
        }
    }

    fn reject_obsolete_operator(&mut self, line: Line<'a>, index: usize) -> Option<usize> {
        let tail = &self.source[index..line.end_byte];
        let (consumed, reason, message, help) = if tail.starts_with("&&") {
            (
                2,
                ObsoleteOperator::AndAnd,
                "`&&` is not used in Marrow",
                "Use `and` for boolean and.",
            )
        } else if tail.starts_with("||") {
            (
                2,
                ObsoleteOperator::OrOr,
                "`||` is not used in Marrow",
                "Use `or` for boolean or.",
            )
        } else if tail.starts_with('!') && !tail.starts_with("!=") {
            (
                1,
                ObsoleteOperator::Bang,
                "`!` is not used in Marrow",
                "Use `not` for boolean negation.",
            )
        } else if tail.starts_with('#') {
            (
                1,
                ObsoleteOperator::Hash,
                "`#` is not used in Marrow source",
                "Marrow uses `//` for comments.",
            )
        } else {
            return None;
        };

        let end = index + consumed;
        let span = self.span(line, index, end);
        self.error_at_with_help(
            span,
            LexerDiagnosticReason::ObsoleteOperator(reason),
            message,
            Some(help.to_string()),
        );
        Some(end)
    }

    fn lex_interpolation(&mut self, line: Line<'a>, start: usize) -> usize {
        let start_end = start + 2;
        self.push(
            TokenKind::InterpolationStart,
            self.span(line, start, start_end),
        );

        let mut index = start_end;
        let mut text_start = index;
        while index < line.end_byte {
            let Some(tail) = self.source.get(index..line.end_byte) else {
                break;
            };
            if tail.starts_with("{{") || tail.starts_with("}}") {
                index += 2;
                continue;
            }

            let Some(ch) = tail.chars().next() else {
                break;
            };

            if ch == '\\' {
                index += ch.len_utf8();
                let rest = self.source.get(index..line.end_byte);
                if rest.is_some_and(|rest| rest.starts_with("u{")) {
                    // `\u{...}` is a unicode escape recognized before hole
                    // detection: consume through its closing `}` so the interior
                    // `{` opens no interpolation hole. The whole escape stays in
                    // the text part; `decode_string_escapes` validates the scalar.
                    index += "u{".len();
                    while let Some(escaped) = self
                        .source
                        .get(index..line.end_byte)
                        .and_then(|tail| tail.chars().next())
                    {
                        index += escaped.len_utf8();
                        if escaped == '}' {
                            break;
                        }
                    }
                } else if let Some(escaped) = rest.and_then(|tail| tail.chars().next()) {
                    index += escaped.len_utf8();
                }
                continue;
            }

            if ch == '"' {
                self.push_interpolation_text(line, text_start, index);
                let end = index + ch.len_utf8();
                self.push(TokenKind::InterpolationEnd, self.span(line, index, end));
                return end;
            }

            if ch == '{' {
                self.push_interpolation_text(line, text_start, index);
                let expr_start_end = index + ch.len_utf8();
                self.push(
                    TokenKind::InterpolationExprStart,
                    self.span(line, index, expr_start_end),
                );

                let expr_end = match self.find_interpolation_expr_end(line, expr_start_end, 1) {
                    Ok(expr_end) => expr_end,
                    Err(InterpolationScanError::Unterminated) => {
                        self.error_at(
                            self.span(line, index, line.end_byte),
                            LexerDiagnosticReason::UnterminatedInterpolationExpression,
                            "unterminated interpolation expression",
                        );
                        return line.end_byte;
                    }
                    Err(InterpolationScanError::NestingLimit(offending)) => {
                        self.report_interpolation_nesting_limit(line, offending);
                        return line.end_byte;
                    }
                };

                self.lex_range(line, expr_start_end, expr_end, HoleContext::InHole);
                self.push(
                    TokenKind::InterpolationExprEnd,
                    self.span(line, expr_end, expr_end + 1),
                );
                index = expr_end + 1;
                text_start = index;
                continue;
            }

            index += ch.len_utf8();
        }

        self.push_interpolation_text(line, text_start, line.end_byte);
        self.error_at(
            self.span(line, start, line.end_byte),
            LexerDiagnosticReason::UnterminatedInterpolationString,
            "unterminated interpolation string",
        );
        line.end_byte
    }

    fn push_interpolation_text(&mut self, line: Line<'a>, start: usize, end: usize) {
        if start < end {
            self.push(TokenKind::InterpolationText, self.span(line, start, end));
        }
    }

    /// Scan a hole for its closing `}`, returning the byte offset of that brace.
    /// A plain `"..."` string and a nested `$"..."` interpolation are skipped as
    /// self-contained literals so their braces do not terminate the hole; a bare
    /// `{` (an interpolation opener with no `$"`) is not a valid expression, so it
    /// terminates the scan as unterminated. `depth` counts nested interpolation
    /// strings and bounds the recursion at [`NESTING_DEPTH_LIMIT`], so a
    /// pathologically deep nest reports the nesting-limit error at the offending
    /// depth rather than overflowing the stack or masquerading as unterminated.
    fn find_interpolation_expr_end(
        &self,
        line: Line<'a>,
        start: usize,
        depth: usize,
    ) -> Result<usize, InterpolationScanError> {
        if depth > NESTING_DEPTH_LIMIT {
            return Err(InterpolationScanError::NestingLimit(start));
        }
        let mut index = start;
        let mut parens = 0usize;
        let mut brackets = 0usize;
        while index < line.end_byte {
            let tail = &self.source[index..line.end_byte];
            if tail.starts_with("$\"") {
                index = self.find_interpolation_string_end(line, index, depth + 1)?;
                continue;
            }
            // An escaped quote opens a nested string literal, the spelling used
            // inside the enclosing `$"..."`. Skip the whole `\"..\"` so its
            // interior braces, parens, and bare quotes are content, not live
            // tokens that could prematurely close the hole or shift its span.
            if tail.starts_with("\\\"") {
                index = self
                    .escaped_hole_string_end(index, line.end_byte)
                    .ok_or(InterpolationScanError::Unterminated)?;
                continue;
            }
            let Some(ch) = tail.chars().next() else { break };
            match ch {
                '"' => {
                    index = self
                        .find_string_end(line, index)
                        .ok_or(InterpolationScanError::Unterminated)?;
                    continue;
                }
                '{' => return Err(InterpolationScanError::Unterminated),
                '}' if parens == 0 && brackets == 0 => return Ok(index),
                '}' => return Err(InterpolationScanError::Unterminated),
                '(' => parens += 1,
                ')' => parens = parens.saturating_sub(1),
                '[' => brackets += 1,
                ']' => brackets = brackets.saturating_sub(1),
                _ => {}
            }
            index += ch.len_utf8();
        }
        Err(InterpolationScanError::Unterminated)
    }

    /// Skip a nested `$"..."` interpolation literal starting at its `$`, returning
    /// the offset just past its closing quote. Its own holes are scanned in turn,
    /// so an interpolation nested inside a hole is treated as one literal rather
    /// than confusing the outer hole scan.
    fn find_interpolation_string_end(
        &self,
        line: Line<'a>,
        start: usize,
        depth: usize,
    ) -> Result<usize, InterpolationScanError> {
        if depth > NESTING_DEPTH_LIMIT {
            return Err(InterpolationScanError::NestingLimit(start));
        }
        let mut index = start + 2;
        while index < line.end_byte {
            let tail = &self.source[index..line.end_byte];
            if tail.starts_with("{{") || tail.starts_with("}}") {
                index += 2;
                continue;
            }
            let Some(ch) = tail.chars().next() else { break };
            if ch == '\\' {
                index += ch.len_utf8();
                if let Some(next) = self
                    .source
                    .get(index..line.end_byte)
                    .and_then(|tail| tail.chars().next())
                {
                    index += next.len_utf8();
                }
                continue;
            }
            if ch == '"' {
                return Ok(index + 1);
            }
            if ch == '{' {
                let brace = self.find_interpolation_expr_end(line, index + 1, depth)?;
                index = brace + 1;
                continue;
            }
            index += ch.len_utf8();
        }
        Err(InterpolationScanError::Unterminated)
    }

    /// Report interpolation nested past [`NESTING_DEPTH_LIMIT`] as the same
    /// `check.nesting_limit` finding every other over-deep construct reports,
    /// anchored at the over-deep opener rather than the enclosing hole.
    fn report_interpolation_nesting_limit(&mut self, line: Line<'a>, offending: usize) {
        self.diagnostics.push(Diagnostic {
            code: NESTING_LIMIT,
            reason: DiagnosticReason::Parser(ParseDiagnosticReason::NestingLimit),
            severity: Severity::Error,
            message: format!("interpolation nests deeper than the limit of {NESTING_DEPTH_LIMIT}"),
            help: None,
            span: self.span(line, offending, line.end_byte),
        });
    }

    fn find_string_end(&self, line: Line<'a>, start: usize) -> Option<usize> {
        let mut index = start + 1;
        while index < line.end_byte {
            let ch = self.source[index..line.end_byte].chars().next()?;
            index += ch.len_utf8();
            if ch == '\\' {
                if let Some(next) = self.source[index..line.end_byte].chars().next() {
                    index += next.len_utf8();
                }
                continue;
            }
            if ch == '"' {
                return Some(index);
            }
        }
        None
    }

    fn lex_word(&mut self, line: Line<'a>, start: usize) -> usize {
        let end = self.identifier_word_end(start, line.end_byte);
        let text = &self.source[start..end];
        let kind = keyword(text)
            .map(TokenKind::Keyword)
            .unwrap_or(TokenKind::Identifier);
        self.push(kind, self.span(line, start, end));
        end
    }

    fn identifier_word_end(&self, start: usize, line_end: usize) -> usize {
        let mut end = start;
        for (offset, ch) in self.source[start..line_end].char_indices() {
            if !is_identifier_continue_char(ch) {
                break;
            }
            end = start + offset + ch.len_utf8();
        }
        end
    }

    fn lex_number(&mut self, line: Line<'a>, start: usize) -> usize {
        let mut end = start;
        for (offset, ch) in self.source[start..line.end_byte].char_indices() {
            if !ch.is_ascii_digit() {
                break;
            }
            end = start + offset + ch.len_utf8();
        }

        let mut kind = TokenKind::Integer;

        // A dot followed by a known fixed-span unit (`1.day`) is one duration
        // literal. The unit set is closed, so `1.foo` is not a literal: the dot
        // and word fall through to ordinary field-access lexing.
        if self.source[end..line.end_byte].starts_with('.') {
            let unit_start = end + 1;
            let unit_end = self.identifier_word_end(unit_start, line.end_byte);
            if unit_end > unit_start
                && duration_unit_seconds(&self.source[unit_start..unit_end]).is_some()
            {
                self.push(TokenKind::Duration, self.span(line, start, unit_end));
                return unit_end;
            }
        }

        if self.source[end..line.end_byte].starts_with('.')
            && self
                .source
                .get(end + 1..line.end_byte)
                .and_then(|tail| tail.chars().next())
                .is_some_and(|ch| ch.is_ascii_digit())
        {
            kind = TokenKind::Decimal;
            end += 1;
            let mut decimal_end = end;
            for (offset, ch) in self.source[end..line.end_byte].char_indices() {
                if !ch.is_ascii_digit() {
                    break;
                }
                decimal_end = end + offset + ch.len_utf8();
            }
            end = decimal_end;
        }

        self.push(kind, self.span(line, start, end));
        end
    }

    fn push_punctuation(&mut self, kind: TokenKind, span: SourceSpan) {
        match kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => {
                self.open_delimiters += 1;
            }
            TokenKind::RightParen | TokenKind::RightBracket => {
                self.open_delimiters = self.open_delimiters.saturating_sub(1);
            }
            TokenKind::LeftBrace => {
                self.brace_depth += 1;
                if self.brace_depth > NESTING_DEPTH_LIMIT {
                    self.report_brace_nesting_limit(span);
                }
            }
            TokenKind::RightBrace => {
                self.brace_depth = self.brace_depth.saturating_sub(1);
                if self.brace_depth <= NESTING_DEPTH_LIMIT {
                    // Left the over-deep region, so a later independent deep nest
                    // reports its own overflow rather than being silenced.
                    self.reported_nesting_limit = false;
                }
            }
            _ => {}
        }
        self.push(kind, span);
    }

    fn lex_string(
        &mut self,
        line: Line<'a>,
        start: usize,
        quote_offset: usize,
        kind: TokenKind,
    ) -> usize {
        let mut index = start + quote_offset + 1;
        while index < line.end_byte {
            let Some(tail) = self.source.get(index..line.end_byte) else {
                break;
            };
            let Some(ch) = tail.chars().next() else {
                break;
            };
            index += ch.len_utf8();
            if ch == '\\' {
                if let Some(next) = self
                    .source
                    .get(index..line.end_byte)
                    .and_then(|tail| tail.chars().next())
                {
                    index += next.len_utf8();
                }
                continue;
            }
            if ch == '"' {
                self.push(kind, self.span(line, start, index));
                return index;
            }
        }

        self.error_at(
            self.span(line, start, line.end_byte),
            LexerDiagnosticReason::UnterminatedString,
            "unterminated string",
        );
        self.push(kind, self.span(line, start, line.end_byte));
        line.end_byte
    }

    /// Find the byte just past the closing `\"` of an escaped-quote string
    /// opened at `start` (a `\"`), searching within `limit`. A bare `"` is a
    /// literal quote and `\x` an interior escape, so only an unescaped `\"`
    /// closes it; the whole span is one nested string. Returns `None` when no
    /// close appears before `limit`. This is the single owner of the escaped
    /// string's extent, shared by the hole scanner and the hole lexer so both
    /// agree on where the string — and its interior structural characters — end.
    fn escaped_hole_string_end(&self, start: usize, limit: usize) -> Option<usize> {
        let mut index = start + 2;
        while index < limit {
            let tail = self.source.get(index..limit)?;
            if tail.starts_with("\\\"") {
                return Some(index + 2);
            }
            let ch = tail.chars().next()?;
            index += ch.len_utf8();
            if ch == '\\'
                && let Some(next) = self.source.get(index..limit).and_then(|t| t.chars().next())
            {
                index += next.len_utf8();
            }
        }
        None
    }

    /// Lex a nested string literal written with escaped quotes inside an
    /// interpolation hole: opened by `\"` and closed by the next `\"`. The
    /// `String` token spans the whole `\"...\"`, bounded by the hole's `end`, so
    /// [`crate::decode_string_literal`] recovers the value the same way it does
    /// for a plainly quoted literal.
    fn lex_escaped_hole_string(&mut self, line: Line<'a>, start: usize, end: usize) -> usize {
        match self.escaped_hole_string_end(start, end) {
            Some(close) => {
                self.push(TokenKind::String, self.span(line, start, close));
                close
            }
            None => {
                self.error_at(
                    self.span(line, start, end),
                    LexerDiagnosticReason::UnterminatedString,
                    "unterminated string",
                );
                self.push(TokenKind::String, self.span(line, start, end));
                end
            }
        }
    }

    fn punctuation(&self, index: usize, line_end: usize) -> Option<(TokenKind, usize)> {
        let tail = &self.source[index..line_end];
        for (text, kind) in [
            ("::", TokenKind::DoubleColon),
            ("..=", TokenKind::DotDotEqual),
            ("..", TokenKind::DotDot),
            ("=>", TokenKind::FatArrow),
            ("==", TokenKind::EqualEqual),
            ("!=", TokenKind::BangEqual),
            ("?.", TokenKind::QuestionDot),
            ("??", TokenKind::QuestionQuestion),
            ("<=", TokenKind::LessEqual),
            (">=", TokenKind::GreaterEqual),
            ("+=", TokenKind::PlusEqual),
            ("-=", TokenKind::MinusEqual),
            ("*=", TokenKind::StarEqual),
            ("/=", TokenKind::SlashEqual),
            ("%=", TokenKind::PercentEqual),
        ] {
            if tail.starts_with(text) {
                return Some((kind, text.len()));
            }
        }

        let ch = tail.chars().next()?;
        let kind = match ch {
            '(' => TokenKind::LeftParen,
            ')' => TokenKind::RightParen,
            '[' => TokenKind::LeftBracket,
            ']' => TokenKind::RightBracket,
            '{' => TokenKind::LeftBrace,
            '}' => TokenKind::RightBrace,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            '=' => TokenKind::Equal,
            '<' => TokenKind::Less,
            '>' => TokenKind::Greater,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '^' => TokenKind::Caret,
            // A lone `?` is the optional type suffix. The `?.`/`??` table above
            // runs first, so longest match keeps those operators intact.
            '?' => TokenKind::Question,
            _ => return None,
        };
        Some((kind, ch.len_utf8()))
    }

    fn push_newline(&mut self, line: Line<'a>) {
        let Some(end_byte) = self.newline_end_byte(line.end_byte) else {
            return;
        };
        self.push(
            TokenKind::Newline,
            SourceSpan {
                start_byte: line.end_byte,
                end_byte,
                line: line.number,
                column: (line.text.len() + 1) as u32,
            },
        );
    }

    fn newline_end_byte(&self, line_end: usize) -> Option<usize> {
        let rest = self.source.get(line_end..)?;
        if rest.starts_with("\r\n") {
            Some(line_end + 2)
        } else if rest.starts_with('\n') {
            Some(line_end + 1)
        } else {
            None
        }
    }

    fn reject_line_tabs(&mut self, line: Line<'a>) {
        if let Some(tab) = line.text.find('\t') {
            self.error_at(
                SourceSpan {
                    start_byte: line.start_byte + tab,
                    end_byte: line.start_byte + tab + 1,
                    line: line.number,
                    column: (tab + 1) as u32,
                },
                LexerDiagnosticReason::TabIndentation,
                "tabs are not allowed in Marrow source; use spaces",
            );
        }
    }

    fn push(&mut self, kind: TokenKind, span: SourceSpan) {
        self.tokens.push(Token { kind, span });
    }

    fn span(&self, line: Line<'a>, start_byte: usize, end_byte: usize) -> SourceSpan {
        SourceSpan {
            start_byte,
            end_byte,
            line: line.number,
            column: (start_byte - line.start_byte + 1) as u32,
        }
    }

    fn eof_span(&self) -> SourceSpan {
        let line = self
            .lines
            .last()
            .map(|line| {
                if self.source.ends_with('\n') {
                    line.number + 1
                } else {
                    line.number
                }
            })
            .unwrap_or(1);
        SourceSpan {
            start_byte: self.source.len(),
            end_byte: self.source.len(),
            line,
            column: 1,
        }
    }

    fn error_at(
        &mut self,
        span: SourceSpan,
        reason: LexerDiagnosticReason,
        message: impl Into<String>,
    ) {
        self.error_at_with_help(span, reason, message, None);
    }

    fn error_at_with_help(
        &mut self,
        span: SourceSpan,
        reason: LexerDiagnosticReason,
        message: impl Into<String>,
        help: Option<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            reason: DiagnosticReason::Lexer(reason),
            severity: Severity::Error,
            message: message.into(),
            help,
            span,
        });
    }
}

#[derive(Debug, Clone, Copy)]
struct Line<'a> {
    number: u32,
    start_byte: usize,
    end_byte: usize,
    text: &'a str,
    indent: usize,
    content: &'a str,
}

impl<'a> Line<'a> {
    fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }

    fn is_comment(&self) -> bool {
        self.content.starts_with("//") && !self.content.starts_with("///")
    }

    fn doc_comment(&self) -> Option<&'a str> {
        self.content.strip_prefix("///").map(str::trim)
    }

    fn span_at_content(&self) -> SourceSpan {
        SourceSpan {
            start_byte: self.start_byte + self.indent,
            end_byte: self.end_byte,
            line: self.number,
            column: (self.indent + 1) as u32,
        }
    }
}

fn split_lines(source: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut number = 1;

    for segment in source.split_inclusive('\n') {
        let mut text = segment;
        if let Some(stripped) = text.strip_suffix('\n') {
            text = stripped;
        }
        if let Some(stripped) = text.strip_suffix('\r') {
            text = stripped;
        }
        lines.push(make_line(number, start, text));
        start += segment.len();
        number += 1;
    }

    if source.is_empty() || !source.ends_with('\n') {
        let text = &source[start..];
        if !text.is_empty() || source.is_empty() {
            lines.push(make_line(number, start, text));
        }
    }

    lines
}

fn make_line(number: u32, start_byte: usize, text: &str) -> Line<'_> {
    let indent = text.bytes().take_while(|byte| *byte == b' ').count();
    Line {
        number,
        start_byte,
        end_byte: start_byte + text.len(),
        text,
        indent,
        content: &text[indent..],
    }
}
