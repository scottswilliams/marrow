//! The declaration and statement parsers: a recursive-descent parser over the
//! file-wide token stream that frames resource, enum, and function bodies, and
//! the statement parser it delegates to. Together with the free token-walking
//! helpers they structure everything above the expression level.

use crate::parse_expr::{ExprParser, join_spans};
use crate::token::{is_identifier, is_qualified_name, is_type_text, keyword, tokens_in_range};
use crate::*;

struct FunctionHead {
    public: bool,
    name: String,
    params: Vec<ParamDecl>,
    return_type: Option<TypeRef>,
}

enum MemberHead {
    Field {
        required: bool,
        name: String,
        keys: Vec<KeyParam>,
        ty: TypeRef,
    },
    Group {
        name: String,
        keys: Vec<KeyParam>,
    },
}

/// Statement keywords that introduce one or more indented blocks. Most have
/// dedicated parsers; this guards the fallback that swallows a block-introducing
/// keyword appearing where it cannot be structured (such as a stray `else`),
/// reporting it and consuming its nested block so following statements still
/// parse.
fn is_compound_statement_keyword(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::If
            | Keyword::Else
            | Keyword::While
            | Keyword::For
            | Keyword::Transaction
            | Keyword::Lock
            | Keyword::Try
            | Keyword::Catch
            | Keyword::Finally
    )
}

/// Parses the statements of a function body over the file-wide token stream.
/// It keeps layout tokens (`NEWLINE`, `INDENT`, `DEDENT`) so statements that
/// span several physical lines inside open delimiters are one statement, and
/// delegates expression parsing to `ExprParser`.
struct StmtParser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    /// Ordinary `;` comments for the block currently being parsed, in source
    /// order. Each nested block swaps in a fresh accumulator (see
    /// `parse_nested_block`) so a comment lands in the block it appears in.
    comments: Vec<Comment>,
    /// Parse errors for statement lines the body parser cannot structure, so a
    /// malformed statement becomes a deterministic diagnostic instead of being
    /// silently accepted.
    diagnostics: Vec<Diagnostic>,
}

impl<'a> StmtParser<'a> {
    fn new(source: &'a str, tokens: &[Token]) -> Self {
        // Drop doc comments (they attach to declarations, not body statements)
        // but keep ordinary `;` comments in the stream: they are collected as
        // block trivia during parsing so the formatter can re-emit them.
        let tokens = tokens
            .iter()
            .copied()
            .filter(|token| token.kind != TokenKind::DocComment)
            .collect();
        Self {
            source,
            tokens,
            pos: 0,
            comments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn parse_block(mut self) -> (Vec<Statement>, Vec<Comment>, Vec<Diagnostic>) {
        // A body opens with the INDENT that began it.
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
        ));
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
    }

