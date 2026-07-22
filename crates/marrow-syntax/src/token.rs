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

/// A token's parser-owned lexical role.
///
/// These classes describe only spelling that the lexer can establish without
/// name resolution. In particular, an identifier remains [`Self::Unscoped`]
/// until a compiler fact assigns it a semantic role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexicalClass {
    Unscoped,
    ControlFlow,
    Declaration,
    Modifier,
    Effect,
    BuiltinType,
    BuiltinValue,
    IntegerLiteral,
    DecimalLiteral,
    DurationLiteral,
    StringLiteral,
    InterpolationString,
    InterpolationDelimiter,
    BytesLiteral,
    Comment,
    DocumentationComment,
    Operator,
    WordOperator,
    Delimiter,
    Punctuation,
    PathSeparator,
    DurableRootSigil,
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

impl Keyword {
    /// Every reserved word, in declaration order.
    pub const ALL: [Self; 60] = [
        Self::Module,
        Self::Use,
        Self::Pub,
        Self::Fn,
        Self::Alias,
        Self::Type,
        Self::Supports,
        Self::Resource,
        Self::Struct,
        Self::Store,
        Self::Enum,
        Self::Test,
        Self::Assert,
        Self::Match,
        Self::Index,
        Self::Unique,
        Self::Required,
        Self::Const,
        Self::Var,
        Self::Place,
        Self::Checked,
        Self::If,
        Self::Else,
        Self::While,
        Self::For,
        Self::In,
        Self::Break,
        Self::Continue,
        Self::Return,
        Self::Absent,
        Self::Delete,
        Self::Unset,
        Self::Merge,
        Self::Journal,
        Self::Sensitive,
        Self::Declassify,
        Self::Transaction,
        Self::Lock,
        Self::Writes,
        Self::Reads,
        Self::Try,
        Self::Require,
        Self::True,
        Self::False,
        Self::Not,
        Self::And,
        Self::Or,
        Self::Is,
        Self::Int,
        Self::Decimal,
        Self::Bool,
        Self::String,
        Self::Bytes,
        Self::Date,
        Self::Instant,
        Self::Duration,
        Self::Unknown,
        Self::Error,
        Self::ErrorCode,
        Self::Id,
    ];

    /// The exact source spelling reserved by the lexer.
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Use => "use",
            Self::Pub => "pub",
            Self::Fn => "fn",
            Self::Alias => "alias",
            Self::Type => "type",
            Self::Supports => "supports",
            Self::Resource => "resource",
            Self::Struct => "struct",
            Self::Store => "store",
            Self::Enum => "enum",
            Self::Test => "test",
            Self::Assert => "assert",
            Self::Match => "match",
            Self::Index => "index",
            Self::Unique => "unique",
            Self::Required => "required",
            Self::Const => "const",
            Self::Var => "var",
            Self::Place => "place",
            Self::Checked => "checked",
            Self::If => "if",
            Self::Else => "else",
            Self::While => "while",
            Self::For => "for",
            Self::In => "in",
            Self::Break => "break",
            Self::Continue => "continue",
            Self::Return => "return",
            Self::Absent => "absent",
            Self::Delete => "delete",
            Self::Unset => "unset",
            Self::Merge => "merge",
            Self::Journal => "journal",
            Self::Sensitive => "sensitive",
            Self::Declassify => "declassify",
            Self::Transaction => "transaction",
            Self::Lock => "lock",
            Self::Writes => "writes",
            Self::Reads => "reads",
            Self::Try => "try",
            Self::Require => "require",
            Self::True => "true",
            Self::False => "false",
            Self::Not => "not",
            Self::And => "and",
            Self::Or => "or",
            Self::Is => "is",
            Self::Int => "int",
            Self::Decimal => "decimal",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Bytes => "bytes",
            Self::Date => "date",
            Self::Instant => "instant",
            Self::Duration => "duration",
            Self::Unknown => "unknown",
            Self::Error => "Error",
            Self::ErrorCode => "ErrorCode",
            Self::Id => "Id",
        }
    }

    /// The lexical role established by this reserved spelling.
    pub const fn lexical_class(self) -> LexicalClass {
        match self {
            Self::Module
            | Self::Use
            | Self::Fn
            | Self::Alias
            | Self::Type
            | Self::Resource
            | Self::Struct
            | Self::Store
            | Self::Enum
            | Self::Test
            | Self::Index
            | Self::Const
            | Self::Var
            | Self::Place => LexicalClass::Declaration,
            Self::Pub
            | Self::Supports
            | Self::Unique
            | Self::Required
            | Self::Checked
            | Self::Journal
            | Self::Sensitive => LexicalClass::Modifier,
            Self::Declassify | Self::Writes | Self::Reads => LexicalClass::Effect,
            Self::Assert
            | Self::Match
            | Self::If
            | Self::Else
            | Self::While
            | Self::For
            | Self::In
            | Self::Break
            | Self::Continue
            | Self::Return
            | Self::Delete
            | Self::Unset
            | Self::Merge
            | Self::Transaction
            | Self::Lock
            | Self::Try
            | Self::Require => LexicalClass::ControlFlow,
            Self::True | Self::False | Self::Absent | Self::Unknown => LexicalClass::BuiltinValue,
            Self::Not | Self::And | Self::Or | Self::Is => LexicalClass::WordOperator,
            Self::Int
            | Self::Decimal
            | Self::Bool
            | Self::String
            | Self::Bytes
            | Self::Date
            | Self::Instant
            | Self::Duration
            | Self::Error
            | Self::ErrorCode
            | Self::Id => LexicalClass::BuiltinType,
        }
    }
}

