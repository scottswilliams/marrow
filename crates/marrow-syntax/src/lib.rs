use std::fmt;

mod format;
pub use format::{format_block, format_declaration, format_expression, format_source};

pub const PARSE_SYNTAX: &str = "parse.syntax";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSource {
    pub file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedSource {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceFile {
    pub module: Option<ModuleDecl>,
    pub uses: Vec<UseDecl>,
    pub declarations: Vec<Declaration>,
}

impl SourceFile {
    pub fn resource(&self, name: &str) -> Option<&ResourceDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(resource) if resource.name == name => Some(resource),
                _ => None,
            })
    }

    pub fn function(&self, name: &str) -> Option<&FunctionDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == name => Some(function),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDecl {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseDecl {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    Const(ConstDecl),
    Resource(ResourceDecl),
    Function(FunctionDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub value: Expression,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expression {
    Literal {
        kind: LiteralKind,
        text: String,
        span: SourceSpan,
    },
    /// A name path of one or more `::`-separated identifiers, such as `x` or
    /// `std::math::PI`.
    Name {
        segments: Vec<String>,
        span: SourceSpan,
    },
    /// A saved-data root such as `^books`. Postfix key lookups and field
    /// access build the rest of a saved path on top of this.
    SavedRoot { name: String, span: SourceSpan },
    /// A parenthesized application: a function call, key lookup, conversion, or
    /// resource constructor. The checker resolves which one from the callee.
    Call {
        callee: Box<Expression>,
        args: Vec<Argument>,
        span: SourceSpan,
    },
    /// Dotted field access, such as `book.title` or `^books(id)."old-title"`.
    /// `name` is the field name without surrounding quotes; `quoted` records
    /// whether it was written as a quoted segment (allowed for data names that
    /// are not identifiers).
    Field {
        base: Box<Expression>,
        name: String,
        quoted: bool,
        span: SourceSpan,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expression>,
        span: SourceSpan,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expression>,
        right: Box<Expression>,
        span: SourceSpan,
    },
    /// An interpolated string `$"..."` as a sequence of literal text and
    /// embedded expression parts, in source order.
    Interpolation {
        parts: Vec<InterpolationPart>,
        span: SourceSpan,
    },
    /// Expression text the grammar does not structure into a node. Carries the
    /// raw text so the formatter can re-emit it verbatim.
    Unparsed { text: String, span: SourceSpan },
}

impl Expression {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Literal { span, .. }
            | Self::Name { span, .. }
            | Self::SavedRoot { span, .. }
            | Self::Call { span, .. }
            | Self::Field { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Interpolation { span, .. }
            | Self::Unparsed { span, .. } => *span,
        }
    }
}

/// One segment of an interpolated string: either literal text (with `{{`/`}}`
/// still escaped as written) or an embedded expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpolationPart {
    Text { text: String, span: SourceSpan },
    Expr(Expression),
}

/// One argument in a call expression. `name` is set for named arguments
/// (`title: draft`); `mode` is set for `out`/`inout` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub mode: Option<ArgMode>,
    pub name: Option<String>,
    pub value: Expression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgMode {
    Out,
    InOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Integer,
    Decimal,
    String,
    Bytes,
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    Concat,
    RangeExclusive,
    RangeInclusive,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub store: Option<SavedRoot>,
    pub members: Vec<ResourceMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRoot {
    pub root: String,
    pub keys: Vec<KeyParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceMember {
    Field(FieldDecl),
    Group(GroupDecl),
    Index(IndexDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub required: bool,
    pub name: String,
    pub keys: Vec<KeyParam>,
    pub ty: TypeRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub keys: Vec<KeyParam>,
    pub members: Vec<ResourceMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub args: Vec<String>,
    pub unique: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub return_type: Option<TypeRef>,
    pub body: Block,
    pub span: SourceSpan,
}

/// An indented sequence of statements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    pub statements: Vec<Statement>,
    /// Ordinary `;` comments inside this block, in source order. They are kept
    /// as block-level trivia (not attached to statement nodes) so the formatter
    /// can re-emit them and `parse -> format` round-trips comments losslessly.
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// An ordinary `;` comment retained as block trivia. `text` is the comment body
/// with the leading `;` marker and surrounding whitespace removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub placement: CommentPlacement,
    pub span: SourceSpan,
}

/// Where a retained comment sits relative to the statements of its block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPlacement {
    /// A comment occupying its own line (a leading or standalone comment).
    OwnLine,
    /// A comment following code on a statement's line.
    Trailing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Const {
        name: String,
        ty: Option<TypeRef>,
        value: Expression,
        span: SourceSpan,
    },
    Var {
        name: String,
        keys: Vec<KeyParam>,
        ty: Option<TypeRef>,
        value: Option<Expression>,
        span: SourceSpan,
    },
    Assign {
        target: Expression,
        value: Expression,
        span: SourceSpan,
    },
    Delete {
        path: Expression,
        span: SourceSpan,
    },
    Merge {
        target: Expression,
        value: Expression,
        span: SourceSpan,
    },
    Return {
        value: Option<Expression>,
        span: SourceSpan,
    },
    Break {
        label: Option<String>,
        span: SourceSpan,
    },
    Continue {
        label: Option<String>,
        span: SourceSpan,
    },
    Throw {
        value: Expression,
        span: SourceSpan,
    },
    Expr {
        value: Expression,
        span: SourceSpan,
    },
    If {
        condition: Expression,
        then_block: Block,
        else_ifs: Vec<ElseIf>,
        else_block: Option<Block>,
        span: SourceSpan,
    },
    While {
        label: Option<String>,
        condition: Expression,
        body: Block,
        span: SourceSpan,
    },
    For {
        label: Option<String>,
        binding: ForBinding,
        iterable: Expression,
        body: Block,
        span: SourceSpan,
    },
    Transaction {
        body: Block,
        span: SourceSpan,
    },
    Lock {
        path: Expression,
        body: Block,
        span: SourceSpan,
    },
    Try {
        body: Block,
        catch: Option<CatchClause>,
        finally: Option<Block>,
        span: SourceSpan,
    },
    /// A statement line the grammar does not recognize. Parsing raises a
    /// diagnostic and keeps this placeholder so the following statements still
    /// parse.
    Unparsed {
        span: SourceSpan,
    },
}

/// One `else if` clause of an `if` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIf {
    pub condition: Expression,
    pub block: Block,
}

/// The `catch name: Error` clause of a `try` statement. `ty` is the optional
/// type annotation on the bound error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchClause {
    pub name: String,
    pub ty: Option<TypeRef>,
    pub block: Block,
}

/// The loop variable(s) of a `for` statement: `for first in ...` or
/// `for first, second in ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForBinding {
    pub first: String,
    pub second: Option<String>,
}

