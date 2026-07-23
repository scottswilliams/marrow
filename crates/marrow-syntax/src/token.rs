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

macro_rules! count_variants {
    ($($variant:ident),+ $(,)?) => {
        <[()]>::len(&[$(count_variants!(@one $variant)),+])
    };
    (@one $variant:ident) => {
        ()
    };
}

macro_rules! define_keywords {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident => {
                spelling: $spelling:literal,
                class: $class:ident $(,)?
            }
        ),+ $(,)?
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Keyword {
            $(
                $(#[$meta])*
                $variant,
            )+
        }

        impl Keyword {
            /// Every reserved word, in declaration order.
            pub const ALL: [Self; count_variants!($($variant),+)] = [
                $(Self::$variant),+
            ];

            /// The exact source spelling reserved by the lexer.
            pub const fn spelling(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling),+
                }
            }

            /// The lexical role established by this reserved spelling.
            pub const fn lexical_class(self) -> LexicalClass {
                match self {
                    $(Self::$variant => LexicalClass::$class),+
                }
            }
        }

        pub(crate) fn keyword(text: &str) -> Option<Keyword> {
            match text {
                $($spelling => Some(Keyword::$variant),)+
                _ => None,
            }
        }
    };
}

define_keywords! {
    Module => { spelling: "module", class: Declaration },
    Use => { spelling: "use", class: Declaration },
    Pub => { spelling: "pub", class: Modifier },
    Fn => { spelling: "fn", class: Declaration },
    Alias => { spelling: "alias", class: Declaration },
    Type => { spelling: "type", class: Declaration },
    Supports => { spelling: "supports", class: Modifier },
    Resource => { spelling: "resource", class: Declaration },
    Struct => { spelling: "struct", class: Declaration },
    Store => { spelling: "store", class: Declaration },
    Enum => { spelling: "enum", class: Declaration },
    Test => { spelling: "test", class: Declaration },
    Assert => { spelling: "assert", class: ControlFlow },
    Match => { spelling: "match", class: ControlFlow },
    Index => { spelling: "index", class: Declaration },
    Unique => { spelling: "unique", class: Modifier },
    Required => { spelling: "required", class: Modifier },
    Const => { spelling: "const", class: Declaration },
    Var => { spelling: "var", class: Declaration },
    Place => { spelling: "place", class: Declaration },
    Checked => { spelling: "checked", class: Modifier },
    If => { spelling: "if", class: ControlFlow },
    Else => { spelling: "else", class: ControlFlow },
    While => { spelling: "while", class: ControlFlow },
    For => { spelling: "for", class: ControlFlow },
    In => { spelling: "in", class: ControlFlow },
    Break => { spelling: "break", class: ControlFlow },
    Continue => { spelling: "continue", class: ControlFlow },
    Return => { spelling: "return", class: ControlFlow },
    Absent => { spelling: "absent", class: BuiltinValue },
    Delete => { spelling: "delete", class: ControlFlow },
    Unset => { spelling: "unset", class: ControlFlow },
    Merge => { spelling: "merge", class: ControlFlow },
    Journal => { spelling: "journal", class: Modifier },
    Sensitive => { spelling: "sensitive", class: Modifier },
    Declassify => { spelling: "declassify", class: Effect },
    Transaction => { spelling: "transaction", class: ControlFlow },
    Lock => { spelling: "lock", class: ControlFlow },
    // Held for the future effect-signature clause. The spelling is reserved,
    // while the clause itself is not yet grammar.
    Writes => { spelling: "writes", class: Effect },
    Reads => { spelling: "reads", class: Effect },
    Try => { spelling: "try", class: ControlFlow },
    Require => { spelling: "require", class: ControlFlow },
    True => { spelling: "true", class: BuiltinValue },
    False => { spelling: "false", class: BuiltinValue },
    Not => { spelling: "not", class: WordOperator },
    And => { spelling: "and", class: WordOperator },
    Or => { spelling: "or", class: WordOperator },
    Is => { spelling: "is", class: WordOperator },
    Int => { spelling: "int", class: BuiltinType },
    Decimal => { spelling: "decimal", class: BuiltinType },
    Bool => { spelling: "bool", class: BuiltinType },
    String => { spelling: "string", class: BuiltinType },
    Bytes => { spelling: "bytes", class: BuiltinType },
    Date => { spelling: "date", class: BuiltinType },
    Instant => { spelling: "instant", class: BuiltinType },
    Duration => { spelling: "duration", class: BuiltinType },
    Unknown => { spelling: "unknown", class: BuiltinType },
    Error => { spelling: "Error", class: BuiltinType },
    ErrorCode => { spelling: "ErrorCode", class: BuiltinType },
    Id => { spelling: "Id", class: BuiltinType },
}