impl TokenKind {
    /// One representative of every token-kind variant, for read-only inventory
    /// projections. The keyword representative does not replace [`Keyword::ALL`].
    pub const INVENTORY: [Self; 50] = [
        Self::Identifier,
        Self::Integer,
        Self::Decimal,
        Self::Duration,
        Self::String,
        Self::InterpolationStart,
        Self::InterpolationText,
        Self::InterpolationExprStart,
        Self::InterpolationExprEnd,
        Self::InterpolationEnd,
        Self::Bytes,
        Self::Keyword(Keyword::Module),
        Self::Comment,
        Self::DocComment,
        Self::Newline,
        Self::Eof,
        Self::LeftParen,
        Self::RightParen,
        Self::LeftBracket,
        Self::RightBracket,
        Self::LeftBrace,
        Self::RightBrace,
        Self::FatArrow,
        Self::Colon,
        Self::DoubleColon,
        Self::Comma,
        Self::Dot,
        Self::DotDot,
        Self::DotDotEqual,
        Self::Equal,
        Self::EqualEqual,
        Self::BangEqual,
        Self::Question,
        Self::QuestionDot,
        Self::QuestionQuestion,
        Self::Less,
        Self::LessEqual,
        Self::Greater,
        Self::GreaterEqual,
        Self::Plus,
        Self::Minus,
        Self::Star,
        Self::Slash,
        Self::Percent,
        Self::PlusEqual,
        Self::MinusEqual,
        Self::StarEqual,
        Self::SlashEqual,
        Self::PercentEqual,
        Self::Caret,
    ];