impl Statement {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Const { span, .. }
            | Self::Var { span, .. }
            | Self::Assign { span, .. }
            | Self::Delete { span, .. }
            | Self::Merge { span, .. }
            | Self::Return { span, .. }
            | Self::Break { span, .. }
            | Self::Continue { span, .. }
            | Self::Throw { span, .. }
            | Self::Expr { span, .. }
            | Self::If { span, .. }
            | Self::While { span, .. }
            | Self::For { span, .. }
            | Self::Transaction { span, .. }
            | Self::Lock { span, .. }
            | Self::Try { span, .. }
            | Self::Unparsed { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    pub mode: Option<ParamMode>,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Out,
    InOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParam {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub text: String,
}

impl fmt::Display for TypeRef {
    // The parser keeps the verbatim source spelling so the formatter re-emits a
    // type annotation exactly as written. Resolution to a structured type happens
    // once in marrow-schema; this text is the AST's only remaining use of it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub kind: &'static str,
    pub severity: Severity,
    pub message: String,
    pub help: Option<String>,
    pub span: SourceSpan,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}: {}",
            self.line,
            self.column,
            self.severity.as_str(),
            self.code,
            self.message
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexedSource {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

impl LexedSource {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: SourceSpan,
}

impl Token {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.start_byte..self.span.end_byte]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Identifier,
    Integer,
    Decimal,
    String,
    InterpolationStart,
    InterpolationText,
    InterpolationExprStart,
    InterpolationExprEnd,
    InterpolationEnd,
    Bytes,
    Keyword(Keyword),
    Comment,
    DocComment,
    Indent,
    Dedent,
    Newline,
    Eof,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Colon,
    DoubleColon,
    Comma,
    Dot,
    DotDot,
    DotDotEqual,
    Equal,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Underscore,
    Caret,
    At,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Module,
    Use,
    Pub,
    Fn,
    Resource,
    At,
    Index,
    Unique,
    Required,
    Const,
    Var,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Return,
    Delete,
    Merge,
    Transaction,
    Lock,
    Try,
    Catch,
    Finally,
    Throw,
    Out,
    InOut,
    True,
    False,
    Not,
    And,
    Or,
    Int,
    Decimal,
    Bool,
    String,
    Bytes,
    Date,
    Instant,
    Duration,
    Sequence,
    Unknown,
    Error,
    ErrorCode,
}

pub fn lex_source(source: &str) -> LexedSource {
    Lexer::new(source).lex()
}

pub fn parse_source(source: &str) -> ParsedSource {
    let lexed = lex_source(source);
    let mut parsed = Parser::new(source, &lexed.tokens).parse();
    let mut combined = lexed.diagnostics;
    combined.append(&mut parsed.diagnostics);
    combined.sort_by_key(|diagnostic| (diagnostic.span.line, diagnostic.span.start_byte));
    parsed.diagnostics = combined;
    parsed
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
        for line in self.lines.clone() {
            self.reject_line_tabs(line);

            if line.is_blank() {
                continue;
            }

            if line.is_comment() || line.doc_comment().is_some() {
                let is_doc_comment = line.doc_comment().is_some();
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
            self.error_at(
                self.span(line, index, end),
                format!("unexpected character `{ch}`"),
            );
            index = end;
        }
    }

    fn reject_obsolete_operator(&mut self, line: Line<'a>, index: usize) -> Option<usize> {
        let tail = &self.source[index..line.end_byte];
        let (consumed, message, help) = if tail.starts_with("==") {
            (2, "`==` is not used in Marrow", "Use `=` for equality.")
        } else if tail.starts_with("&&") {
            (
                2,
                "`&&` is not used in Marrow",
                "Use `and` for boolean and.",
            )
        } else if tail.starts_with("||") {
            (2, "`||` is not used in Marrow", "Use `or` for boolean or.")
        } else if tail.starts_with('!') && !tail.starts_with("!=") {
            (
                1,
                "`!` is not used in Marrow",
                "Use `not` for boolean negation.",
            )
        } else if tail.starts_with('#') {
            (
                1,
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
            kind: "parse",
            severity: Severity::Error,
            message: message.to_string(),
            help: Some(help.to_string()),
            line: span.line,
            column: span.column,
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
        let mut end = start;
        for (offset, ch) in self.source[start..line.end_byte].char_indices() {
            if !is_identifier_continue_char(ch) {
                break;
            }
            end = start + offset + ch.len_utf8();
        }
        let text = &self.source[start..end];
        let kind = keyword(text)
            .map(TokenKind::Keyword)
            .unwrap_or(TokenKind::Identifier);
        self.push(kind, self.span(line, start, end));
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

        self.error_at(self.span(line, start, line.end_byte), "unterminated string");
        self.push(kind, self.span(line, start, line.end_byte));
        line.end_byte
    }

    fn punctuation(&self, index: usize, line_end: usize) -> Option<(TokenKind, usize)> {
        let tail = &self.source[index..line_end];
        for (text, kind) in [
            ("::", TokenKind::DoubleColon),
            ("..=", TokenKind::DotDotEqual),
            ("..", TokenKind::DotDot),
            ("!=", TokenKind::BangEqual),
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

    fn error_at(&mut self, span: SourceSpan, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            kind: "parse",
            severity: Severity::Error,
            message: message.into(),
            help: None,
            line: span.line,
            column: span.column,
            span,
        });
    }
}

struct Parser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    lines: Vec<Line<'a>>,
    index: usize,
    diagnostics: Vec<Diagnostic>,
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

impl<'a> Parser<'a> {
    fn new(source: &'a str, tokens: &'a [Token]) -> Self {
        Self {
            source,
            tokens,
            lines: split_lines(source),
            index: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse(mut self) -> ParsedSource {
        let mut file = SourceFile::default();
        let mut docs = Vec::new();
        let mut saw_top_level_item = false;

        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if self.reject_tabs(line) {
                self.index += 1;
                continue;
            }
            if line.is_blank() || line.is_comment() {
                self.index += 1;
                continue;
            }
            if let Some(doc) = line.doc_comment() {
                docs.push(doc.to_string());
                self.index += 1;
                continue;
            }
            if line.indent != 0 {
                self.error(line, "expected a top-level declaration");
                self.index += 1;
                continue;
            }

            let content = line.content;
            if let Some(rest) = content.strip_prefix("module ") {
                if saw_top_level_item {
                    self.error(
                        line,
                        "module declaration must appear once at the start of the file",
                    );
                } else {
                    let name = rest.trim();
                    if is_qualified_name(name) {
                        file.module = Some(ModuleDecl {
                            name: name.to_string(),
                            span: line.span(),
                        });
                    } else {
                        self.error(line, "expected qualified module name");
                    }
                }
                saw_top_level_item = true;
                docs.clear();
                self.index += 1;
            } else if let Some(rest) = content.strip_prefix("use ") {
                let name = rest.trim();
                if is_qualified_name(name) {
                    file.uses.push(UseDecl {
                        name: name.to_string(),
                        span: line.span(),
                    });
                } else {
                    self.error(line, "expected qualified import name");
                }
                saw_top_level_item = true;
                docs.clear();
                self.index += 1;
            } else if content.starts_with("const ") {
                // A const value may span several physical lines inside open
                // delimiters; consume the whole logical line, not just the first.
                let end_index = self.logical_line_end(self.index);
                let declaration = self.parse_const(line, end_index, std::mem::take(&mut docs));
                file.declarations.push(Declaration::Const(declaration));
                saw_top_level_item = true;
                self.index = end_index;
            } else if content.starts_with("resource ") {
                let resource = self.parse_resource(line, std::mem::take(&mut docs));
                file.declarations.push(Declaration::Resource(resource));
                saw_top_level_item = true;
            } else if starts_function(content)
                || content.starts_with("internal fn ")
                || content.starts_with("private fn ")
            {
                let function = self.parse_function(line, std::mem::take(&mut docs));
                file.declarations.push(Declaration::Function(function));
                saw_top_level_item = true;
            } else if content.starts_with("type ") {
                self.error(
                    line,
                    "type aliases are not used in Marrow; declare a resource or use a builtin type directly",
                );
                docs.clear();
                saw_top_level_item = true;
                self.index += 1;
            } else {
                self.error(
                    line,
                    "expected module, use, const, resource, or fn declaration",
                );
                docs.clear();
                saw_top_level_item = true;
                self.index += 1;
            }
        }

        self.report_keyword_field_names();
        report_positional_after_named(&file, &mut self.diagnostics);
        self.drop_redundant_statement_errors();
        ParsedSource {
            file,
            diagnostics: self.diagnostics,
        }
    }

    /// Drop the generic "expected a statement" fallback on any line that already
    /// carries a more specific diagnostic (e.g. a keyword used as a field name),
    /// so a single malformed line reports once with the most useful message.
    fn drop_redundant_statement_errors(&mut self) {
        let specific_lines: std::collections::HashSet<u32> = self
            .diagnostics
            .iter()
            .filter(|d| d.message != "expected a statement")
            .map(|d| d.line)
            .collect();
        self.diagnostics
            .retain(|d| d.message != "expected a statement" || !specific_lines.contains(&d.line));
    }

    /// Report bare keywords used as field names. A `.` is always data field
    /// access, and a field name must be an identifier or string literal, so a
    /// reserved word immediately after `.` is never a valid field name and must
    /// be quoted (`."at"`). The structural parsers cannot build such a field and
    /// leave the line `Unparsed`, so the diagnostic is raised here from the
    /// token stream, where the `.` and the keyword are both visible.
    fn report_keyword_field_names(&mut self) {
        for pair in self.tokens.windows(2) {
            let [dot, name] = pair else { continue };
            if dot.kind != TokenKind::Dot || !matches!(name.kind, TokenKind::Keyword(_)) {
                continue;
            }
            let keyword = name.text(self.source);
            let span = join_spans(dot.span, name.span);
            self.diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                kind: "parse",
                severity: Severity::Error,
                message: format!("`{keyword}` is a keyword and cannot be used as a field name"),
                help: Some(format!(
                    "quote the reserved word to use it as a data name: .\"{keyword}\""
                )),
                span,
                line: span.line,
                column: span.column,
            });
        }
    }

    /// Parse a top-level `const` declaration that occupies the physical lines
    /// `[self.index, end_index)`. The value may span several lines inside open
    /// delimiters, so it is parsed from the file-wide token stream over the
    /// whole byte range rather than only the first line's text.
    fn parse_const(&mut self, line: Line<'a>, end_index: usize, docs: Vec<String>) -> ConstDecl {
        let prefix_len = "const ".len();
        let after_prefix = &line.content[prefix_len..];
        let equal_offset = after_prefix.find('=');

        // The value's byte range runs from just after `=` on the first line to
        // the end of the last physical line the const spans.
        let value_end_byte = self.lines[end_index - 1].end_byte;

        let (head, value_start_byte, value_column) = match equal_offset {
            Some(offset) => {
                let head = after_prefix[..offset].trim();
                let after_equal_offset = prefix_len + offset + 1;
                let after_equal = &line.content[after_equal_offset..];
                let leading = after_equal.len() - after_equal.trim_start().len();
                let value_offset_in_content = after_equal_offset + leading;
                let start_byte = line.start_byte + line.indent + value_offset_in_content;
                if start_byte >= value_end_byte
                    || self.source[start_byte..value_end_byte].trim().is_empty()
                {
                    self.error(line, "const declarations require a value after `=`");
                }
                (
                    head,
                    start_byte,
                    (line.indent + value_offset_in_content + 1) as u32,
                )
            }
            None => {
                self.error(line, "const declarations require `=` and a value");
                (
                    after_prefix.trim(),
                    value_end_byte,
                    (line.indent + line.content.len() + 1) as u32,
                )
            }
        };

        let (name, ty) = parse_name_type(head);
        if !is_identifier(name) {
            self.error(line, "expected const name before type annotation");
        }
        if ty.is_some_and(|ty| !is_type_text(ty)) {
            self.error(line, "expected const type annotation");
        }

        let value = self.parse_value_expression(
            value_start_byte,
            value_end_byte,
            line.number,
            value_column,
        );

        ConstDecl {
            docs,
            name: name.to_string(),
            ty: ty.filter(|ty| is_type_text(ty)).map(type_ref),
            value,
            span: line.span(),
        }
    }

    /// Find the exclusive line index where the logical line starting at `start`
    /// ends. Lines continue while delimiters are open, matching the lexer, which
    /// suppresses `NEWLINE` inside `(...)` / `[...]`.
    fn logical_line_end(&self, start: usize) -> usize {
        let mut depth = 0usize;
        let mut index = start;
        while index < self.lines.len() {
            let line = self.lines[index];
            let tokens = tokens_in_range(self.tokens, line.start_byte, line.end_byte);
            for token in tokens {
                match token.kind {
                    TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
                    TokenKind::RightParen | TokenKind::RightBracket => {
                        depth = depth.saturating_sub(1)
                    }
                    _ => {}
                }
            }
            index += 1;
            if depth == 0 {
                break;
            }
        }
        index
    }

    /// Parse an expression occupying a value position (such as a `const` value)
    /// from the file-wide token stream over `[start_byte, end_byte)`. Spans stay
    /// absolute. Anything the expression grammar does not structure becomes
    /// `Expression::Unparsed`.
    fn parse_value_expression(
        &self,
        start_byte: usize,
        end_byte: usize,
        line: u32,
        column: u32,
    ) -> Expression {
        let value_text = self.source.get(start_byte..end_byte).unwrap_or_default();
        let fallback_span = SourceSpan {
            start_byte,
            end_byte,
            line,
            column,
        };
        let tokens = tokens_in_range(self.tokens, start_byte, end_byte);
        ExprParser::new(self.source, tokens).parse_value(fallback_span, value_text)
    }

    fn parse_resource(&mut self, line: Line<'a>, docs: Vec<String>) -> ResourceDecl {
        let (name, store) = match parse_resource_header(line.content) {
            Ok(header) => header,
            Err(message) => {
                self.error(line, message);
                ("".to_string(), None)
            }
        };
        self.index += 1;
        let members = if self.has_child_body(line.indent) {
            self.parse_resource_members(line.indent)
        } else {
            self.error(line, "expected an indented resource body");
            Vec::new()
        };

        ResourceDecl {
            docs,
            name,
            store,
            members,
            span: line.span(),
        }
    }

    fn parse_resource_members(&mut self, parent_indent: usize) -> Vec<ResourceMember> {
        let mut members = Vec::new();
        let mut docs = Vec::new();
        let mut stable_id = None;
        let Some(block_indent) = self.resource_block_indent(parent_indent) else {
            return members;
        };

        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if self.reject_tabs(line) {
                self.index += 1;
                continue;
            }
            if line.is_blank() || line.is_comment() {
                self.index += 1;
                continue;
            }
            if line.indent <= parent_indent {
                break;
            }
            if line.indent != block_indent {
                self.error(
                    line,
                    "unexpected indentation in resource body; only groups introduce nested resource members",
                );
                self.index += 1;
                self.skip_deeper_resource_lines(line.indent);
                continue;
            }
            if let Some(doc) = line.doc_comment() {
                docs.push(doc.to_string());
                self.index += 1;
                continue;
            }
            if line.content.starts_with("@id(") {
                stable_id = parse_stable_id(line.content).or_else(|| {
                    self.error(line, "expected @id(\"stable.id\")");
                    None
                });
                self.index += 1;
                continue;
            }

            if line.content.starts_with("index ") {
                match parse_index(line.content) {
                    Ok(index) => members.push(ResourceMember::Index(IndexDecl {
                        docs: std::mem::take(&mut docs),
                        stable_id: stable_id.take(),
                        span: line.span(),
                        ..index
                    })),
                    Err(message) => self.error(line, message),
                }
                self.index += 1;
                continue;
            }

            match parse_field_or_group_head(line.content) {
                Ok(MemberHead::Field {
                    required,
                    name,
                    keys,
                    ty,
                }) => {
                    if !is_type_text(&ty.text) {
                        self.error(line, "expected field type annotation");
                    }
                    members.push(ResourceMember::Field(FieldDecl {
                        docs: std::mem::take(&mut docs),
                        stable_id: stable_id.take(),
                        required,
                        name,
                        keys,
                        ty,
                        span: line.span(),
                    }));
                    self.index += 1;
                }
                Ok(MemberHead::Group { name, keys }) => {
                    self.index += 1;
                    let children = if self.has_child_body(line.indent) {
                        self.parse_resource_members(line.indent)
                    } else {
                        self.error(line, "expected an indented resource group body");
                        Vec::new()
                    };
                    members.push(ResourceMember::Group(GroupDecl {
                        docs: std::mem::take(&mut docs),
                        stable_id: stable_id.take(),
                        name,
                        keys,
                        members: children,
                        span: line.span(),
                    }));
                }
                Err(message) => {
                    self.error(line, message);
                    self.index += 1;
                }
            }
        }

        members
    }

    fn resource_block_indent(&self, parent_indent: usize) -> Option<usize> {
        let mut index = self.index;
        while index < self.lines.len() {
            let line = self.lines[index];
            if line.is_blank() || line.is_comment() {
                index += 1;
                continue;
            }
            if line.indent <= parent_indent {
                return None;
            }
            return Some(line.indent);
        }
        None
    }

    fn skip_deeper_resource_lines(&mut self, bad_indent: usize) {
        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if line.is_blank() || line.is_comment() {
                self.index += 1;
                continue;
            }
            if line.indent > bad_indent {
                self.index += 1;
                continue;
            }
            break;
        }
    }

    fn parse_function(&mut self, line: Line<'a>, docs: Vec<String>) -> FunctionDecl {
        let header = match parse_function_header(line.content) {
            Ok(header) => header,
            Err(message) => {
                self.error(line, message);
                FunctionHead {
                    public: false,
                    name: String::new(),
                    params: Vec::new(),
                    return_type: None,
                }
            }
        };

        self.index += 1;
        let body_start = self.index;
        if self.has_child_body(line.indent) {
            while self.index < self.lines.len() {
                let next = self.lines[self.index];
                if self.reject_tabs(next) {
                    self.index += 1;
                    continue;
                }
                if next.is_blank() || next.is_comment() || next.doc_comment().is_some() {
                    self.index += 1;
                    continue;
                }
                if next.indent <= line.indent {
                    break;
                }
                self.index += 1;
            }
        } else {
            self.error(line, "expected an indented function body");
        }
        let body = self.parse_body_block(body_start, self.index, line.span());

        FunctionDecl {
            docs,
            public: header.public,
            name: header.name,
            params: header.params,
            return_type: header.return_type,
            body,
            span: line.span(),
        }
    }

    /// Parse the statements of a function body from the lines in
    /// `[body_start, body_end)`. Works on the file-wide token stream so that
    /// statements spanning several physical lines inside open delimiters stay
    /// together.
    fn parse_body_block(
        &mut self,
        body_start: usize,
        body_end: usize,
        header_span: SourceSpan,
    ) -> Block {
        let Some(span) = span_for_lines(&self.lines, body_start, body_end) else {
            return Block {
                statements: Vec::new(),
                comments: Vec::new(),
                span: header_span,
            };
        };
        let tokens = tokens_in_range(self.tokens, span.start_byte, span.end_byte);
        let (statements, comments, diagnostics) =
            StmtParser::new(self.source, tokens).parse_block();
        self.diagnostics.extend(diagnostics);
        Block {
            statements,
            comments,
            span,
        }
    }

    fn has_child_body(&self, parent_indent: usize) -> bool {
        let mut index = self.index;
        while index < self.lines.len() {
            let line = self.lines[index];
            if line.is_blank() || line.is_comment() || line.doc_comment().is_some() {
                index += 1;
                continue;
            }
            return line.indent > parent_indent;
        }
        false
    }

    fn reject_tabs(&self, line: Line<'a>) -> bool {
        // The lexer reports tabs with a dedicated diagnostic; this parser only
        // needs to know whether to skip the line, since tabs corrupt
        // indentation-based parsing.
        line.text.contains('\t')
    }

    fn error(&mut self, line: Line<'a>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            code: PARSE_SYNTAX,
            kind: "parse",
            severity: Severity::Error,
            message: message.into(),
            help: None,
            span: line.span_at_content(),
            line: line.number,
            column: (line.indent + 1) as u32,
        });
    }
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

    fn span(&self) -> SourceSpan {
        SourceSpan {
            start_byte: self.start_byte,
            end_byte: self.end_byte,
            line: self.number,
            column: 1,
        }
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

fn parse_resource_header(content: &str) -> Result<(String, Option<SavedRoot>), &'static str> {
    let rest = content
        .strip_prefix("resource ")
        .ok_or("expected resource declaration")?
        .trim();
    let Some((name, rest)) = read_identifier(rest) else {
        return Err("expected resource name");
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok((name.to_string(), None));
    }
    let rest = rest
        .strip_prefix("at ")
        .ok_or("expected `at ^root` after resource name")?
        .trim();
    let rest = rest
        .strip_prefix('^')
        .ok_or("expected saved root beginning with `^`")?;
    let Some((root, rest)) = read_identifier(rest) else {
        return Err("expected saved root name");
    };
    let rest = rest.trim();
    let keys = if rest.is_empty() {
        Vec::new()
    } else {
        parse_key_params(rest)?
    };
    Ok((
        name.to_string(),
        Some(SavedRoot {
            root: root.to_string(),
            keys,
        }),
    ))
}

