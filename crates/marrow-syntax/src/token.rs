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
    Newline,
    Eof,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    /// Block delimiters. A bare `{`/`}` in source is always a block delimiter;
    /// interpolation holes and `{{`/`}}` escapes are handled inside `$"..."` before
    /// these are produced.
    LeftBrace,
    RightBrace,
    /// The `=>` of a match arm.
    FatArrow,
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
    Alias,
    Type,
    Supports,
    Resource,
    Struct,
    Store,
    Enum,
    Test,
    Assert,
    Match,
    Index,
    Unique,
    Required,
    Const,
    Var,
    Place,
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
    Unset,
    Merge,
    Journal,
    Sensitive,
    Declassify,
    Transaction,
    Lock,
    // Held for the future effect-signature clause (`pub fn f(): T writes ^a reads ^b`).
    // Reserved now so the spelling stays free; the clause itself is not yet grammar.
    Writes,
    Reads,
    Try,
    Require,
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

pub(crate) fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Newline | TokenKind::Eof | TokenKind::Comment | TokenKind::DocComment
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
        "alias" => Keyword::Alias,
        "type" => Keyword::Type,
        "supports" => Keyword::Supports,
        "resource" => Keyword::Resource,
        "struct" => Keyword::Struct,
        "store" => Keyword::Store,
        "enum" => Keyword::Enum,
        "test" => Keyword::Test,
        "assert" => Keyword::Assert,
        "match" => Keyword::Match,
        "index" => Keyword::Index,
        "unique" => Keyword::Unique,
        "required" => Keyword::Required,
        "const" => Keyword::Const,
        "var" => Keyword::Var,
        "place" => Keyword::Place,
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
        "unset" => Keyword::Unset,
        "merge" => Keyword::Merge,
        "journal" => Keyword::Journal,
        "sensitive" => Keyword::Sensitive,
        "declassify" => Keyword::Declassify,
        "transaction" => Keyword::Transaction,
        "lock" => Keyword::Lock,
        "writes" => Keyword::Writes,
        "reads" => Keyword::Reads,
        "try" => Keyword::Try,
        "require" => Keyword::Require,
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

/// Whether `word` names a calendar unit with no fixed span — a month or a year.
/// These read like duration units but their length varies, so a duration word
/// literal spelled with one is refused rather than silently folded.
pub fn is_unfixed_duration_unit(word: &str) -> bool {
    matches!(word, "month" | "months" | "year" | "years")
}

/// The singular and plural spellings of a fixed duration unit, from either spelling,
/// or `None` for a non-unit. A duration word literal agrees in number with its count:
/// the singular for `1`, the plural otherwise.
pub fn duration_unit_forms(unit: &str) -> Option<(&'static str, &'static str)> {
    Some(match unit {
        "second" | "seconds" => ("second", "seconds"),
        "minute" | "minutes" => ("minute", "minutes"),
        "hour" | "hours" => ("hour", "hours"),
        "day" | "days" => ("day", "days"),
        "week" | "weeks" => ("week", "weeks"),
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