macro_rules! token_kind_name_pattern {
    ($variant:ident) => {
        Self::$variant
    };
    ($variant:ident, $payload:ty) => {
        Self::$variant(_)
    };
}

macro_rules! token_kind_inventory {
    ($variant:ident) => {
        Self::$variant
    };
    ($variant:ident, $inventory:expr) => {
        $inventory
    };
}

macro_rules! token_kind_value_pattern {
    ($variant:ident) => {
        Self::$variant
    };
    ($variant:ident, $binding:ident) => {
        Self::$variant($binding)
    };
}

macro_rules! define_token_kinds {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident $(($payload:ty, $binding:ident, $inventory:expr))? => {
                class: $class:expr,
                fixed: $fixed:expr,
            }
        ),+ $(,)?
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum TokenKind {
            $(
                $(#[$meta])*
                $variant $(($payload))?,
            )+
        }

        impl TokenKind {
            /// One representative of every token-kind variant, generated from
            /// the same construction as the enum and its lexical facts.
            pub const INVENTORY: [Self; count_variants!($($variant),+)] = [
                $(token_kind_inventory!($variant $(, $inventory)?)),+
            ];

            /// The stable variant name used by lexical inventory projections.
            pub const fn inventory_name(self) -> &'static str {
                match self {
                    $(token_kind_name_pattern!($variant $(, $payload)?) => stringify!($variant)),+
                }
            }

            /// The class known from tokenization alone.
            pub const fn lexical_class(self) -> LexicalClass {
                match self {
                    $(token_kind_value_pattern!($variant $(, $binding)?) => $class),+
                }
            }

            /// Exact fixed spelling, when the token is not content-bearing.
            pub const fn fixed_spelling(self) -> Option<&'static str> {
                match self {
                    $(token_kind_value_pattern!($variant $(, $binding)?) => $fixed),+
                }
            }
        }
    };
}