fn parse_function_header(content: &str) -> Result<FunctionHead, &'static str> {
    let (public, rest) = if let Some(rest) = content.strip_prefix("pub ") {
        (true, rest)
    } else if let Some(rest) = content.strip_prefix("internal ") {
        if rest.starts_with("fn ") {
            return Err("function visibility is only `pub` or module-private; remove `internal`");
        }
        (false, content)
    } else if let Some(rest) = content.strip_prefix("private ") {
        if rest.starts_with("fn ") {
            return Err("function visibility is only `pub` or module-private; remove `private`");
        }
        (false, content)
    } else {
        (false, content)
    };
    let rest = rest
        .strip_prefix("fn ")
        .ok_or("expected fn declaration")?
        .trim();
    let Some((name, after_name)) = read_identifier(rest) else {
        return Err("expected function name");
    };
    let after_name = after_name.trim_start();
    if after_name.starts_with('<') {
        return Err("user-defined generics are not used in Marrow");
    }
    let (params_text, after_params) =
        parse_parenthesized_prefix(after_name).ok_or("expected function parameter list")?;
    let params = parse_params(params_text)?;
    let after_params = after_params.trim();
    let return_type = if after_params.is_empty() {
        None
    } else {
        let ty = after_params
            .strip_prefix(':')
            .ok_or("expected return type after `:`")?
            .trim();
        if ty.is_empty() {
            return Err("expected return type after `:`");
        }
        if !is_type_text(ty) {
            return Err("expected return type annotation");
        }
        Some(type_ref(ty))
    };

    Ok(FunctionHead {
        public,
        name: name.to_string(),
        params,
        return_type,
    })
}

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

