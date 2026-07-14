//! Tokens and the lexical building blocks: the token kinds and keyword set,
//! the lexed-source result, and the small text/range helpers the lexer and the
//! parsers share.

use crate::{Diagnostic, SourceSpan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexedSource {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
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
    /// A duration literal `NUMBER.UNIT` (`1.day`); the token text is the whole literal.
    Duration,
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
    EqualEqual,
    BangEqual,
    Question,
    QuestionDot,
    QuestionQuestion,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    PercentEqual,
    Caret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Module,
    Use,
    Pub,
    Fn,
    Resource,
    Store,
    Enum,
    Evolve,
    Match,
    Index,
    Unique,
    Required,
    Const,
    Var,
    Checked,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Return,
    Absent,
    Delete,
    Merge,
    Journal,
    Sensitive,
    Declassify,
    Transaction,
    Lock,
    Try,
    Catch,
    Throw,
    True,
    False,
    Not,
    And,
    Or,
    Is,
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
    Id,
}

/// Type and constructor keywords that may head a single-token call expression.
pub fn is_expression_callable_keyword(keyword: Keyword) -> bool {
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
            | Keyword::Id
    )
}

/// Keywords the expression parser accepts after `::` in a name path. `absent` is
/// a primary value, not a path segment, so it is deliberately excluded.
pub fn is_expression_path_segment_keyword(keyword: Keyword) -> bool {
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

/// The tokens whose spans fall entirely within `[start_byte, end_byte)`. Relies
/// on tokens being sorted by start byte with monotonic end bytes (true in the
/// value positions that call this; nested interpolation would break it).
pub(crate) fn tokens_in_range(tokens: &[Token], start_byte: usize, end_byte: usize) -> &[Token] {
    let first = tokens.partition_point(|token| token.span.start_byte < start_byte);
    let last = first + tokens[first..].partition_point(|token| token.span.end_byte <= end_byte);
    &tokens[first..last]
}

pub(crate) fn is_trivia(kind: TokenKind) -> bool {
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

pub(crate) fn keyword(text: &str) -> Option<Keyword> {
    Some(match text {
        "module" => Keyword::Module,
        "use" => Keyword::Use,
        "pub" => Keyword::Pub,
        "fn" => Keyword::Fn,
        "resource" => Keyword::Resource,
        "store" => Keyword::Store,
        "enum" => Keyword::Enum,
        "evolve" => Keyword::Evolve,
        "match" => Keyword::Match,
        "index" => Keyword::Index,
        "unique" => Keyword::Unique,
        "required" => Keyword::Required,
        "const" => Keyword::Const,
        "var" => Keyword::Var,
        "checked" => Keyword::Checked,
        "if" => Keyword::If,
        "else" => Keyword::Else,
        "while" => Keyword::While,
        "for" => Keyword::For,
        "in" => Keyword::In,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "return" => Keyword::Return,
        "absent" => Keyword::Absent,
        "delete" => Keyword::Delete,
        "merge" => Keyword::Merge,
        "journal" => Keyword::Journal,
        "sensitive" => Keyword::Sensitive,
        "declassify" => Keyword::Declassify,
        "transaction" => Keyword::Transaction,
        "lock" => Keyword::Lock,
        "try" => Keyword::Try,
        "catch" => Keyword::Catch,
        "throw" => Keyword::Throw,
        "true" => Keyword::True,
        "false" => Keyword::False,
        "not" => Keyword::Not,
        "and" => Keyword::And,
        "or" => Keyword::Or,
        "is" => Keyword::Is,
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
        "Id" => Keyword::Id,
        _ => return None,
    })
}

/// The whole-second span of a duration unit (singular and plural alike), or
/// `None` for a non-unit. Months and years are omitted: their length varies.
pub fn duration_unit_seconds(unit: &str) -> Option<i64> {
    Some(match unit {
        "second" | "seconds" => 1,
        "minute" | "minutes" => 60,
        "hour" | "hours" => 3_600,
        "day" | "days" => 86_400,
        "week" | "weeks" => 604_800,
        _ => return None,
    })
}

pub(crate) fn is_identifier_start_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

pub(crate) fn is_identifier_continue_char(ch: char) -> bool {
    is_identifier_start_char(ch) || ch.is_ascii_digit()
}

pub(crate) fn is_identifier(text: &str) -> bool {
    let Some((ident, rest)) = read_identifier(text) else {
        return false;
    };
    ident == text && rest.is_empty()
}

pub(crate) fn is_qualified_name(text: &str) -> bool {
    let mut parts = text.split("::");
    let Some(first) = parts.next() else {
        return false;
    };
    is_identifier(first) && parts.all(is_identifier)
}