    fn statements(&mut self) -> Vec<Statement> {
        let mut statements = Vec::new();
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Dedent => break,
                TokenKind::Newline => {
                    self.advance();
                }
                TokenKind::Comment => self.take_own_line_comment(),
                TokenKind::Indent => {
                    // A stray nested block (e.g. under a swallowed compound
                    // statement). Skip it rather than mis-parse.
                    self.skip_block();
                }
                _ => statements.extend(self.statement()),
            }
        }
        statements
    }

    fn peek(&self) -> Option<TokenKind> {
        self.tokens.get(self.pos).map(|token| token.kind)
    }

    fn advance(&mut self) -> Token {
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
            TokenKind::Keyword(Keyword::Lock) => return Some(self.lock_stmt()),
            TokenKind::Keyword(Keyword::Try) => return Some(self.try_stmt()),
            TokenKind::Keyword(Keyword::Match) => return Some(self.match_stmt()),
            TokenKind::Keyword(keyword) if is_compound_statement_keyword(keyword) => {
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
        // The generic fallback only fires when nothing more specific was raised
        // for the line: a keyword field name or another inline syntax-rule
        // diagnostic already explains the failure, and a single line reports once.
        if statement.is_none() && self.diagnostics.len() == before {
            let span = line_span(&self.tokens[self.pos..content_end]);
            self.error_span(span, "expected a statement");
        }
        self.pos = (newline + 1).min(self.tokens.len());
        statement
    }

    /// If the token just before `line_end` is a trailing `;` comment, record it
    /// as a `Trailing` comment for the current block and return the index that
    /// excludes it; otherwise return `line_end` unchanged. `line_end` is the
    /// index of the `NEWLINE`/`INDENT`/`DEDENT` that ends the current line.
    fn split_trailing_comment(&mut self, line_end: usize) -> usize {
        if line_end > self.pos && self.tokens[line_end - 1].kind == TokenKind::Comment {
            let token = self.tokens[line_end - 1];
            self.comments.push(comment_from_token(
                self.source,
                token,
                CommentPlacement::Trailing,
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
                self.error_span(header_span, "expected `for <binding> in <iterable>`");
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

    fn lock_stmt(&mut self) -> Statement {
        let start = self.advance().span; // `lock`
        let path = self.header_expression();
        let body = self.block_body();
        Statement::Lock {
            span: join_spans(start, body.span),
            path,
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
                    TokenKind::Comment => self.take_own_line_comment(),
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
            self.error_span(start, "expected an indented match body");
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
            self.error_span(span, "a match arm is a member path relative to the enum");
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

    /// Parse the expression that ends the current header line (an `if`/`while`
    /// condition or `lock` path), consuming up to and including its `NEWLINE`.
    /// Returns `None`, after raising a syntax error, when the header does not
    /// parse as a complete expression.
    fn header_expression(&mut self) -> Option<Expression> {
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let before = self.diagnostics.len();
        let expr = expr_of(self.source, line, &mut self.diagnostics);
        // The generic fallback only fires when nothing more specific was raised:
        // a keyword field name or another inline syntax-rule diagnostic already
        // explains the failure, so a single header reports once.
        if expr.is_none() && self.diagnostics.len() == before {
            self.error_span(
                line_span(&self.tokens[self.pos..content_end]),
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
                TokenKind::Comment => {
                    let token = self.advance();
                    self.comments.push(comment_from_token(
                        self.source,
                        token,
                        CommentPlacement::Trailing,
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
        let mut index = self.pos;
        while index < self.tokens.len()
            && !matches!(
                self.tokens[index].kind,
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
            )
        {
            index += 1;
        }
        index
    }

    /// A block-introducing keyword (such as a stray `else`) appearing where it
    /// cannot be structured. Report it and consume its header and nested block
    /// so the following statements still parse.
    fn skip_compound(&mut self) {
        let start = self.tokens[self.pos].span;
        let mut end = start;
        // Consume the header up to its NEWLINE.
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
        // Consume an immediately following indented block, if any.
        if matches!(self.peek(), Some(TokenKind::Indent)) {
            end = self.skip_block();
        }
        self.error_span(join_spans(start, end), "expected a statement");
    }

    fn error_span(&mut self, span: SourceSpan, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            kind: "parse",
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span,
        });
    }

    /// Consume a balanced `INDENT … DEDENT` run starting at the current
    /// `INDENT`, returning the span of the last token consumed. Tolerates a
    /// missing trailing `DEDENT` at the end of the body token slice.
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

fn parse_simple_statement(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let first = line.first()?;
    match first.kind {
        TokenKind::Keyword(Keyword::Const) => parse_const_or_var(source, line, false, diagnostics),
        TokenKind::Keyword(Keyword::Var) => parse_const_or_var(source, line, true, diagnostics),
        TokenKind::Keyword(Keyword::Return) => parse_return(source, line, diagnostics),
        TokenKind::Keyword(Keyword::Delete) => {
            let value = expr_of(source, &line[1..], diagnostics)?;
            Some(Statement::Delete {
                span: join_spans(first.span, value.span()),
                path: value,
            })
        }
        TokenKind::Keyword(Keyword::Throw) => {
            let value = expr_of(source, &line[1..], diagnostics)?;
            Some(Statement::Throw {
                span: join_spans(first.span, value.span()),
                value,
            })
        }
        TokenKind::Keyword(Keyword::Merge) => parse_merge(source, line, diagnostics),
        TokenKind::Keyword(Keyword::Break) => parse_break_or_continue(source, line, true),
        TokenKind::Keyword(Keyword::Continue) => parse_break_or_continue(source, line, false),
        _ => parse_assign_or_expr(source, line, diagnostics),
    }
}

/// Recursive-descent parser for top-level declarations over the file-wide token
/// stream, the same stream `StmtParser`/`ExprParser` consume. It dispatches on
/// token shape, frames resource and function bodies by `INDENT`/`DEDENT` tokens,
/// and delegates statement and expression parsing to those parsers. A
/// declaration spans its whole first physical line at column 1.
pub(crate) struct DeclParser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> DeclParser<'a> {
    pub(crate) fn new(source: &'a str, tokens: &'a [Token]) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn parse(mut self) -> ParsedSource {
        let mut file = SourceFile::default();
        let mut docs = Vec::new();
        let mut saw_top_level_item = false;

        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Newline => {
                    self.advance();
                }
                TokenKind::Comment => {
                    self.advance();
                }
                TokenKind::DocComment => {
                    let token = self.advance();
                    docs.push(doc_comment_text(token.text(self.source)));
                }
                TokenKind::Eof => break,
                // Indentation where a top-level declaration was expected: report
                // each stray indented line.
                TokenKind::Indent => {
                    self.report_stray_indented_lines();
                    saw_top_level_item = true;
                }
                TokenKind::Dedent => {
                    self.advance();
                }
                // Each declaration keyword introduces its kind only when followed
                // by a space. A bare keyword (or one glued to the next token,
                // like `module::x`) is an unknown top-level declaration.
                TokenKind::Keyword(Keyword::Module) if self.keyword_introduces_decl() => {
                    self.parse_module(&mut file, &mut docs, saw_top_level_item);
                    saw_top_level_item = true;
                }
                TokenKind::Keyword(Keyword::Use) if self.keyword_introduces_decl() => {
                    self.parse_use(&mut file);
                    docs.clear();
                    saw_top_level_item = true;
                }
                TokenKind::Keyword(Keyword::Const) if self.keyword_introduces_decl() => {
                    let decl = self.parse_const(std::mem::take(&mut docs));
                    file.declarations.push(Declaration::Const(decl));
                    saw_top_level_item = true;
                }
                TokenKind::Keyword(Keyword::Resource) if self.keyword_introduces_decl() => {
                    let resource = self.parse_resource(std::mem::take(&mut docs));
                    file.declarations.push(Declaration::Resource(resource));
                    saw_top_level_item = true;
                }
                _ if self.starts_enum_header() => {
                    let decl = self.parse_enum(std::mem::take(&mut docs));
                    file.declarations.push(Declaration::Enum(decl));
                    saw_top_level_item = true;
                }
                _ if self.starts_function_header() => {
                    let function = self.parse_function(std::mem::take(&mut docs));
                    file.declarations.push(Declaration::Function(function));
                    saw_top_level_item = true;
                }
                // `type` is not a keyword in Marrow; it lexes as an identifier.
                TokenKind::Identifier
                    if self.identifier_is(self.pos, "type") && self.keyword_introduces_decl() =>
                {
                    self.error_header(
                    "type aliases are not used in Marrow; declare a resource or use a builtin type directly",
                );
                    docs.clear();
                    saw_top_level_item = true;
                }
                _ => {
                    self.error_header("expected module, use, const, resource, or fn declaration");
                    docs.clear();
                    saw_top_level_item = true;
                }
            }
        }

        ParsedSource {
            file,
            diagnostics: self.diagnostics,
        }
    }

    fn parse_module(
        &mut self,
        file: &mut SourceFile,
        docs: &mut Vec<String>,
        saw_top_level_item: bool,
    ) {
        let span = self.header_span();
        let header = self.take_header_line();
        let name = qualified_name(self.source, &header[1..]);
        if saw_top_level_item {
            self.error_span(
                span,
                "module declaration must appear once at the start of the file",
            );
        } else if let Some(name) = name {
            file.module = Some(ModuleDecl { name, span });
        } else {
            self.error_span(span, "expected qualified module name");
        }
        docs.clear();
    }

    fn parse_use(&mut self, file: &mut SourceFile) {
        let span = self.header_span();
        let header = self.take_header_line();
        if let Some(name) = qualified_name(self.source, &header[1..]) {
            file.uses.push(UseDecl { name, span });
        } else {
            self.error_span(span, "expected qualified import name");
        }
    }

    fn parse_const(&mut self, docs: Vec<String>) -> ConstDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        // `const Name [: type] = value`. The name is the identifier after the
        // keyword; the type runs from `:` to `=`; the value is everything after.
        let equal = find_top_level_equal(&header[1..]).map(|index| index + 1);
        let (name, ty, value) = match equal {
            Some(equal) => {
                let head = &header[1..equal];
                let value_tokens = &header[equal + 1..];
                // A missing value is reported before checking the name and
                // type, so its diagnostic sorts first on the line.
                if value_tokens.is_empty() {
                    self.error_span(span, "const declarations require a value after `=`");
                }
                let (name, ty) = self.const_name_type(span, head);
                let value = self.value_expression(value_tokens);
                (name, ty, value)
            }
            None => {
                self.error_span(span, "const declarations require `=` and a value");
                let (name, ty) = self.const_name_type(span, &header[1..]);
                (name, ty, None)
            }
        };
        ConstDecl {
            docs,
            name,
            ty,
            value,
            span,
        }
    }

    /// Split a const head (`Name` or `Name: type`) into the name and optional
    /// type, reporting a non-identifier name or malformed type annotation.
    fn const_name_type(&mut self, span: SourceSpan, head: &[Token]) -> (String, Option<TypeRef>) {
        let colon = head.iter().position(|token| token.kind == TokenKind::Colon);
        let (name_tokens, type_tokens) = match colon {
            Some(index) => (&head[..index], Some(&head[index + 1..])),
            None => (head, None),
        };
        // The name is the verbatim text before any `:`, kept even when invalid so
        // the declaration still carries a name; only a non-identifier reports.
        let name = match name_tokens.first().zip(name_tokens.last()) {
            Some((first, last)) => self.source[first.span.start_byte..last.span.end_byte]
                .trim()
                .to_string(),
            None => String::new(),
        };
        // A reserved word is not an identifier (per the grammar), so it cannot name
        // a const any more than it can name a param, member, or key.
        if keyword(&name).is_some() {
            self.error_span(
                span,
                format!("`{name}` is a keyword and cannot be used as a const name"),
            );
        } else if !is_identifier(&name) {
            self.error_span(span, "expected const name before type annotation");
        }
        let ty = match type_tokens {
            Some(tokens) if !tokens.is_empty() => {
                let ty = type_ref_from_tokens(self.source, tokens);
                if !is_type_text(&ty.text) {
                    self.error_span(span, "expected const type annotation");
                    None
                } else {
                    Some(ty)
                }
            }
            Some(_) => {
                self.error_span(span, "expected const type annotation");
                None
            }
            None => None,
        };
        (name, ty)
    }

    fn parse_resource(&mut self, docs: Vec<String>) -> ResourceDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (name, store) = match parse_resource_head(self.source, &header[1..]) {
            Ok(parsed) => parsed,
            Err(message) => {
                self.error_span(span, message);
                (String::new(), None)
            }
        };
        let members = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_resource_members()
        } else {
            self.error_span(span, "expected an indented resource body");
            Vec::new()
        };
        ResourceDecl {
            docs,
            name,
            store,
            members,
            span,
        }
    }

    /// Parse an `INDENT … DEDENT` block of resource members. Nested groups recurse
    /// on their own child block. Each member's span is its whole header line.
    fn parse_resource_members(&mut self) -> Vec<ResourceMember> {
        let mut members = Vec::new();
        let mut docs = Vec::new();
        let mut stable_id = None;
        self.advance(); // INDENT

        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Dedent => {
                    self.advance();
                    break;
                }
                TokenKind::Newline | TokenKind::Comment => {
                    self.advance();
                }
                TokenKind::DocComment => {
                    let token = self.advance();
                    docs.push(doc_comment_text(token.text(self.source)));
                }
                // A deeper indent under a field (rather than a group) is stray:
                // report at the deeper line's content and skip the whole block.
                TokenKind::Indent => {
                    self.advance(); // INDENT
                    if self.peek().is_some_and(|kind| {
                        !matches!(
                            kind,
                            TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
                        )
                    }) {
                        let err = self.content_span();
                        self.error_span(
                        err,
                        "unexpected indentation in resource body; only groups introduce nested resource members",
                    );
                    }
                    self.skip_to_block_end();
                }
                _ => {
                    // The node carries the whole-line span (column 1); a member
                    // error points at the content after the indentation.
                    let span = self.header_span();
                    let err = self.content_span();
                    if matches!(kind, TokenKind::At) {
                        match parse_stable_id_tokens(self.source, &self.take_header_line()) {
                            Some(id) => stable_id = Some(id),
                            None => {
                                self.error_span(err, "expected @id(\"stable.id\")");
                            }
                        }
                        continue;
                    }
                    if matches!(kind, TokenKind::Keyword(Keyword::Index)) {
                        let header = self.take_header_line();
                        match parse_index_tokens(self.source, &header[1..]) {
                            Ok(index) => members.push(ResourceMember::Index(IndexDecl {
                                docs: std::mem::take(&mut docs),
                                stable_id: stable_id.take(),
                                span,
                                ..index
                            })),
                            Err(message) => self.error_span(err, message),
                        }
                        continue;
                    }
                    let header = self.take_header_line();
                    match parse_field_or_group_tokens(self.source, &header) {
                        Ok(MemberHead::Field {
                            required,
                            name,
                            keys,
                            ty,
                        }) => {
                            if !is_type_text(&ty.text) {
                                self.error_span(err, "expected field type annotation");
                            }
                            members.push(ResourceMember::Field(FieldDecl {
                                docs: std::mem::take(&mut docs),
                                stable_id: stable_id.take(),
                                required,
                                name,
                                keys,
                                ty,
                                span,
                            }));
                        }
                        Ok(MemberHead::Group { name, keys }) => {
                            let children = if matches!(self.peek(), Some(TokenKind::Indent)) {
                                self.parse_resource_members()
                            } else {
                                self.error_span(err, "expected an indented resource group body");
                                Vec::new()
                            };
                            members.push(ResourceMember::Group(GroupDecl {
                                docs: std::mem::take(&mut docs),
                                stable_id: stable_id.take(),
                                name,
                                keys,
                                members: children,
                                span,
                            }));
                        }
                        Err(message) => self.error_span(err, message),
                    }
                }
            }
        }
        members
    }

    fn parse_enum(&mut self, docs: Vec<String>) -> EnumDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (public, name) = match parse_enum_head(self.source, &header) {
            Ok(parsed) => parsed,
            Err(message) => {
                self.error_span(span, message);
                (false, String::new())
            }
        };
        let members = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_enum_members()
        } else {
            self.error_span(span, "expected an indented enum body");
            Vec::new()
        };
        if members.is_empty() {
            self.error_span(span, "an enum needs at least one member");
        }
        EnumDecl {
            docs,
            public,
            name,
            members,
            span,
        }
    }

    /// Parse an `INDENT … DEDENT` block of enum members. A member is a bare
    /// identifier on its own line; anything else (a type annotation, key
    /// parameters, an `@id`, or a deeper indent) is a parse error. This mirrors
    /// `parse_resource_members` but accepts only the bare-name form.
    fn parse_enum_members(&mut self) -> Vec<EnumMember> {
        let mut members = Vec::new();
        let mut docs = Vec::new();
        self.advance(); // INDENT

        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Dedent => {
                    self.advance();
                    break;
                }
                TokenKind::Newline | TokenKind::Comment => {
                    self.advance();
                }
                TokenKind::DocComment => {
                    let token = self.advance();
                    docs.push(doc_comment_text(token.text(self.source)));
                }
                // An indent that opens before any member on this level is stray:
                // there is no member header for it to nest under. A member's own
                // nested block is consumed right after its header, below.
                TokenKind::Indent => {
                    self.advance(); // INDENT
                    if self.peek().is_some_and(|kind| {
                        !matches!(
                            kind,
                            TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
                        )
                    }) {
                        let err = self.content_span();
                        self.error_span(err, "an enum member has no nested body");
                    }
                    self.skip_to_block_end();
                }
                _ => {
                    let span = self.header_span();
                    let err = self.content_span();
                    let header = self.take_header_line();
                    match enum_member_name(self.source, &header) {
                        Ok((name, category)) => {
                            // A member's children are the indented block that
                            // immediately follows its header, parsed by the same
                            // routine and attached, so members nest to any depth.
                            let nested = if matches!(self.peek(), Some(TokenKind::Indent)) {
                                self.parse_enum_members()
                            } else {
                                Vec::new()
                            };
                            members.push(EnumMember {
                                docs: std::mem::take(&mut docs),
                                stable_id: None,
                                name,
                                category,
                                members: nested,
                                span,
                            });
                        }
                        Err(message) => self.error_span(err, message),
                    }
                }
            }
        }
        members
    }

    fn parse_function(&mut self, docs: Vec<String>) -> FunctionDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let head = match parse_function_head(self.source, &header) {
            Ok(head) => head,
            Err(message) => {
                self.error_span(span, message);
                FunctionHead {
                    public: false,
                    name: String::new(),
                    params: Vec::new(),
                    return_type: None,
                }
            }
        };
        let body = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_function_body()
        } else {
            self.error_span(span, "expected an indented function body");
            Block {
                statements: Vec::new(),
                comments: Vec::new(),
                span,
            }
        };
        FunctionDecl {
            docs,
            public: head.public,
            name: head.name,
            params: head.params,
            return_type: head.return_type,
            body,
            span,
        }
    }

    /// Parse a function body from its `INDENT … DEDENT` run via the statement
    /// parser. The body span runs from the first body line at column 1 to the end
    /// of the last physical line of the body.
    fn parse_function_body(&mut self) -> Block {
        let indent = self.tokens[self.pos];
        let start = self.pos;
        let end = self.consume_block();
        // The block ends just before the line that closed it; that line is where
        // the matching `DEDENT` sits (or end-of-file for the final body).
        let closing = self.tokens[start..end]
            .iter()
            .rev()
            .find(|token| token.kind == TokenKind::Dedent)
            .map(|dedent| dedent.span.start_byte - (dedent.span.column as usize - 1))
            .unwrap_or_else(|| self.source.len());
        let span = SourceSpan {
            start_byte: indent.span.start_byte,
            end_byte: line_text_end_before(self.source, closing),
            line: indent.span.line,
            column: 1,
        };
        // Feed the statement parser a byte-bounded slice: tokens inside the body
        // span, so a `DEDENT` emitted past the last body line (at end of file) is
        // excluded and nested-block spans stay anchored to the source.
        let body_tokens = tokens_in_range(self.tokens, span.start_byte, span.end_byte);
        let (statements, comments, diagnostics) =
            StmtParser::new(self.source, body_tokens).parse_block();
        self.diagnostics.extend(diagnostics);
        Block {
            statements,
            comments,
            span,
        }
    }

    /// Parse a value-position expression. Returns `None` when the value text
    /// does not parse as a complete expression. An absent value is already
    /// reported by the caller, so only a present-but-malformed value raises a
    /// diagnostic here (a type spelling such as `int` in value position lands
    /// here, where it is a syntax error rather than a silent acceptance).
    fn value_expression(&mut self, tokens: &[Token]) -> Option<Expression> {
        if tokens.is_empty() {
            return None;
        }
        let before = self.diagnostics.len();
        let parsed = ExprParser::new(self.source, tokens).parse_complete(&mut self.diagnostics);
        // The generic fallback only fires when nothing more specific was raised:
        // a keyword field name or another inline syntax-rule diagnostic already
        // explains the failure, so a single value reports once.
        if parsed.is_none() && self.diagnostics.len() == before {
            self.error_span(value_span(tokens), "expected an expression");
        }
        parsed
    }

    /// Collect the tokens of the current header line (up to the next
    /// `NEWLINE`/`INDENT`/`DEDENT`/`EOF`) and advance past the closing `NEWLINE`.
    /// A header line continues across newlines suppressed inside open delimiters,
    /// so a multi-line const value stays one header line.
    fn take_header_line(&mut self) -> Vec<Token> {
        let end = self.header_end();
        let line = self.tokens[self.pos..end].to_vec();
        self.pos = end;
        if matches!(self.peek(), Some(TokenKind::Newline)) {
            self.advance();
        }
        line
    }

    fn header_end(&self) -> usize {
        let mut index = self.pos;
        while index < self.tokens.len()
            && !matches!(
                self.tokens[index].kind,
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
            )
        {
            index += 1;
        }
        index
    }

    /// The span of the current declaration's first physical line at column 1.
    /// The line starts before any indentation, which a token's `column` recovers
    /// as the byte offset from the line start. This is the span stored on
    /// declaration and resource-member nodes.
    fn header_span(&self) -> SourceSpan {
        let token = self.tokens[self.pos];
        let start_byte = token.span.start_byte - (token.span.column as usize - 1);
        SourceSpan {
            start_byte,
            end_byte: first_line_end(self.source, start_byte),
            line: token.span.line,
            column: 1,
        }
    }

    /// The span of the current line's content, starting after its indentation.
    /// Declaration and member error diagnostics point here, at the first
    /// non-space column.
    fn content_span(&self) -> SourceSpan {
        let token = self.tokens[self.pos];
        SourceSpan {
            start_byte: token.span.start_byte,
            end_byte: first_line_end(self.source, token.span.start_byte),
            line: token.span.line,
            column: token.span.column,
        }
    }

    /// Consume a balanced `INDENT … DEDENT` run starting at the current `INDENT`,
    /// returning the exclusive index just past the matching `DEDENT`.
    fn consume_block(&mut self) -> usize {
        let mut depth = 0usize;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Indent => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::Dedent => {
                    self.advance();
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {
                    self.advance();
                }
            }
        }
        self.pos
    }

    /// Consume the rest of an indented block whose opening `INDENT` was already
    /// advanced, stopping after its matching `DEDENT`.
    fn skip_to_block_end(&mut self) {
        let mut depth = 1usize;
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Indent => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::Dedent => {
                    self.advance();
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// Report one "expected a top-level declaration" per content line of a stray
    /// indented region at the top level, each at its content position. Blank and
    /// comment-only lines produce no tokens and so raise nothing.
    fn report_stray_indented_lines(&mut self) {
        let start = self.pos;
        let end = self.consume_block();
        let mut index = start;
        let mut at_line_start = true;
        while index < end {
            let token = self.tokens[index];
            match token.kind {
                TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline => at_line_start = true,
                TokenKind::Comment | TokenKind::DocComment => at_line_start = false,
                _ => {
                    if at_line_start {
                        let line_start = token.span.start_byte - (token.span.column as usize - 1);
                        let content_start = token.span.start_byte;
                        let span = SourceSpan {
                            start_byte: content_start,
                            end_byte: first_line_end(self.source, line_start),
                            line: token.span.line,
                            column: token.span.column,
                        };
                        self.error_span(span, "expected a top-level declaration");
                    }
                    at_line_start = false;
                }
            }
            index += 1;
        }
    }

    fn peek(&self) -> Option<TokenKind> {
        self.tokens.get(self.pos).map(|token| token.kind)
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos];
        self.pos += 1;
        token
    }

    fn identifier_is(&self, index: usize, text: &str) -> bool {
        self.tokens.get(index).is_some_and(|token| {
            token.kind == TokenKind::Identifier && token.text(self.source) == text
        })
    }

    /// Whether the source byte immediately after `token` is a space. A keyword
    /// introduces a declaration only when a space follows it, so `module x` is a
    /// module declaration but `module::x` is a name path.
    fn space_after(&self, token: Token) -> bool {
        self.source.as_bytes().get(token.span.end_byte) == Some(&b' ')
    }

    fn keyword_introduces_decl(&self) -> bool {
        self.space_after(self.tokens[self.pos])
    }

    /// Whether the current line is a function header: `fn `, `pub fn `,
    /// `internal fn `, or `private fn ` (the visibility words being plain
    /// identifiers). The trailing-space rule applies to each word.
    fn starts_function_header(&self) -> bool {
        let lead = self.tokens[self.pos];
        match lead.kind {
            TokenKind::Keyword(Keyword::Fn) => self.space_after(lead),
            TokenKind::Keyword(Keyword::Pub) => {
                self.space_after(lead) && self.followed_by_fn_space()
            }
            TokenKind::Identifier
                if lead.text(self.source) == "internal" || lead.text(self.source) == "private" =>
            {
                self.space_after(lead) && self.followed_by_fn_space()
            }
            _ => false,
        }
    }

    fn followed_by_fn_space(&self) -> bool {
        self.tokens.get(self.pos + 1).is_some_and(|token| {
            token.kind == TokenKind::Keyword(Keyword::Fn) && self.space_after(*token)
        })
    }

    /// Whether the current line is an enum header: `enum ` or `pub enum `. The
    /// trailing-space rule applies to each word, matching `pub fn`.
    fn starts_enum_header(&self) -> bool {
        let lead = self.tokens[self.pos];
        match lead.kind {
            TokenKind::Keyword(Keyword::Enum) => self.space_after(lead),
            TokenKind::Keyword(Keyword::Pub) => {
                self.space_after(lead) && self.followed_by_enum_space()
            }
            _ => false,
        }
    }

    fn followed_by_enum_space(&self) -> bool {
        self.tokens.get(self.pos + 1).is_some_and(|token| {
            token.kind == TokenKind::Keyword(Keyword::Enum) && self.space_after(*token)
        })
    }

    /// Report an error spanning the current header line.
    fn error_header(&mut self, message: impl Into<String>) {
        let span = self.header_span();
        self.take_header_line();
        self.error_span(span, message);
    }

    fn error_span(&mut self, span: SourceSpan, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            kind: "parse",
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span,
        });
    }
}