fn parse_field_or_group_head(content: &str) -> Result<MemberHead, &'static str> {
    let (required, rest) = if let Some(rest) = content.strip_prefix("required ") {
        (true, rest.trim())
    } else {
        (false, content.trim())
    };
    let Some((name, rest)) = read_identifier(rest) else {
        return Err("expected resource member name");
    };
    let mut rest = rest.trim_start();
    let keys = if rest.starts_with('(') {
        let (inside, tail) = parse_parenthesized_prefix(rest)
            .ok_or("expected closing `)` in keyed resource member")?;
        rest = tail.trim_start();
        parse_key_params_inside(inside)?
    } else {
        Vec::new()
    };
    if let Some(ty) = rest.strip_prefix(':') {
        let ty = ty.trim();
        if !is_type_text(ty) {
            return Err("expected field type after `:`");
        }
        return Ok(MemberHead::Field {
            required,
            name: name.to_string(),
            keys,
            ty: type_ref(ty),
        });
    }
    if required {
        return Err("required resource members must declare a field type");
    }
    if rest.is_empty() {
        return Ok(MemberHead::Group {
            name: name.to_string(),
            keys,
        });
    }
    Err("expected resource field, keyed field, group, or index")
}

fn parse_index(content: &str) -> Result<IndexDecl, &'static str> {
    let rest = content
        .strip_prefix("index ")
        .ok_or("expected index declaration")?
        .trim();
    let Some((name, rest)) = read_identifier(rest) else {
        return Err("expected index name");
    };
    let rest = rest.trim_start();
    let (args_text, tail) =
        parse_parenthesized_prefix(rest).ok_or("expected index argument list")?;
    if args_text.trim().is_empty() {
        return Err("expected at least one index argument");
    }
    let args = split_commas(args_text)?;
    if !args.iter().all(|arg| is_field_path(arg)) {
        return Err("expected index field path");
    }
    let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
    let tail = tail.trim();
    let unique = match tail {
        "" => false,
        "unique" => true,
        _ => return Err("expected `unique` or end of index declaration"),
    };
    Ok(IndexDecl {
        docs: Vec::new(),
        stable_id: None,
        name: name.to_string(),
        args,
        unique,
        span: SourceSpan::default(),
    })
}

