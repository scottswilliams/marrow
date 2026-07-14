//! The lexer: turns Marrow source text into a flat token stream with
//! file-absolute spans, emitting `INDENT`/`DEDENT`/`NEWLINE` layout tokens and
//! recording lexical diagnostics as it goes.

use crate::token::{
    duration_unit_seconds, is_identifier_continue_char, is_identifier_start_char, keyword,
};
use crate::{
    Diagnostic, DiagnosticReason, LexedSource, LexerDiagnosticReason, NESTING_DEPTH_LIMIT,
    NESTING_LIMIT, ObsoleteOperator, PARSE_SYNTAX, ParseDiagnosticReason, Severity, SourceSpan,
    Token, TokenKind,
};

pub fn lex_source(source: &str) -> LexedSource {
    Lexer::new(source).lex()
}

/// Whether a line opened a block within the layout nesting limit or past it. An
/// over-deep line opens no block and has its content dropped, so the token stream
/// the parser sees is bounded by the limit.
#[derive(PartialEq, Eq)]
enum Indent {
    Within,
    OverDeep,
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
    indents: Vec<usize>,
    open_delimiters: usize,
    /// Set once the layout nesting limit is first crossed, so a run of over-deep
    /// lines reports [`NESTING_LIMIT`] a single time rather than per line.
    reported_nesting_limit: bool,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            lines: split_lines(source),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            indents: Vec::new(),
            open_delimiters: 0,
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
                let starts_in_delimiters = self.open_delimiters > 0;
                let next_indent = self.next_significant_indent(index);
                if !starts_in_delimiters
                    && self.apply_comment_indent(line, is_doc_comment, next_indent)
                        == Indent::OverDeep
                {
                    continue;
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
            if !starts_in_delimiters && self.apply_indent(line) == Indent::OverDeep {
                // The line nests past the layout limit. Its content is dropped so
                // the token stream — and the AST and every later walk over it —
                // stays bounded by the limit no matter how deep the source goes.
                continue;
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

    /// Open or close indented blocks to match `line`, returning whether the line
    /// nests past the layout limit. A line deeper than the limit opens no further
    /// block: it reports [`NESTING_LIMIT`] once at the limit and is reported
    /// [`Indent::OverDeep`] so the caller drops its content, keeping the token
    /// stream bounded by the limit regardless of source depth. The indent stack
    /// stays at the limit's width while the over-deep region lasts, so a return to
    /// or below that width resumes normal layout.
    fn apply_indent(&mut self, line: Line<'a>) -> Indent {
        let current = self.current_indent();
        if line.indent > current {
            // The outermost declaration body is one indent above the file's
            // top level and is not itself a nesting level, so the stack may hold
            // that body plus [`NESTING_DEPTH_LIMIT`] nested blocks before a deeper
            // block fails closed.
            if self.indents.len() > NESTING_DEPTH_LIMIT {
                self.report_nesting_limit(line);
                return Indent::OverDeep;
            }
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
            return Indent::Within;
        }

        if line.indent == current {
            return Indent::Within;
        }

        while line.indent < self.current_indent() {
            self.indents.pop();
            self.push(TokenKind::Dedent, self.empty_span(line, line.indent));
        }
        if self.indents.len() <= NESTING_DEPTH_LIMIT {
            // Left the over-deep region, so a later independent deep nest reports
            // its own overflow rather than being silenced by the earlier one.
            self.reported_nesting_limit = false;
        }

        if line.indent != self.current_indent() {
            self.error_at(
                self.empty_span(line, line.indent),
                LexerDiagnosticReason::IndentationMismatch,
                "indentation does not match an open block",
            );
        }
        Indent::Within
    }

    /// Report the layout nesting overflow once, at the first line that would open
    /// a block past the limit. Suppressed while the over-deep region lasts so a
    /// run of deeper lines yields a single diagnostic, not one per line.
    fn report_nesting_limit(&mut self, line: Line<'a>) {
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
            span: SourceSpan {
                start_byte: line.start_byte + line.indent,
                end_byte: line.end_byte,
                line: line.number,
                column: (line.indent + 1) as u32,
            },
        });
    }

    /// Decide how a comment-only line interacts with the indent stack. A comment
    /// run is classified by the next NON-COMMENT significant line, so it docks
    /// where the construct it introduces docks, regardless of the comments' own
    /// indentation. A line comment outdented below the current block is otherwise
    /// transparent — kept inside the open block as trailing trivia rather than
    /// closing it.
    ///
    /// When the construct the run introduces is at the file's top level the run
    /// belongs at the top level: a top-level comment is not the body of any
    /// declaration. So an indented comment whose run resolves to the top level
    /// stays transparent rather than opening a spurious block above it, and a
    /// column-zero run that follows an open block closes that block down to the
    /// top level. Likewise an over-indented comment whose run resolves back to the
    /// current block is trivia inside that block and opens no spurious INDENT.
    fn apply_comment_indent(
        &mut self,
        line: Line<'a>,
        is_doc_comment: bool,
        next_indent: Option<usize>,
    ) -> Indent {
        let current = self.current_indent();
        let run_at_top_level = matches!(next_indent, Some(0) | None);
        if current == 0 {
            // No open block to dock into: a top-level run is transparent and a
            // run that opens an indented body below drives the normal logic.
            return if run_at_top_level {
                Indent::Within
            } else {
                self.apply_indent(line)
            };
        }
        // A comment run docks where the construct it introduces docks, so an
        // over-indented comment whose run resolves back to (or below) the current
        // block is layout trivia: it stays inside the block and opens no INDENT.
        if line.indent > current && next_indent.is_none_or(|indent| indent <= current) {
            return Indent::Within;
        }
        let introduces_top_level_decl = line.indent == 0 && next_indent == Some(0);
        if is_doc_comment || line.indent >= current || introduces_top_level_decl {
            self.apply_indent(line)
        } else {
            Indent::Within
        }
    }

    /// The indentation of the next non-blank, non-comment line after `index`, or
    /// `None` when only blank or comment lines remain. A run of column-zero
    /// comments is classified by the declaration that follows the whole run, so
    /// comment lines are skipped: a column-zero comment followed by another
    /// column-zero comment looks past both to whatever the run introduces.
    fn next_significant_indent(&self, index: usize) -> Option<usize> {
        self.lines[index + 1..]
            .iter()
            .find(|line| !line.is_blank() && !line.is_comment() && line.doc_comment().is_none())
            .map(|line| line.indent)
    }

    fn current_indent(&self) -> usize {
        self.indents.last().copied().unwrap_or(0)
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

            if ch == ';' {
                let kind = if tail.starts_with(";;") {
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
                "Marrow uses `;` for comments.",
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
                if let Some(escaped) = self
                    .source
                    .get(index..line.end_byte)
                    .and_then(|tail| tail.chars().next())
                {
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

    fn close_indents(&mut self) {
        while !self.indents.is_empty() {
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