/// The end byte of the physical line containing `start`, excluding the trailing
/// `\r`/`\n`. This matches `Line::end_byte` for a declaration's first line.
fn first_line_end(source: &str, start: usize) -> usize {
    let tail = &source[start..];
    let break_at = tail
        .find('\n')
        .map(|index| {
            if tail[..index].ends_with('\r') {
                index - 1
            } else {
                index
            }
        })
        .unwrap_or(tail.len());
    start + break_at
}

/// Strip the `;;` doc-comment marker and surrounding whitespace, matching
/// `Line::doc_comment`.
fn doc_comment_text(text: &str) -> String {
    text.strip_prefix(";;").unwrap_or(text).trim().to_string()
}

/// The end byte of the physical line that ends just before `pos`, excluding that
/// line's trailing `\r`/`\n`. Used to bound a function body at the end of its
/// last line, the line just above the line that closed the block.
fn line_text_end_before(source: &str, pos: usize) -> usize {
    let before = &source[..pos.min(source.len())];
    let before = before.strip_suffix('\n').unwrap_or(before);
    let before = before.strip_suffix('\r').unwrap_or(before);
    before.len()
}

/// The fallback span of a value-position expression: the byte range of its
/// tokens, or an empty span when there are none.
fn value_span(tokens: &[Token]) -> SourceSpan {
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => join_spans(first.span, last.span),
        _ => SourceSpan::default(),
    }
}

