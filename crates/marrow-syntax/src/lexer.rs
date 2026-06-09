//! The lexer: turns Marrow source text into a flat token stream with
//! file-absolute spans, emitting `INDENT`/`DEDENT`/`NEWLINE` layout tokens and
//! recording lexical diagnostics as it goes.

use crate::token::{
    duration_unit_seconds, is_identifier_continue_char, is_identifier_start_char, keyword,
};
use crate::{
    Diagnostic, DiagnosticReason, LexedSource, LexerDiagnosticReason, ObsoleteOperator,
    PARSE_SYNTAX, Severity, SourceSpan, Token, TokenKind,
};

pub fn lex_source(source: &str) -> LexedSource {
    Lexer::new(source).lex()
}

struct Lexer<'a> {
    source: &'a str,
    lines: Vec<Line<'a>>,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    indents: Vec<usize>,
    open_delimiters: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            lines: split_lines(source),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            indents: vec![0],
            open_delimiters: 0,
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
                let starts_in_delimiters = self.open_delimiters > 0;
                if !starts_in_delimiters {
                    self.apply_comment_indent(line, is_doc_comment);
                }

                let kind = if is_doc_comment {
                    TokenKind::DocComment
                } else {
                    TokenKind::Comment
                };
                self.push(kind, line.span_at_content());
                if !starts_in_delimiters {
                    self.push_newline(line);
                }
                continue;
            }

            let starts_in_delimiters = self.open_delimiters > 0;
            if !starts_in_delimiters {
                self.apply_indent(line);
            }
            self.lex_line(line);
            if self.open_delimiters == 0 {
                self.push_newline(line);
            }
        }

        self.close_indents();
        self.push(TokenKind::Eof, self.eof_span());
        LexedSource {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn apply_indent(&mut self, line: Line<'a>) {
        let current = *self.indents.last().expect("root indent");
        if line.indent > current {
            self.indents.push(line.indent);
            self.push(
                TokenKind::Indent,
                SourceSpan {
                    start_byte: line.start_byte,
                    end_byte: line.start_byte + line.indent,
                    line: line.number,
                    column: 1,
                },
            );
            return;
        }

        if line.indent == current {
            return;
        }

        while self.indents.len() > 1 && line.indent < *self.indents.last().expect("indent stack") {
            self.indents.pop();
            self.push(TokenKind::Dedent, self.empty_span(line, line.indent));
        }

        if line.indent != *self.indents.last().expect("indent stack") {
            self.error_at(
                self.empty_span(line, line.indent),
                LexerDiagnosticReason::IndentationMismatch,
                "indentation does not match an open block",
            );
        }
    }

    fn apply_comment_indent(&mut self, line: Line<'a>, is_doc_comment: bool) {
        let current = *self.indents.last().expect("root indent");
        if is_doc_comment || line.indent >= current {
            self.apply_indent(line);
        }
    }

    fn lex_line(&mut self, line: Line<'a>) {
        self.lex_range(line, line.start_byte + line.indent, line.end_byte);
    }

    fn lex_range(&mut self, line: Line<'a>, start: usize, end: usize) {
        let mut index = start;
        while index < end {
            let ch = self.source[index..line.end_byte]
                .chars()
                .next()
                .expect("line byte index at char boundary");

            if ch == ' ' || ch == '\t' {
                index += ch.len_utf8();
                continue;
            }

            if ch == ';' {
                let kind = if self.source[index..line.end_byte].starts_with(";;") {
                    TokenKind::DocComment
                } else {
                    TokenKind::Comment
                };
                self.push(kind, self.span(line, index, line.end_byte));
                break;
            }

            if ch == '"' {
                index = self.lex_string(line, index, 0, TokenKind::String);
                continue;
            }

            if self.source[index..line.end_byte].starts_with("b\"") {
                index = self.lex_string(line, index, 1, TokenKind::Bytes);
                continue;
            }

            if self.source[index..line.end_byte].starts_with("$\"") {
                index = self.lex_interpolation(line, index);
                continue;
            }

            if ch.is_ascii_digit() {
                index = self.lex_number(line, index);
                continue;
            }

            if is_identifier_start_char(ch) {
                if ch == '_' && !self.identifier_continues_after(index, line.end_byte) {
                    let end = index + ch.len_utf8();
                    self.push(TokenKind::Underscore, self.span(line, index, end));
                    index = end;
                    continue;
                }
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
                "Marrow uses `;` for comments.",
            )
        } else {
            return None;
        };

        let end = index + consumed;
        let span = self.span(line, index, end);
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            reason: DiagnosticReason::Lexer(LexerDiagnosticReason::ObsoleteOperator(reason)),
            severity: Severity::Error,
            message: message.to_string(),
            help: Some(help.to_string()),
            span,
        });
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
            let tail = &self.source[index..line.end_byte];
            if tail.starts_with("{{") || tail.starts_with("}}") {
                index += 2;
                continue;
            }

            let ch = tail
                .chars()
                .next()
                .expect("interpolation byte index at char boundary");

            if ch == '\\' {
                index += ch.len_utf8();
                if let Some(escaped) = self.source[index..line.end_byte].chars().next() {
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

                let Some(expr_end) = self.find_interpolation_expr_end(line, expr_start_end) else {
                    self.error_at(
                        self.span(line, index, line.end_byte),
                        LexerDiagnosticReason::UnterminatedInterpolationExpression,
                        "unterminated interpolation expression",
                    );
                    return line.end_byte;
                };

                self.lex_range(line, expr_start_end, expr_end);
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

    fn find_interpolation_expr_end(&self, line: Line<'a>, start: usize) -> Option<usize> {
        let mut index = start;
        let mut parens = 0usize;
        let mut brackets = 0usize;
        while index < line.end_byte {
            let ch = self.source[index..line.end_byte].chars().next()?;
            match ch {
                '"' => {
                    index = self.find_string_end(line, index, 0)?;
                    continue;
                }
                '{' => return None,
                '}' if parens == 0 && brackets == 0 => return Some(index),
                '}' => return None,
                '(' => parens += 1,
                ')' => parens = parens.saturating_sub(1),
                '[' => brackets += 1,
                ']' => brackets = brackets.saturating_sub(1),
                _ => {}
            }
            index += ch.len_utf8();
        }
        None
    }

    fn find_string_end(&self, line: Line<'a>, start: usize, quote_offset: usize) -> Option<usize> {
        let mut index = start + quote_offset + 1;
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
            let ch = self.source[index..line.end_byte]
                .chars()
                .next()
                .expect("string byte index at char boundary");
            index += ch.len_utf8();
            if ch == '\\' {
                if let Some(next) = self.source[index..line.end_byte].chars().next() {
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

    fn punctuation(&self, index: usize, line_end: usize) -> Option<(TokenKind, usize)> {
        let tail = &self.source[index..line_end];
        for (text, kind) in [
            ("::", TokenKind::DoubleColon),
            ("..=", TokenKind::DotDotEqual),
            ("..", TokenKind::DotDot),
            ("==", TokenKind::EqualEqual),
            ("!=", TokenKind::BangEqual),
            ("?.", TokenKind::QuestionDot),
            ("??", TokenKind::QuestionQuestion),
            ("<=", TokenKind::LessEqual),
            (">=", TokenKind::GreaterEqual),
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
            '@' => TokenKind::At,
            _ => return None,
        };
        Some((kind, ch.len_utf8()))
    }

    fn identifier_continues_after(&self, index: usize, line_end: usize) -> bool {
        self.source
            .get(index + 1..line_end)
            .and_then(|tail| tail.chars().next())
            .is_some_and(is_identifier_continue_char)
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

    fn close_indents(&mut self) {
        while self.indents.len() > 1 {
            self.indents.pop();
            self.push(TokenKind::Dedent, self.eof_span());
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

    fn empty_span(&self, line: Line<'a>, column_offset: usize) -> SourceSpan {
        self.span(
            line,
            line.start_byte + column_offset,
            line.start_byte + column_offset,
        )
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
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            reason: DiagnosticReason::Lexer(reason),
            severity: Severity::Error,
            message: message.into(),
            help: None,
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
        self.content.starts_with(';') && !self.content.starts_with(";;")
    }

    fn doc_comment(&self) -> Option<&'a str> {
        self.content.strip_prefix(";;").map(str::trim)
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