fn parse_key_params(text: &str) -> Result<Vec<KeyParam>, &'static str> {
    let (inside, tail) = parse_parenthesized_prefix(text).ok_or("expected key parameter list")?;
    if !tail.trim().is_empty() {
        return Err("unexpected text after key parameter list");
    }
    parse_key_params_inside(inside)
}

fn parse_key_params_inside(text: &str) -> Result<Vec<KeyParam>, &'static str> {
    if text.trim().is_empty() {
        return Err("expected at least one key parameter");
    }
    let mut params = Vec::new();
    for part in split_commas(text)? {
        let (name, ty) = parse_name_type(part);
        let Some(ty) = ty else {
            return Err("expected key type annotation");
        };
        if !is_identifier(name) {
            return Err("expected key name");
        }
        if !is_type_text(ty) {
            return Err("expected key type annotation");
        }
        params.push(KeyParam {
            name: name.to_string(),
            ty: type_ref(ty),
        });
    }
    Ok(params)
}

fn parse_params(text: &str) -> Result<Vec<ParamDecl>, &'static str> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for part in split_commas(text)? {
        let (mode, rest) = if let Some(rest) = part.strip_prefix("out ") {
            (Some(ParamMode::Out), rest.trim())
        } else if let Some(rest) = part.strip_prefix("inout ") {
            (Some(ParamMode::InOut), rest.trim())
        } else {
            (None, part)
        };
        let (name, ty) = parse_name_type(rest);
        let Some(ty) = ty else {
            return Err("expected parameter type annotation");
        };
        if !is_identifier(name) {
            return Err("expected parameter name");
        }
        if ty.contains('=') {
            return Err("parameter defaults are not used in Marrow");
        }
        if !is_type_text(ty) {
            return Err("expected parameter type annotation");
        }
        params.push(ParamDecl {
            mode,
            name: name.to_string(),
            ty: type_ref(ty),
        });
    }
    Ok(params)
}

fn parse_name_type(text: &str) -> (&str, Option<&str>) {
    match split_once_trimmed(text, ':') {
        Some((name, ty)) => (name, Some(ty)),
        None => (text.trim(), None),
    }
}

/// Return the tokens whose spans fall entirely within `[start_byte, end_byte)`.
/// Tokens are sorted by start byte and (in the value positions that call this)
/// have monotonic end bytes, so the matches form one contiguous window. Nested
/// interpolation tokens break that monotonicity but do not occur here.
fn tokens_in_range(tokens: &[Token], start_byte: usize, end_byte: usize) -> &[Token] {
    let first = tokens.partition_point(|token| token.span.start_byte < start_byte);
    let last = first + tokens[first..].partition_point(|token| token.span.end_byte <= end_byte);
    &tokens[first..last]
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline
            | TokenKind::Eof
            | TokenKind::Comment
            | TokenKind::DocComment
            | TokenKind::Indent
            | TokenKind::Dedent
    )
}

/// Recursive-descent parser for a single Marrow expression over a token slice
/// with file-absolute spans. It covers the primary, postfix, unary, and binary
/// precedence levels, including calls and saved paths. A value it does not
/// structure is reported whole as `Expression::Unparsed`.
struct ExprParser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
}

impl<'a> ExprParser<'a> {
    fn new(source: &'a str, tokens: &[Token]) -> Self {
        let tokens = tokens
            .iter()
            .copied()
            .filter(|token| !is_trivia(token.kind))
            .collect();
        Self {
            source,
            tokens,
            pos: 0,
        }
    }