/// The `::`-separated source text spanned by the `module`/`use` name tokens, if
/// it is a qualified name. The text is validated lexically (not by token kind),
/// so a keyword that is also a valid path segment — such as the `bytes` in
/// `use std::bytes` — is accepted, the same way it is mid-path in an expression.
fn qualified_name(source: &str, tokens: &[Token]) -> Option<String> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let text = &source[first.span.start_byte..last.span.end_byte];
    is_qualified_name(text).then(|| text.to_string())
}

/// Parse an enum header line: `[pub] enum Name`. Returns the visibility flag and
/// the enum name. `pub` is recorded for consistency with `pub fn`; the body of
/// the enum is parsed separately from its indented block.
fn parse_enum_head(source: &str, tokens: &[Token]) -> Result<(bool, String), &'static str> {
    let (public, rest) = if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Pub))
    ) {
        (true, &tokens[1..])
    } else {
        (false, tokens)
    };
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Enum))
    ) {
        return Err("expected enum declaration");
    }
    let rest = &rest[1..];
    let name = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => return Err("expected enum name"),
    };
    if rest.len() > 1 {
        return Err("an enum header is just `enum Name`");
    }
    Ok((public, name))
}

/// The name and category flag of an enum member from its header tokens: a bare
/// identifier, optionally led by a contextual `category` word that marks it a
/// grouping node. `category` is recognized positionally as the header lead, so it
/// never collides with `category` used as an ordinary identifier elsewhere.
/// Anything else — a type annotation, key parameters, or extra tokens — is the
/// resource-member surface, which an enum member does not have.
fn enum_member_name(source: &str, tokens: &[Token]) -> Result<(String, bool), &'static str> {
    let (category, rest) = match tokens.first() {
        Some(token)
            if token.kind == TokenKind::Identifier
                && token.text(source) == "category"
                && tokens.len() > 1 =>
        {
            (true, &tokens[1..])
        }
        _ => (false, tokens),
    };
    match rest {
        [token] if token.kind == TokenKind::Identifier => {
            Ok((token.text(source).to_string(), category))
        }
        // A single non-identifier token is a reserved word standing in for a name.
        [_] => Err("expected an enum member name"),
        _ => Err("an enum member is a bare name; it takes no type or parameters"),
    }
}