define_token_kinds! {
    Identifier => {
        class: LexicalClass::Unscoped,
        fixed: None,
    },
    Integer => {
        class: LexicalClass::IntegerLiteral,
        fixed: None,
    },
    Decimal => {
        class: LexicalClass::DecimalLiteral,
        fixed: None,
    },
    /// A duration literal NUMBER.UNIT; the token text is the whole literal.
    Duration => {
        class: LexicalClass::DurationLiteral,
        fixed: None,
    },
    String => {
        class: LexicalClass::StringLiteral,
        fixed: None,
    },
    InterpolationStart => {
        class: LexicalClass::InterpolationString,
        fixed: Some("$\""),
    },
    InterpolationText => {
        class: LexicalClass::InterpolationString,
        fixed: None,
    },
    InterpolationExprStart => {
        class: LexicalClass::InterpolationDelimiter,
        fixed: Some("{"),
    },
    InterpolationExprEnd => {
        class: LexicalClass::InterpolationDelimiter,
        fixed: Some("}"),
    },
    InterpolationEnd => {
        class: LexicalClass::InterpolationString,
        fixed: Some("\""),
    },
    Bytes => {
        class: LexicalClass::BytesLiteral,
        fixed: None,
    },
    Keyword(Keyword, keyword, Self::Keyword(Keyword::Module)) => {
        class: keyword.lexical_class(),
        fixed: Some(keyword.spelling()),
    },
    Comment => {
        class: LexicalClass::Comment,
        fixed: None,
    },
    DocComment => {
        class: LexicalClass::DocumentationComment,
        fixed: None,
    },
    Newline => {
        class: LexicalClass::Unscoped,
        fixed: None,
    },
    Eof => {
        class: LexicalClass::Unscoped,
        fixed: None,
    },
    LeftParen => {
        class: LexicalClass::Delimiter,
        fixed: Some("("),
    },
    RightParen => {
        class: LexicalClass::Delimiter,
        fixed: Some(")"),
    },
    LeftBracket => {
        class: LexicalClass::Delimiter,
        fixed: Some("["),
    },
    RightBracket => {
        class: LexicalClass::Delimiter,
        fixed: Some("]"),
    },
    /// Block delimiters. Interpolation holes and doubled-brace escapes are
    /// recognized inside interpolation before these tokens are produced.
    LeftBrace => {
        class: LexicalClass::Delimiter,
        fixed: Some("{"),
    },
    RightBrace => {
        class: LexicalClass::Delimiter,
        fixed: Some("}"),
    },
    /// The match-arm arrow.
    FatArrow => {
        class: LexicalClass::Operator,
        fixed: Some("=>"),
    },
    Colon => {
        class: LexicalClass::Punctuation,
        fixed: Some(":"),
    },
    DoubleColon => {
        class: LexicalClass::PathSeparator,
        fixed: Some("::"),
    },
    Comma => {
        class: LexicalClass::Punctuation,
        fixed: Some(","),
    },
    Dot => {
        class: LexicalClass::Punctuation,
        fixed: Some("."),
    },
    DotDot => {
        class: LexicalClass::Operator,
        fixed: Some(".."),
    },
    DotDotEqual => {
        class: LexicalClass::Operator,
        fixed: Some("..="),
    },
    Equal => {
        class: LexicalClass::Operator,
        fixed: Some("="),
    },
    EqualEqual => {
        class: LexicalClass::Operator,
        fixed: Some("=="),
    },
    BangEqual => {
        class: LexicalClass::Operator,
        fixed: Some("!="),
    },
    Question => {
        class: LexicalClass::Punctuation,
        fixed: Some("?"),
    },
    QuestionDot => {
        class: LexicalClass::Operator,
        fixed: Some("?."),
    },
    QuestionQuestion => {
        class: LexicalClass::Operator,
        fixed: Some("??"),
    },
    Less => {
        class: LexicalClass::Operator,
        fixed: Some("<"),
    },
    LessEqual => {
        class: LexicalClass::Operator,
        fixed: Some("<="),
    },
    Greater => {
        class: LexicalClass::Operator,
        fixed: Some(">"),
    },
    GreaterEqual => {
        class: LexicalClass::Operator,
        fixed: Some(">="),
    },
    Plus => {
        class: LexicalClass::Operator,
        fixed: Some("+"),
    },
    Minus => {
        class: LexicalClass::Operator,
        fixed: Some("-"),
    },
    Star => {
        class: LexicalClass::Operator,
        fixed: Some("*"),
    },
    Slash => {
        class: LexicalClass::Operator,
        fixed: Some("/"),
    },
    Percent => {
        class: LexicalClass::Operator,
        fixed: Some("%"),
    },
    PlusEqual => {
        class: LexicalClass::Operator,
        fixed: Some("+="),
    },
    MinusEqual => {
        class: LexicalClass::Operator,
        fixed: Some("-="),
    },
    StarEqual => {
        class: LexicalClass::Operator,
        fixed: Some("*="),
    },
    SlashEqual => {
        class: LexicalClass::Operator,
        fixed: Some("/="),
    },
    PercentEqual => {
        class: LexicalClass::Operator,
        fixed: Some("%="),
    },
    Caret => {
        class: LexicalClass::DurableRootSigil,
        fixed: Some("^"),
    },
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