    /// The stable variant name used by lexical inventory projections.
    pub const fn inventory_name(self) -> &'static str {
        match self {
            Self::Identifier => "Identifier",
            Self::Integer => "Integer",
            Self::Decimal => "Decimal",
            Self::Duration => "Duration",
            Self::String => "String",
            Self::InterpolationStart => "InterpolationStart",
            Self::InterpolationText => "InterpolationText",
            Self::InterpolationExprStart => "InterpolationExprStart",
            Self::InterpolationExprEnd => "InterpolationExprEnd",
            Self::InterpolationEnd => "InterpolationEnd",
            Self::Bytes => "Bytes",
            Self::Keyword(_) => "Keyword",
            Self::Comment => "Comment",
            Self::DocComment => "DocComment",
            Self::Newline => "Newline",
            Self::Eof => "Eof",
            Self::LeftParen => "LeftParen",
            Self::RightParen => "RightParen",
            Self::LeftBracket => "LeftBracket",
            Self::RightBracket => "RightBracket",
            Self::LeftBrace => "LeftBrace",
            Self::RightBrace => "RightBrace",
            Self::FatArrow => "FatArrow",
            Self::Colon => "Colon",
            Self::DoubleColon => "DoubleColon",
            Self::Comma => "Comma",
            Self::Dot => "Dot",
            Self::DotDot => "DotDot",
            Self::DotDotEqual => "DotDotEqual",
            Self::Equal => "Equal",
            Self::EqualEqual => "EqualEqual",
            Self::BangEqual => "BangEqual",
            Self::Question => "Question",
            Self::QuestionDot => "QuestionDot",
            Self::QuestionQuestion => "QuestionQuestion",
            Self::Less => "Less",
            Self::LessEqual => "LessEqual",
            Self::Greater => "Greater",
            Self::GreaterEqual => "GreaterEqual",
            Self::Plus => "Plus",
            Self::Minus => "Minus",
            Self::Star => "Star",
            Self::Slash => "Slash",
            Self::Percent => "Percent",
            Self::PlusEqual => "PlusEqual",
            Self::MinusEqual => "MinusEqual",
            Self::StarEqual => "StarEqual",
            Self::SlashEqual => "SlashEqual",
            Self::PercentEqual => "PercentEqual",
            Self::Caret => "Caret",
        }
    }

    /// The class known from tokenization alone.
    pub const fn lexical_class(self) -> LexicalClass {
        match self {
            Self::Identifier | Self::Newline | Self::Eof => LexicalClass::Unscoped,
            Self::Integer => LexicalClass::IntegerLiteral,
            Self::Decimal => LexicalClass::DecimalLiteral,
            Self::Duration => LexicalClass::DurationLiteral,
            Self::String => LexicalClass::StringLiteral,
            Self::InterpolationStart | Self::InterpolationText | Self::InterpolationEnd => {
                LexicalClass::InterpolationString
            }
            Self::InterpolationExprStart | Self::InterpolationExprEnd => {
                LexicalClass::InterpolationDelimiter
            }
            Self::Bytes => LexicalClass::BytesLiteral,
            Self::Keyword(keyword) => keyword.lexical_class(),
            Self::Comment => LexicalClass::Comment,
            Self::DocComment => LexicalClass::DocumentationComment,
            Self::LeftParen
            | Self::RightParen
            | Self::LeftBracket
            | Self::RightBracket
            | Self::LeftBrace
            | Self::RightBrace => LexicalClass::Delimiter,
            Self::FatArrow
            | Self::DotDot
            | Self::DotDotEqual
            | Self::Equal
            | Self::EqualEqual
            | Self::BangEqual
            | Self::QuestionDot
            | Self::QuestionQuestion
            | Self::Less
            | Self::LessEqual
            | Self::Greater
            | Self::GreaterEqual
            | Self::Plus
            | Self::Minus
            | Self::Star
            | Self::Slash
            | Self::Percent
            | Self::PlusEqual
            | Self::MinusEqual
            | Self::StarEqual
            | Self::SlashEqual
            | Self::PercentEqual => LexicalClass::Operator,
            Self::Colon | Self::Comma | Self::Dot | Self::Question => LexicalClass::Punctuation,
            Self::DoubleColon => LexicalClass::PathSeparator,
            Self::Caret => LexicalClass::DurableRootSigil,
        }
    }

    /// Exact fixed spelling, when the token is not content-bearing.
    pub const fn fixed_spelling(self) -> Option<&'static str> {
        match self {
            Self::Identifier
            | Self::Integer
            | Self::Decimal
            | Self::Duration
            | Self::String
            | Self::InterpolationText
            | Self::Bytes
            | Self::Comment
            | Self::DocComment
            | Self::Newline
            | Self::Eof => None,
            Self::InterpolationStart => Some("$\""),
            Self::InterpolationExprStart | Self::LeftBrace => Some("{"),
            Self::InterpolationExprEnd | Self::RightBrace => Some("}"),
            Self::InterpolationEnd => Some("\""),
            Self::Keyword(keyword) => Some(keyword.spelling()),
            Self::LeftParen => Some("("),
            Self::RightParen => Some(")"),
            Self::LeftBracket => Some("["),
            Self::RightBracket => Some("]"),
            Self::FatArrow => Some("=>"),
            Self::Colon => Some(":"),
            Self::DoubleColon => Some("::"),
            Self::Comma => Some(","),
            Self::Dot => Some("."),
            Self::DotDot => Some(".."),
            Self::DotDotEqual => Some("..="),
            Self::Equal => Some("="),
            Self::EqualEqual => Some("=="),
            Self::BangEqual => Some("!="),
            Self::Question => Some("?"),
            Self::QuestionDot => Some("?."),
            Self::QuestionQuestion => Some("??"),
            Self::Less => Some("<"),
            Self::LessEqual => Some("<="),
            Self::Greater => Some(">"),
            Self::GreaterEqual => Some(">="),
            Self::Plus => Some("+"),
            Self::Minus => Some("-"),
            Self::Star => Some("*"),
            Self::Slash => Some("/"),
            Self::Percent => Some("%"),
            Self::PlusEqual => Some("+="),
            Self::MinusEqual => Some("-="),
            Self::StarEqual => Some("*="),
            Self::SlashEqual => Some("/="),
            Self::PercentEqual => Some("%="),
            Self::Caret => Some("^"),
        }
    }
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

#[derive(Clone, Copy)]
struct DurationUnitFacts {
    singular: &'static str,
    plural: &'static str,
    seconds: i64,
}

const FIXED_DURATION_UNITS: [DurationUnitFacts; 5] = [
    DurationUnitFacts {
        singular: "second",
        plural: "seconds",
        seconds: 1,
    },
    DurationUnitFacts {
        singular: "minute",
        plural: "minutes",
        seconds: 60,
    },
    DurationUnitFacts {
        singular: "hour",
        plural: "hours",
        seconds: 3_600,
    },
    DurationUnitFacts {
        singular: "day",
        plural: "days",
        seconds: 86_400,
    },
    DurationUnitFacts {
        singular: "week",
        plural: "weeks",
        seconds: 604_800,
    },
];

/// Every fixed-span duration-unit spelling accepted after `NUMBER.`.
pub fn duration_unit_spellings() -> impl Iterator<Item = &'static str> {
    FIXED_DURATION_UNITS
        .iter()
        .flat_map(|unit| [unit.singular, unit.plural])
}

/// The whole-second span of a duration unit (singular and plural alike), or
/// `None` for a non-unit. Months and years are omitted: their length varies.
pub fn duration_unit_seconds(unit: &str) -> Option<i64> {
    FIXED_DURATION_UNITS
        .iter()
        .find(|facts| unit == facts.singular || unit == facts.plural)
        .map(|facts| facts.seconds)
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
    FIXED_DURATION_UNITS
        .iter()
        .find(|facts| unit == facts.singular || unit == facts.plural)
        .map(|facts| (facts.singular, facts.plural))
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