/// The `::`-separated identifier segments of a match-arm header, or `None` when
/// the header is not a member path (`identifier ("::" identifier)*`). The
/// scrutinee supplies the enum, so an arm header carries no enum prefix — it is a
/// relative path the checker walks against the scrutinee enum's member tree.
fn arm_member_path(source: &str, tokens: &[Token]) -> Option<Vec<String>> {
    if tokens.is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        // Even positions are identifiers, odd positions the `::` separators.
        if index % 2 == 0 {
            if token.kind != TokenKind::Identifier {
                return None;
            }
            segments.push(token.text(source).to_string());
        } else if token.kind != TokenKind::DoubleColon {
            return None;
        }
    }
    // A trailing `::` (an even count of tokens) leaves a separator with no segment.
    if tokens.len() % 2 == 0 {
        return None;
    }
    Some(segments)
}

/// Parse a resource header's tokens after the `resource` keyword:
/// `Name [at ^root [(key: type, ...)]]`.
fn parse_resource_head(
    source: &str,
    tokens: &[Token],
) -> Result<(String, Option<SavedRoot>), &'static str> {
    let name = match tokens.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => return Err("expected resource name"),
    };
    let rest = &tokens[1..];
    if rest.is_empty() {
        return Ok((name, None));
    }
    // `at` is the saved-root keyword; the `@` symbol is a separate token used for
    // `@id` member annotations.
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::At))
    ) {
        return Err("expected `at ^root` after resource name");
    }
    let rest = &rest[1..];
    if !matches!(rest.first().map(|token| token.kind), Some(TokenKind::Caret)) {
        return Err("expected saved root beginning with `^`");
    }
    let root = match rest.get(1) {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => return Err("expected saved root name"),
    };
    let rest = &rest[2..];
    let keys = if rest.is_empty() {
        Vec::new()
    } else {
        parse_paren_key_params(source, rest)?
    };
    Ok((name, Some(SavedRoot { root, keys })))
}