    fn parse_value(mut self, fallback_span: SourceSpan, fallback_text: &str) -> Expression {
        let unparsed = || Expression::Unparsed {
            text: fallback_text.trim().to_string(),
            span: fallback_span,
        };
        if self.tokens.is_empty() {
            return unparsed();
        }
        match self.expression() {
            Some(expr) if self.pos == self.tokens.len() => expr,
            _ => unparsed(),
        }
    }

    /// Parse the whole token slice as one expression, returning `None` unless
    /// every significant token is consumed.
    fn parse_complete(mut self) -> Option<Expression> {
        if self.tokens.is_empty() {
            return None;
        }
        let expr = self.expression()?;
        (self.pos == self.tokens.len()).then_some(expr)
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
        self.or_expr()
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
        let mut left = self.equality_expr()?;
        while matches!(self.peek(), Some(TokenKind::Keyword(Keyword::And))) {
            self.advance();
            let right = self.equality_expr()?;
            left = binary_expr(BinaryOp::And, left, right);
        }
        Some(left)
    }

    fn equality_expr(&mut self) -> Option<Expression> {
        let left = self.comparison_expr()?;
        let op = match self.peek() {
            Some(TokenKind::Equal) => BinaryOp::Equal,
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
        let left = self.concat_expr()?;
        let op = match self.peek() {
            Some(TokenKind::DotDot) => BinaryOp::RangeExclusive,
            Some(TokenKind::DotDotEqual) => BinaryOp::RangeInclusive,
            _ => return Some(left),
        };
        self.advance();
        let right = self.concat_expr()?;
        Some(binary_expr(op, left, right))
    }

    fn concat_expr(&mut self) -> Option<Expression> {
        let mut left = self.additive_expr()?;
        while matches!(self.peek(), Some(TokenKind::Underscore)) {
            self.advance();
            let right = self.additive_expr()?;
            left = binary_expr(BinaryOp::Concat, left, right);
        }
        Some(left)
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
        let operand = self.unary_expr()?;
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
                    self.advance();
                    let args = self.arguments()?;
                    if !matches!(self.peek(), Some(TokenKind::RightParen)) {
                        return None;
                    }
                    let close = self.advance();
                    let span = join_spans(expr.span(), close.span);
                    expr = Expression::Call {
                        callee: Box::new(expr),
                        args,
                        span,
                    };
                }
                Some(TokenKind::Dot) => {
                    self.advance();
                    let segment = *self.tokens.get(self.pos)?;
                    let (name, quoted) = match segment.kind {
                        TokenKind::Identifier => (segment.text(self.source).to_string(), false),
                        // A quoted segment names data with a non-identifier name,
                        // e.g. `^books(id)."old-title"`. Store the raw inner text,
                        // escapes unresolved like other string literals. An
                        // unterminated string (already a lexer error) lacks a
                        // closing quote, so fall back to empty rather than panic.
                        TokenKind::String => {
                            let text = segment.text(self.source);
                            let inner = text
                                .strip_prefix('"')
                                .and_then(|rest| rest.strip_suffix('"'))
                                .unwrap_or("");
                            (inner.to_string(), true)
                        }
                        _ => return None,
                    };
                    self.advance();
                    let span = join_spans(expr.span(), segment.span);
                    expr = Expression::Field {
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

    fn arguments(&mut self) -> Option<Vec<Argument>> {
        let mut args = Vec::new();
        if matches!(self.peek(), Some(TokenKind::RightParen)) {
            return Some(args);
        }
        loop {
            args.push(self.argument()?);
            if !matches!(self.peek(), Some(TokenKind::Comma)) {
                break;
            }
            self.advance();
            if matches!(self.peek(), Some(TokenKind::RightParen)) {
                break;
            }
        }
        Some(args)
    }

    fn argument(&mut self) -> Option<Argument> {
        let mode = match self.peek() {
            Some(TokenKind::Keyword(Keyword::Out)) => Some(ArgMode::Out),
            Some(TokenKind::Keyword(Keyword::InOut)) => Some(ArgMode::InOut),
            _ => None,
        };
        if mode.is_some() {
            self.advance();
        }
        let name = if mode.is_none()
            && matches!(self.peek(), Some(TokenKind::Identifier))
            && matches!(self.peek_at(1), Some(TokenKind::Colon))
        {
            let identifier = self.advance();
            self.advance();
            Some(identifier.text(self.source).to_string())
        } else {
            None
        };
        let value = self.expression()?;
        Some(Argument { mode, name, value })
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
            // A type keyword leading a `::` path is the start of a name, as in the
            // short-form `bytes::length(...)` after `use std::bytes` (the same
            // keyword is already valid mid-path, e.g. `std::bytes::length`).
            // `name_expr` accepts callable keywords as path segments.
            TokenKind::Keyword(keyword)
                if is_callable_keyword(keyword)
                    && matches!(self.peek_at(1), Some(TokenKind::DoubleColon)) =>
            {
                self.name_expr()
            }
            // Conversion types and `Error` are only values when called, e.g.
            // `int(value)` or `Error(code: ...)`. A bare type keyword is not an
            // expression, so require a following `(`.
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

fn binary_expr(op: BinaryOp, left: Expression, right: Expression) -> Expression {
    let span = join_spans(left.span(), right.span());
    Expression::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
        span,
    }
}

fn join_spans(start: SourceSpan, end: SourceSpan) -> SourceSpan {
    SourceSpan {
        start_byte: start.start_byte,
        end_byte: end.end_byte,
        line: start.line,
        column: start.column,
    }
}

/// Statement keywords that introduce one or more indented blocks. Most have
/// dedicated parsers; this guards the fallback that swallows a block-introducing
/// keyword appearing where it cannot be structured (such as a stray `else`),
/// consuming its nested block as `Statement::Unparsed` so following statements
/// still parse.
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
    /// malformed statement becomes a deterministic diagnostic instead of a
    /// silent `Statement::Unparsed`.
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
                    // A stray nested block (e.g. under a compound statement left
                    // Unparsed). Skip it rather than mis-parse.
                    self.skip_block();
                }
                _ => statements.push(self.statement()),
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

    fn statement(&mut self) -> Statement {
        // A loop label (`outer:`) precedes a `while` or `for`. `try_loop_label`
        // only consumes the label when one of those keywords follows, so the
        // `_` arm is necessarily `while`.
        if let Some((label, label_span)) = self.try_loop_label() {
            return match self.peek() {
                Some(TokenKind::Keyword(Keyword::For)) => {
                    self.for_stmt(Some(label), Some(label_span))
                }
                _ => self.while_stmt(Some(label), Some(label_span)),
            };
        }

        match self.tokens[self.pos].kind {
            TokenKind::Keyword(Keyword::If) => return self.if_stmt(),
            TokenKind::Keyword(Keyword::While) => return self.while_stmt(None, None),
            TokenKind::Keyword(Keyword::For) => return self.for_stmt(None, None),
            TokenKind::Keyword(Keyword::Transaction) => return self.transaction_stmt(),
            TokenKind::Keyword(Keyword::Lock) => return self.lock_stmt(),
            TokenKind::Keyword(Keyword::Try) => return self.try_stmt(),
            TokenKind::Keyword(keyword) if is_compound_statement_keyword(keyword) => {
                return self.unparsed_compound();
            }
            _ => {}
        }

        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let statement = match parse_simple_statement(self.source, line) {
            Some(statement) => statement,
            None => {
                let span = line_span(line);
                self.diagnostics.push(Diagnostic {
                    code: PARSE_SYNTAX,
                    kind: "parse",
                    severity: Severity::Error,
                    message: "expected a statement".to_string(),
                    help: None,
                    span,
                    line: span.line,
                    column: span.column,
                });
                Statement::Unparsed { span }
            }
        };
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

    fn for_stmt(&mut self, label: Option<String>, label_span: Option<SourceSpan>) -> Statement {
        let keyword = self.advance(); // `for`
        let start = label_span.unwrap_or(keyword.span);
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let header = &self.tokens[self.pos..content_end];
        let parsed = parse_for_header(self.source, header);
        self.pos = (newline + 1).min(self.tokens.len());
        let body = self.block_body();

        match parsed {
            Some((binding, iterable)) => Statement::For {
                label,
                binding,
                iterable,
                span: join_spans(start, body.span),
                body,
            },
            None => Statement::Unparsed {
                span: join_spans(start, body.span),
            },
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

    /// Parse the expression that ends the current header line, consuming up to
    /// and including its `NEWLINE`.
    fn header_expression(&mut self) -> Expression {
        let newline = self.find_line_end();
        let content_end = self.split_trailing_comment(newline);
        let line = &self.tokens[self.pos..content_end];
        let expr =
            expr_of(self.source, line).unwrap_or_else(|| unparsed_expression(self.source, line));
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

    fn unparsed_compound(&mut self) -> Statement {
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
        Statement::Unparsed {
            span: join_spans(start, end),
        }
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

fn parse_simple_statement(source: &str, line: &[Token]) -> Option<Statement> {
    let first = line.first()?;
    match first.kind {
        TokenKind::Keyword(Keyword::Const) => parse_const_or_var(source, line, false),
        TokenKind::Keyword(Keyword::Var) => parse_const_or_var(source, line, true),
        TokenKind::Keyword(Keyword::Return) => parse_return(source, line),
        TokenKind::Keyword(Keyword::Delete) => {
            let value = expr_of(source, &line[1..])?;
            Some(Statement::Delete {
                span: join_spans(first.span, value.span()),
                path: value,
            })
        }
        TokenKind::Keyword(Keyword::Throw) => {
            let value = expr_of(source, &line[1..])?;
            Some(Statement::Throw {
                span: join_spans(first.span, value.span()),
                value,
            })
        }
        TokenKind::Keyword(Keyword::Merge) => parse_merge(source, line),
        TokenKind::Keyword(Keyword::Break) => parse_break_or_continue(source, line, true),
        TokenKind::Keyword(Keyword::Continue) => parse_break_or_continue(source, line, false),
        _ => parse_assign_or_expr(source, line),
    }
}

fn parse_const_or_var(source: &str, line: &[Token], is_var: bool) -> Option<Statement> {
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
            let value = expr_of(source, &line[index + 1..])?;
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

fn parse_return(source: &str, line: &[Token]) -> Option<Statement> {
    let keyword = line[0];
    if line.len() == 1 {
        return Some(Statement::Return {
            value: None,
            span: keyword.span,
        });
    }
    let value = expr_of(source, &line[1..])?;
    Some(Statement::Return {
        span: join_spans(keyword.span, value.span()),
        value: Some(value),
    })
}

fn parse_merge(source: &str, line: &[Token]) -> Option<Statement> {
    let keyword = line[0];
    let rest = &line[1..];
    let equal = find_top_level_equal(rest)?;
    let target = expr_of(source, &rest[..equal])?;
    let value = expr_of(source, &rest[equal + 1..])?;
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

fn parse_assign_or_expr(source: &str, line: &[Token]) -> Option<Statement> {
    if let Some(equal) = find_top_level_equal(line) {
        let target = expr_of(source, &line[..equal])?;
        let value = expr_of(source, &line[equal + 1..])?;
        Some(Statement::Assign {
            span: join_spans(target.span(), value.span()),
            target,
            value,
        })
    } else {
        let value = expr_of(source, line)?;
        Some(Statement::Expr {
            span: value.span(),
            value,
        })
    }
}

/// Index of the first top-level `=` (assignment separator), skipping `=` nested
/// inside parentheses or brackets where it means equality.
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

/// Parse a `for` header `binding in iterable` into the loop binding and the
/// iterable expression. Returns `None` if the `in` keyword or binding is
/// malformed.
fn parse_for_header(source: &str, header: &[Token]) -> Option<(ForBinding, Expression)> {
    let in_index = find_top_level(header, TokenKind::Keyword(Keyword::In))?;
    let binding = parse_for_binding(source, &header[..in_index])?;
    let iterable = expr_of(source, &header[in_index + 1..])?;
    Some((binding, iterable))
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

/// Report positional arguments that follow a named argument. After the first
/// named argument, every remaining argument must be named, because a positional
/// argument after a named one would silently back-fill an earlier parameter. The
/// argument list is parsed before its ordering matters, so the rule is checked
/// here over the built tree, where every call's arguments and spans are known.
fn report_positional_after_named(file: &SourceFile, diagnostics: &mut Vec<Diagnostic>) {
    for declaration in &file.declarations {
        match declaration {
            Declaration::Const(decl) => walk_expr_arguments(&decl.value, diagnostics),
            Declaration::Function(decl) => walk_block_arguments(&decl.body, diagnostics),
            // Resource members are typed declarations, not expressions.
            Declaration::Resource(_) => {}
        }
    }
}

fn walk_block_arguments(block: &Block, diagnostics: &mut Vec<Diagnostic>) {
    for statement in &block.statements {
        walk_statement_arguments(statement, diagnostics);
    }
}

fn walk_statement_arguments(statement: &Statement, diagnostics: &mut Vec<Diagnostic>) {
    match statement {
        Statement::Const { value, .. }
        | Statement::Expr { value, .. }
        | Statement::Throw { value, .. } => walk_expr_arguments(value, diagnostics),
        Statement::Var { value, .. } | Statement::Return { value, .. } => {
            if let Some(value) = value {
                walk_expr_arguments(value, diagnostics);
            }
        }
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            walk_expr_arguments(target, diagnostics);
            walk_expr_arguments(value, diagnostics);
        }
        Statement::Delete { path, .. } => walk_expr_arguments(path, diagnostics),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_expr_arguments(condition, diagnostics);
            walk_block_arguments(then_block, diagnostics);
            for else_if in else_ifs {
                walk_expr_arguments(&else_if.condition, diagnostics);
                walk_block_arguments(&else_if.block, diagnostics);
            }
            if let Some(else_block) = else_block {
                walk_block_arguments(else_block, diagnostics);
            }
        }
        Statement::While {
            condition, body, ..
        } => {
            walk_expr_arguments(condition, diagnostics);
            walk_block_arguments(body, diagnostics);
        }
        Statement::For { iterable, body, .. } => {
            walk_expr_arguments(iterable, diagnostics);
            walk_block_arguments(body, diagnostics);
        }
        Statement::Transaction { body, .. } => walk_block_arguments(body, diagnostics),
        Statement::Lock { path, body, .. } => {
            walk_expr_arguments(path, diagnostics);
            walk_block_arguments(body, diagnostics);
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            walk_block_arguments(body, diagnostics);
            if let Some(catch) = catch {
                walk_block_arguments(&catch.block, diagnostics);
            }
            if let Some(finally) = finally {
                walk_block_arguments(finally, diagnostics);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Unparsed { .. } => {}
    }
}

fn walk_expr_arguments(expression: &Expression, diagnostics: &mut Vec<Diagnostic>) {
    match expression {
        Expression::Call { callee, args, .. } => {
            walk_expr_arguments(callee, diagnostics);
            let mut seen_named = false;
            for arg in args {
                // A plain positional argument (no name, no `out`/`inout` mode)
                // after a named one breaks the grammar contract; point at it.
                // Mode arguments are not plain positionals and keep their own
                // rules (see `parses_named_and_moded_call_arguments`).
                if seen_named && arg.name.is_none() && arg.mode.is_none() {
                    let span = arg.value.span();
                    diagnostics.push(Diagnostic {
                        code: PARSE_SYNTAX,
                        kind: "parse",
                        severity: Severity::Error,
                        message: "a positional argument cannot follow a named argument".to_string(),
                        help: Some(
                            "name this argument or move it before the named arguments".to_string(),
                        ),
                        span,
                        line: span.line,
                        column: span.column,
                    });
                }
                seen_named |= arg.name.is_some();
                walk_expr_arguments(&arg.value, diagnostics);
            }
        }
        Expression::Field { base, .. } => walk_expr_arguments(base, diagnostics),
        Expression::Unary { operand, .. } => walk_expr_arguments(operand, diagnostics),
        Expression::Binary { left, right, .. } => {
            walk_expr_arguments(left, diagnostics);
            walk_expr_arguments(right, diagnostics);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    walk_expr_arguments(expr, diagnostics);
                }
            }
        }
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Unparsed { .. } => {}
    }
}

fn expr_of(source: &str, tokens: &[Token]) -> Option<Expression> {
    ExprParser::new(source, tokens).parse_complete()
}

fn type_ref_from_tokens(source: &str, tokens: &[Token]) -> TypeRef {
    let start = tokens[0].span.start_byte;
    let end = tokens[tokens.len() - 1].span.end_byte;
    TypeRef {
        text: source[start..end].trim().to_string(),
    }
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

fn unparsed_expression(source: &str, tokens: &[Token]) -> Expression {
    let span = line_span(tokens);
    Expression::Unparsed {
        text: source
            .get(span.start_byte..span.end_byte)
            .unwrap_or_default()
            .trim()
            .to_string(),
        span,
    }
}

fn parse_stable_id(content: &str) -> Option<String> {
    let rest = content.strip_prefix("@id(")?.strip_suffix(')')?.trim();
    let body = rest.strip_prefix('"')?.strip_suffix('"')?;
    Some(body.to_string())
}

fn parse_parenthesized_prefix(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if !text.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((&text[1..index], &text[index + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn read_identifier(text: &str) -> Option<(&str, &str)> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !is_identifier_start_char(first) {
        return None;
    }
    let mut end = first.len_utf8();
    for (index, ch) in chars {
        if is_identifier_continue_char(ch) {
            end = index + ch.len_utf8();
        } else {
            return Some((&text[..index], &text[index..]));
        }
    }
    Some((&text[..end], &text[end..]))
}

fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "module" => Keyword::Module,
        "use" => Keyword::Use,
        "pub" => Keyword::Pub,
        "fn" => Keyword::Fn,
        "resource" => Keyword::Resource,
        "at" => Keyword::At,
        "index" => Keyword::Index,
        "unique" => Keyword::Unique,
        "required" => Keyword::Required,
        "const" => Keyword::Const,
        "var" => Keyword::Var,
        "if" => Keyword::If,
        "else" => Keyword::Else,
        "while" => Keyword::While,
        "for" => Keyword::For,
        "in" => Keyword::In,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "return" => Keyword::Return,
        "delete" => Keyword::Delete,
        "merge" => Keyword::Merge,
        "transaction" => Keyword::Transaction,
        "lock" => Keyword::Lock,
        "try" => Keyword::Try,
        "catch" => Keyword::Catch,
        "finally" => Keyword::Finally,
        "throw" => Keyword::Throw,
        "out" => Keyword::Out,
        "inout" => Keyword::InOut,
        "true" => Keyword::True,
        "false" => Keyword::False,
        "not" => Keyword::Not,
        "and" => Keyword::And,
        "or" => Keyword::Or,
        "int" => Keyword::Int,
        "decimal" => Keyword::Decimal,
        "bool" => Keyword::Bool,
        "string" => Keyword::String,
        "bytes" => Keyword::Bytes,
        "date" => Keyword::Date,
        "instant" => Keyword::Instant,
        "duration" => Keyword::Duration,
        "sequence" => Keyword::Sequence,
        "unknown" => Keyword::Unknown,
        "Error" => Keyword::Error,
        "ErrorCode" => Keyword::ErrorCode,
        _ => return None,
    })
}

fn is_identifier_start_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue_char(ch: char) -> bool {
    is_identifier_start_char(ch) || ch.is_ascii_digit()
}

fn is_identifier(text: &str) -> bool {
    let Some((ident, rest)) = read_identifier(text) else {
        return false;
    };
    ident == text && rest.is_empty()
}

fn is_qualified_name(text: &str) -> bool {
    let mut parts = text.split("::");
    let Some(first) = parts.next() else {
        return false;
    };
    is_identifier(first) && parts.all(is_identifier)
}

fn is_type_text(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('=') {
        return false;
    }
    if let Some(inner) = text
        .strip_prefix("sequence[")
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return is_type_text(inner);
    }
    is_qualified_name(text)
}

fn is_field_path(text: &str) -> bool {
    let mut parts = text.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    is_identifier(first) && parts.all(is_identifier)
}

fn split_commas(text: &str) -> Result<Vec<&str>, &'static str> {
    let raw = text.split(',').collect::<Vec<_>>();
    let mut parts = Vec::new();
    for (index, part) in raw.iter().enumerate() {
        let part = part.trim();
        if part.is_empty() {
            if index + 1 == raw.len() {
                continue;
            }
            return Err("expected item between commas");
        }
        parts.push(part);
    }
    Ok(parts)
}

fn split_once_trimmed(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let (left, right) = text.split_once(delimiter)?;
    Some((left.trim(), right.trim()))
}

fn type_ref(text: &str) -> TypeRef {
    TypeRef {
        text: text.trim().to_string(),
    }
}

fn starts_function(content: &str) -> bool {
    content.starts_with("fn ") || content.starts_with("pub fn ")
}

fn span_for_lines(lines: &[Line<'_>], start: usize, end: usize) -> Option<SourceSpan> {
    if start >= end {
        return None;
    }
    let first = lines[start];
    let last = lines[end - 1];
    Some(SourceSpan {
        start_byte: first.start_byte,
        end_byte: last.end_byte,
        line: first.number,
        column: 1,
    })
}
