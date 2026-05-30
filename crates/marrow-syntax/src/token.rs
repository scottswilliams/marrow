//! Tokens and the lexical building blocks: the token kinds and keyword set,
//! the lexed-source result, and the small text/range helpers the lexer and the
//! parsers share.

use crate::{Diagnostic, Severity, SourceSpan};

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
    /// A duration literal `NUMBER.UNIT`, such as `1.day` or `2.hours`. The token
    /// text is the whole literal; [`duration_unit_seconds`] names the unit set.
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
    Enum,
    Match,
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
}

/// Return the tokens whose spans fall entirely within `[start_byte, end_byte)`.
/// Tokens are sorted by start byte and (in the value positions that call this)
/// have monotonic end bytes, so the matches form one contiguous window. Nested
/// interpolation tokens break that monotonicity but do not occur here.
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
        "enum" => Keyword::Enum,
        "match" => Keyword::Match,
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
        _ => return None,
    })
}

/// The whole-seconds span of a duration-literal unit, or `None` for a word that
/// is not a unit. The set is closed and every unit has a fixed length, so months
/// and years (which vary) are deliberately absent. Singular and plural spellings
/// name the same span.
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

pub(crate) fn is_type_text(text: &str) -> bool {
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