/// Parse a parenthesized `(name: type, ...)` key parameter list spanning the
/// whole token slice. Requires the parentheses to be the only content.
fn parse_paren_key_params(source: &str, tokens: &[Token]) -> Result<Vec<KeyParam>, &'static str> {
    if !matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err("expected key parameter list");
    }
    let close = match_paren(tokens).ok_or("expected key parameter list")?;
    if close + 1 != tokens.len() {
        return Err("unexpected text after key parameter list");
    }
    parse_key_params_tokens(source, &tokens[1..close])
}

/// Parse a comma-separated `name: type` key list. Requires at least one key.
fn parse_key_params_tokens(source: &str, inner: &[Token]) -> Result<Vec<KeyParam>, &'static str> {
    if inner.is_empty() {
        return Err("expected at least one key parameter");
    }
    let mut params = Vec::new();
    for part in split_top_level_commas(inner) {
        let name = match part.first() {
            Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
            _ => return Err("expected key name"),
        };
        if part.get(1).map(|token| token.kind) != Some(TokenKind::Colon) || part.len() < 3 {
            return Err("expected key type annotation");
        }
        let ty = type_ref_from_tokens(source, &part[2..]);
        if !is_type_text(&ty.text) {
            return Err("expected key type annotation");
        }
        params.push(KeyParam { name, ty });
    }
    Ok(params)
}

/// Parse a `@id("stable.id")` member annotation from its tokens.
fn parse_stable_id_tokens(source: &str, tokens: &[Token]) -> Option<String> {
    // `@id ( "..." )`: At, Identifier(id), LeftParen, String, RightParen.
    let [at, id, open, value, close] = tokens else {
        return None;
    };
    if at.kind != TokenKind::At
        || !(id.kind == TokenKind::Identifier && id.text(source) == "id")
        || open.kind != TokenKind::LeftParen
        || value.kind != TokenKind::String
        || close.kind != TokenKind::RightParen
    {
        return None;
    }
    let text = value.text(source);
    let body = text.strip_prefix('"')?.strip_suffix('"')?;
    Some(body.to_string())
}

/// Parse an `index name(field, ...) [unique]` declaration from the tokens after
/// the `index` keyword. The span is filled in by the caller.
fn parse_index_tokens(source: &str, tokens: &[Token]) -> Result<IndexDecl, &'static str> {
    let name = match tokens.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => return Err("expected index name"),
    };
    let rest = &tokens[1..];
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err("expected index argument list");
    }
    let close = match_paren(rest).ok_or("expected index argument list")?;
    let inner = &rest[1..close];
    if inner.is_empty() {
        return Err("expected at least one index argument");
    }
    let mut args = Vec::new();
    for part in split_top_level_commas(inner) {
        args.push(field_path_text(source, part).ok_or("expected index field path")?);
    }
    let tail = &rest[close + 1..];
    let unique = match tail {
        [] => false,
        [token] if token.kind == TokenKind::Keyword(Keyword::Unique) => true,
        _ => return Err("expected `unique` or end of index declaration"),
    };
    Ok(IndexDecl {
        docs: Vec::new(),
        stable_id: None,
        name,
        args,
        unique,
        span: SourceSpan::default(),
    })
}

/// Validate a dotted field path (`field` or `field.sub`) and return its text.
fn field_path_text(source: &str, tokens: &[Token]) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }
    let mut expect_segment = true;
    for token in tokens {
        if expect_segment {
            if token.kind != TokenKind::Identifier {
                return None;
            }
        } else if token.kind != TokenKind::Dot {
            return None;
        }
        expect_segment = !expect_segment;
    }
    if expect_segment {
        return None;
    }
    let start = tokens[0].span.start_byte;
    let end = tokens[tokens.len() - 1].span.end_byte;
    Some(source[start..end].to_string())
}

/// Parse a `required? name (keys)? (: type)?` resource member head into a field
/// or group, matching `parse_field_or_group_head`.
fn parse_field_or_group_tokens(source: &str, tokens: &[Token]) -> Result<MemberHead, &'static str> {
    let (required, rest) = if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Required))
    ) {
        (true, &tokens[1..])
    } else {
        (false, tokens)
    };
    let name = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => return Err("expected resource member name"),
    };
    let mut rest = &rest[1..];
    let keys = if matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        let close = match_paren(rest).ok_or("expected closing `)` in keyed resource member")?;
        let inner = &rest[1..close];
        let keys = parse_key_params_tokens(source, inner)?;
        rest = &rest[close + 1..];
        keys
    } else {
        Vec::new()
    };
    if matches!(rest.first().map(|token| token.kind), Some(TokenKind::Colon)) {
        let ty_tokens = &rest[1..];
        if ty_tokens.is_empty() {
            return Err("expected field type after `:`");
        }
        let ty = type_ref_from_tokens(source, ty_tokens);
        if !is_type_text(&ty.text) {
            return Err("expected field type after `:`");
        }
        return Ok(MemberHead::Field {
            required,
            name,
            keys,
            ty,
        });
    }
    if required {
        return Err("required resource members must declare a field type");
    }
    if rest.is_empty() {
        return Ok(MemberHead::Group { name, keys });
    }
    Err("expected resource field, keyed field, group, or index")
}

/// Parse a function header's tokens: `pub? fn name(params) (: return)?`.
fn parse_function_head(source: &str, tokens: &[Token]) -> Result<FunctionHead, &'static str> {
    let (public, rest) = if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Pub))
    ) {
        (true, &tokens[1..])
    } else if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Identifier)
    ) {
        // `internal fn`/`private fn`: the visibility word lexes as an
        // identifier; reject it with a pointed message.
        let word = tokens[0].text(source);
        if word == "internal" {
            return Err("function visibility is only `pub` or module-private; remove `internal`");
        }
        if word == "private" {
            return Err("function visibility is only `pub` or module-private; remove `private`");
        }
        (false, tokens)
    } else {
        (false, tokens)
    };
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Fn))
    ) {
        return Err("expected fn declaration");
    }
    let rest = &rest[1..];
    let name = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => return Err("expected function name"),
    };
    let rest = &rest[1..];
    if matches!(rest.first().map(|token| token.kind), Some(TokenKind::Less)) {
        return Err("user-defined generics are not used in Marrow");
    }
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err("expected function parameter list");
    }
    let close = match_paren(rest).ok_or("expected function parameter list")?;
    let params = parse_params_tokens(source, &rest[1..close])?;
    let after = &rest[close + 1..];
    let return_type = if after.is_empty() {
        None
    } else {
        if after[0].kind != TokenKind::Colon {
            return Err("expected return type after `:`");
        }
        let ty_tokens = &after[1..];
        if ty_tokens.is_empty() {
            return Err("expected return type after `:`");
        }
        let ty = type_ref_from_tokens(source, ty_tokens);
        if !is_type_text(&ty.text) {
            return Err("expected return type annotation");
        }
        Some(ty)
    };
    Ok(FunctionHead {
        public,
        name,
        params,
        return_type,
    })
}

/// Parse a `(out|inout)? name: type` parameter list. Parameters are separated by
/// commas, and in a multi-line list a line break separates one from the next just
/// as a comma does, so the list reads cleanly written with commas, without them,
/// or mixed. A run of `;;` doc lines directly above a parameter is its
/// documentation, captured in source order.
fn parse_params_tokens(source: &str, inner: &[Token]) -> Result<Vec<ParamDecl>, &'static str> {
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for group in split_param_groups(inner) {
        // A doc run with no parameter after it documents nothing; report the
        // misplaced doc rather than dropping it.
        if group.body.is_empty() {
            return Err("a doc comment must precede a parameter");
        }
        let docs = group
            .docs
            .iter()
            .map(|token| doc_comment_text(token.text(source)))
            .collect();
        let (mode, rest) = match group.body.first().map(|token| token.kind) {
            Some(TokenKind::Keyword(Keyword::Out)) => (Some(ParamMode::Out), &group.body[1..]),
            Some(TokenKind::Keyword(Keyword::InOut)) => (Some(ParamMode::InOut), &group.body[1..]),
            _ => (None, group.body),
        };
        let name = match rest.first() {
            Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
            _ => return Err("expected parameter name"),
        };
        if rest.get(1).map(|token| token.kind) != Some(TokenKind::Colon) || rest.len() < 3 {
            return Err("expected parameter type annotation");
        }
        let ty_tokens = &rest[2..];
        // A default value (`name: type = expr`) is rejected; an `=` here means a
        // parameter default, which Marrow does not use.
        if ty_tokens.iter().any(|token| token.kind == TokenKind::Equal) {
            return Err("parameter defaults are not used in Marrow");
        }
        let ty = type_ref_from_tokens(source, ty_tokens);
        if !is_type_text(&ty.text) {
            return Err("expected parameter type annotation");
        }
        params.push(ParamDecl {
            docs,
            mode,
            name,
            ty,
        });
    }
    Ok(params)
}

/// One parameter's tokens: its leading `;;` doc-comment run and the body tokens
/// that spell `(out|inout)? name: type`.
struct ParamGroup<'a> {
    docs: Vec<&'a Token>,
    body: &'a [Token],
}

/// Split a parameter list's inner tokens into per-parameter groups. A top-level
/// comma ends a parameter, and so does a line break in a multi-line list: a body
/// token that opens on a later source line than the parameter in progress starts
/// the next one. Newlines are suppressed inside the parentheses, so the line
/// boundary is read from token spans rather than a separator token. A leading run
/// of `;;` doc comments attaches to the parameter it precedes.
fn split_param_groups(inner: &[Token]) -> Vec<ParamGroup<'_>> {
    let mut groups = Vec::new();
    let mut docs: Vec<&Token> = Vec::new();
    let mut body_start: Option<usize> = None;
    let mut depth = 0usize;

    let mut index = 0;
    while index < inner.len() {
        let token = &inner[index];
        // The depth before this token's own bracket is what places the token: a
        // closing `]` or `)` still belongs to the type it closes, so it reads at
        // the deeper level even though it drops the depth back afterwards.
        let depth_before = depth;
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            _ => {}
        }

        if depth == 0 && token.kind == TokenKind::Comma {
            if let Some(start) = body_start.take() {
                groups.push(ParamGroup {
                    docs: std::mem::take(&mut docs),
                    body: &inner[start..index],
                });
            }
            index += 1;
            continue;
        }

        if token.kind == TokenKind::DocComment {
            // A doc comment that opens a new parameter's documentation follows a
            // completed parameter body, so close that body before collecting it.
            if let Some(start) = body_start.take() {
                groups.push(ParamGroup {
                    docs: std::mem::take(&mut docs),
                    body: &inner[start..index],
                });
            }
            docs.push(token);
            index += 1;
            continue;
        }

        match body_start {
            None => body_start = Some(index),
            // A body token on a later source line than the parameter in progress
            // begins the next parameter, so a line break separates parameters the
            // same way a comma does. Only a top-level line break ends a parameter;
            // a parameter occupies one logical line, and its type may still wrap
            // across physical lines inside `(` or `[`, where the deeper depth keeps
            // the wrap from splitting the parameter.
            Some(start) if depth_before == 0 && token.span.line > inner[start].span.line => {
                groups.push(ParamGroup {
                    docs: std::mem::take(&mut docs),
                    body: &inner[start..index],
                });
                body_start = Some(index);
            }
            Some(_) => {}
        }
        index += 1;
    }

    match body_start {
        Some(start) => groups.push(ParamGroup {
            docs,
            body: &inner[start..],
        }),
        // A `;;` run with no parameter after it documents nothing. Surface it as a
        // body-less group so the caller can report the misplaced doc rather than
        // drop it.
        None if !docs.is_empty() => groups.push(ParamGroup {
            docs,
            body: &inner[inner.len()..],
        }),
        None => {}
    }
    groups
}

/// Index of the `)` matching the leading `(` of `tokens`, if balanced.
fn match_paren(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen => depth += 1,
            TokenKind::RightParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_const_or_var(
    source: &str,
    line: &[Token],
    is_var: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let keyword = line[0];
    let name_token = line.get(1)?;
    if name_token.kind != TokenKind::Identifier {
        return None;
    }
    let name = name_token.text(source).to_string();
    let mut index = 2;

    // A keyed `var` declares a local keyed tree: `var counts(name: string): int`.
    // `const` has no key parameters.
    let mut keys = Vec::new();
    if line.get(index).map(|token| token.kind) == Some(TokenKind::LeftParen) {
        if !is_var {
            return None;
        }
        let (parsed_keys, after) = parse_var_keys(source, line, index)?;
        keys = parsed_keys;
        index = after;
    }

    let mut ty = None;
    if line.get(index).map(|token| token.kind) == Some(TokenKind::Colon) {
        index += 1;
        let type_start = index;
        while index < line.len() && line[index].kind != TokenKind::Equal {
            index += 1;
        }
        if index == type_start {
            return None;
        }
        ty = Some(type_ref_from_tokens(source, &line[type_start..index]));
    }

    match line.get(index).map(|token| token.kind) {
        Some(TokenKind::Equal) => {
            let value = expr_of(source, &line[index + 1..], diagnostics)?;
            let span = join_spans(keyword.span, value.span());
            Some(if is_var {
                Statement::Var {
                    name,
                    keys,
                    ty,
                    value: Some(value),
                    span,
                }
            } else {
                Statement::Const {
                    name,
                    ty,
                    value,
                    span,
                }
            })
        }
        // `var name[(keys)][: type]` without an initializer is allowed; `const` is not.
        None if is_var => Some(Statement::Var {
            name,
            keys,
            ty,
            value: None,
            span: join_spans(keyword.span, line[line.len() - 1].span),
        }),
        _ => None,
    }
}

/// Parse `(name: type, ...)` key parameters of a keyed `var`, starting at the
/// `(` token at `open_index`. Returns the parsed keys and the line index just
/// past the closing `)`.
fn parse_var_keys(
    source: &str,
    line: &[Token],
    open_index: usize,
) -> Option<(Vec<KeyParam>, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (offset, token) in line[open_index..].iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen => depth += 1,
            TokenKind::RightParen => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open_index + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let keys = parse_key_param_list(source, &line[open_index + 1..close])?;
    Some((keys, close + 1))
}

/// Parse a comma-separated list of `name: type` key declarations. Requires at
/// least one declaration.
fn parse_key_param_list(source: &str, inner: &[Token]) -> Option<Vec<KeyParam>> {
    if inner.is_empty() {
        return None;
    }
    let mut keys = Vec::new();
    for part in split_top_level_commas(inner) {
        let name = part.first()?;
        if name.kind != TokenKind::Identifier
            || part.get(1).map(|token| token.kind) != Some(TokenKind::Colon)
            || part.len() < 3
        {
            return None;
        }
        keys.push(KeyParam {
            name: name.text(source).to_string(),
            ty: type_ref_from_tokens(source, &part[2..]),
        });
    }
    Some(keys)
}

/// Split tokens on top-level commas (depth 0), dropping a trailing empty group
/// from a trailing comma.
fn split_top_level_commas(tokens: &[Token]) -> Vec<&[Token]> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            TokenKind::Comma if depth == 0 => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < tokens.len() {
        parts.push(&tokens[start..]);
    }
    parts
}

fn parse_return(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let keyword = line[0];
    if line.len() == 1 {
        return Some(Statement::Return {
            value: None,
            span: keyword.span,
        });
    }
    let value = expr_of(source, &line[1..], diagnostics)?;
    Some(Statement::Return {
        span: join_spans(keyword.span, value.span()),
        value: Some(value),
    })
}

fn parse_merge(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let keyword = line[0];
    let rest = &line[1..];
    let equal = find_top_level_equal(rest)?;
    let target = expr_of(source, &rest[..equal], diagnostics)?;
    let value = expr_of(source, &rest[equal + 1..], diagnostics)?;
    Some(Statement::Merge {
        span: join_spans(keyword.span, value.span()),
        target,
        value,
    })
}

fn parse_break_or_continue(source: &str, line: &[Token], is_break: bool) -> Option<Statement> {
    let keyword = line[0];
    let (label, span) = match line.get(1) {
        None => (None, keyword.span),
        Some(token) if token.kind == TokenKind::Identifier && line.len() == 2 => (
            Some(token.text(source).to_string()),
            join_spans(keyword.span, token.span),
        ),
        _ => return None,
    };
    Some(if is_break {
        Statement::Break { label, span }
    } else {
        Statement::Continue { label, span }
    })
}

fn parse_assign_or_expr(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    if let Some(equal) = find_top_level_equal(line) {
        let target = expr_of(source, &line[..equal], diagnostics)?;
        let value = expr_of(source, &line[equal + 1..], diagnostics)?;
        Some(Statement::Assign {
            span: join_spans(target.span(), value.span()),
            target,
            value,
        })
    } else {
        let value = expr_of(source, line, diagnostics)?;
        Some(Statement::Expr {
            span: value.span(),
            value,
        })
    }
}

/// Index of the first top-level `=` (assignment separator). Equality is `==`, so
/// a depth-0 `=` is unambiguously the assignment in a statement; the depth-0
/// restriction still keeps named-argument colons and nested forms from splitting.
fn find_top_level_equal(tokens: &[Token]) -> Option<usize> {
    find_top_level(tokens, TokenKind::Equal)
}

/// Index of the first occurrence of `kind` at parenthesis/bracket depth 0.
fn find_top_level(tokens: &[Token], kind: TokenKind) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            other if other == kind && depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

/// Parse a `for` header `binding in iterable [by step]` into the loop binding,
/// the iterable expression, and the optional range step. Returns `None` if the
/// `in` keyword or binding is malformed. `by` is a contextual keyword: it splits
/// the header only as a bare top-level word, so a name `by` elsewhere is unaffected.
fn parse_for_header(
    source: &str,
    header: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(ForBinding, Expression, Option<Expression>)> {
    let in_index = find_top_level(header, TokenKind::Keyword(Keyword::In))?;
    let binding = parse_for_binding(source, &header[..in_index])?;
    let after_in = &header[in_index + 1..];
    let (iterable_tokens, step) = match find_top_level_by(source, after_in) {
        Some(by_index) => {
            let step = expr_of(source, &after_in[by_index + 1..], diagnostics)?;
            (&after_in[..by_index], Some(step))
        }
        None => (after_in, None),
    };
    let iterable = expr_of(source, iterable_tokens, diagnostics)?;
    Some((binding, iterable, step))
}

/// Index of a top-level contextual `by` in a range-for header. `by` is a plain
/// identifier, not a reserved word, so it splits the header only when it stands at
/// bracket depth 0 — never inside a call's arguments or a name `by` used as a value.
fn find_top_level_by(source: &str, tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            TokenKind::Identifier if depth == 0 && token.text(source) == "by" => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// Parse a `catch` header `name` or `name: Type` into the bound name and an
/// optional type annotation. A malformed header yields an empty name.
fn parse_catch_header(source: &str, header: &[Token]) -> (String, Option<TypeRef>) {
    let Some(name_token) = header.first() else {
        return (String::new(), None);
    };
    if name_token.kind != TokenKind::Identifier {
        return (String::new(), None);
    }
    let name = name_token.text(source).to_string();
    let ty = match header.get(1) {
        Some(colon) if colon.kind == TokenKind::Colon && header.len() > 2 => {
            Some(type_ref_from_tokens(source, &header[2..]))
        }
        _ => None,
    };
    (name, ty)
}

fn parse_for_binding(source: &str, tokens: &[Token]) -> Option<ForBinding> {
    let ident = |token: &Token| {
        (token.kind == TokenKind::Identifier).then(|| token.text(source).to_string())
    };
    match tokens {
        [first] => Some(ForBinding {
            first: ident(first)?,
            second: None,
        }),
        [first, comma, second] if comma.kind == TokenKind::Comma => Some(ForBinding {
            first: ident(first)?,
            second: Some(ident(second)?),
        }),
        _ => None,
    }
}

fn expr_of(
    source: &str,
    tokens: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    ExprParser::new(source, tokens).parse_complete(diagnostics)
}

fn type_ref_from_tokens(source: &str, tokens: &[Token]) -> TypeRef {
    let start = tokens[0].span.start_byte;
    let end = tokens[tokens.len() - 1].span.end_byte;
    let span = join_spans(tokens[0].span, tokens[tokens.len() - 1].span);
    // A type is a qualified name, optionally wrapped in `sequence[...]`, so no
    // interior whitespace is significant. A type that wraps across physical lines
    // inside its brackets is stored by its canonical single-line spelling, with
    // the wrap whitespace removed, so the formatter emits one line.
    let text = source[start..end]
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    TypeRef { text, span }
}

fn line_span(tokens: &[Token]) -> SourceSpan {
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => join_spans(first.span, last.span),
        _ => SourceSpan::default(),
    }
}

/// Build a `Comment` from a `;` comment token, stripping the leading marker and
/// surrounding whitespace so the formatter renders a canonical `; text` line.
fn comment_from_token(source: &str, token: Token, placement: CommentPlacement) -> Comment {
    let text = token
        .text(source)
        .trim_start_matches(';')
        .trim()
        .to_string();
    Comment {
        text,
        placement,
        span: token.span,
    }
}
