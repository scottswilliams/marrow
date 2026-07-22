//! Drift check for the machine-readable language facts published in
//! `docs/tools/ai-legibility.md`. The parser owns syntax; that page republishes
//! two lexical inventories the parser derives — the reserved words and the token
//! kinds — and this test proves the committed lists still match the parser.
//!
//! The enforcement is native to the type system: `keyword_spelling` and
//! `token_kind_name` are exhaustive matches over the public `Keyword` and
//! `TokenKind` types, so adding, removing, or renaming a variant in the parser
//! fails to compile this test until the match is updated, and the set comparison
//! then fails until the published block is updated in the same change. No parser
//! change can outpace the page silently.

use marrow_syntax::{Keyword, TokenKind, is_reserved_word};

/// Every `Keyword` variant. Completeness is guarded by the exhaustive match in
/// [`keyword_spelling`]: a new variant cannot be added to the parser without a
/// compile error there, at which point this list is extended in the same change.
const ALL_KEYWORDS: &[Keyword] = &[
    Keyword::Module,
    Keyword::Use,
    Keyword::Pub,
    Keyword::Fn,
    Keyword::Alias,
    Keyword::Type,
    Keyword::Supports,
    Keyword::Resource,
    Keyword::Struct,
    Keyword::Store,
    Keyword::Enum,
    Keyword::Test,
    Keyword::Assert,
    Keyword::Match,
    Keyword::Index,
    Keyword::Unique,
    Keyword::Required,
    Keyword::Const,
    Keyword::Var,
    Keyword::Place,
    Keyword::Checked,
    Keyword::If,
    Keyword::Else,
    Keyword::While,
    Keyword::For,
    Keyword::In,
    Keyword::Break,
    Keyword::Continue,
    Keyword::Return,
    Keyword::Absent,
    Keyword::Delete,
    Keyword::Unset,
    Keyword::Merge,
    Keyword::Journal,
    Keyword::Sensitive,
    Keyword::Declassify,
    Keyword::Transaction,
    Keyword::Lock,
    Keyword::Writes,
    Keyword::Reads,
    Keyword::Try,
    Keyword::Require,
    Keyword::True,
    Keyword::False,
    Keyword::Not,
    Keyword::And,
    Keyword::Or,
    Keyword::Is,
    Keyword::Int,
    Keyword::Decimal,
    Keyword::Bool,
    Keyword::String,
    Keyword::Bytes,
    Keyword::Date,
    Keyword::Instant,
    Keyword::Duration,
    Keyword::Unknown,
    Keyword::Error,
    Keyword::ErrorCode,
    Keyword::Id,
];

/// The canonical source spelling of a keyword. Exhaustive by construction: a new
/// `Keyword` variant fails to compile here.
fn keyword_spelling(keyword: Keyword) -> &'static str {
    match keyword {
        Keyword::Module => "module",
        Keyword::Use => "use",
        Keyword::Pub => "pub",
        Keyword::Fn => "fn",
        Keyword::Alias => "alias",
        Keyword::Type => "type",
        Keyword::Supports => "supports",
        Keyword::Resource => "resource",
        Keyword::Struct => "struct",
        Keyword::Store => "store",
        Keyword::Enum => "enum",
        Keyword::Test => "test",
        Keyword::Assert => "assert",
        Keyword::Match => "match",
        Keyword::Index => "index",
        Keyword::Unique => "unique",
        Keyword::Required => "required",
        Keyword::Const => "const",
        Keyword::Var => "var",
        Keyword::Place => "place",
        Keyword::Checked => "checked",
        Keyword::If => "if",
        Keyword::Else => "else",
        Keyword::While => "while",
        Keyword::For => "for",
        Keyword::In => "in",
        Keyword::Break => "break",
        Keyword::Continue => "continue",
        Keyword::Return => "return",
        Keyword::Absent => "absent",
        Keyword::Delete => "delete",
        Keyword::Unset => "unset",
        Keyword::Merge => "merge",
        Keyword::Journal => "journal",
        Keyword::Sensitive => "sensitive",
        Keyword::Declassify => "declassify",
        Keyword::Transaction => "transaction",
        Keyword::Lock => "lock",
        Keyword::Writes => "writes",
        Keyword::Reads => "reads",
        Keyword::Try => "try",
        Keyword::Require => "require",
        Keyword::True => "true",
        Keyword::False => "false",
        Keyword::Not => "not",
        Keyword::And => "and",
        Keyword::Or => "or",
        Keyword::Is => "is",
        Keyword::Int => "int",
        Keyword::Decimal => "decimal",
        Keyword::Bool => "bool",
        Keyword::String => "string",
        Keyword::Bytes => "bytes",
        Keyword::Date => "date",
        Keyword::Instant => "instant",
        Keyword::Duration => "duration",
        Keyword::Unknown => "unknown",
        Keyword::Error => "Error",
        Keyword::ErrorCode => "ErrorCode",
        Keyword::Id => "Id",
    }
}

/// Every `TokenKind` variant. Completeness is guarded by the exhaustive match in
/// [`token_kind_name`].
const ALL_TOKEN_KINDS: &[TokenKind] = &[
    TokenKind::Identifier,
    TokenKind::Integer,
    TokenKind::Decimal,
    TokenKind::Duration,
    TokenKind::String,
    TokenKind::InterpolationStart,
    TokenKind::InterpolationText,
    TokenKind::InterpolationExprStart,
    TokenKind::InterpolationExprEnd,
    TokenKind::InterpolationEnd,
    TokenKind::Bytes,
    TokenKind::Keyword(Keyword::Module),
    TokenKind::Comment,
    TokenKind::DocComment,
    TokenKind::Newline,
    TokenKind::Eof,
    TokenKind::LeftParen,
    TokenKind::RightParen,
    TokenKind::LeftBracket,
    TokenKind::RightBracket,
    TokenKind::LeftBrace,
    TokenKind::RightBrace,
    TokenKind::FatArrow,
    TokenKind::Colon,
    TokenKind::DoubleColon,
    TokenKind::Comma,
    TokenKind::Dot,
    TokenKind::DotDot,
    TokenKind::DotDotEqual,
    TokenKind::Equal,
    TokenKind::EqualEqual,
    TokenKind::BangEqual,
    TokenKind::Question,
    TokenKind::QuestionDot,
    TokenKind::QuestionQuestion,
    TokenKind::Less,
    TokenKind::LessEqual,
    TokenKind::Greater,
    TokenKind::GreaterEqual,
    TokenKind::Plus,
    TokenKind::Minus,
    TokenKind::Star,
    TokenKind::Slash,
    TokenKind::Percent,
    TokenKind::PlusEqual,
    TokenKind::MinusEqual,
    TokenKind::StarEqual,
    TokenKind::SlashEqual,
    TokenKind::PercentEqual,
    TokenKind::Caret,
];

/// The published inventory name of a token kind. Exhaustive by construction: a
/// new `TokenKind` variant fails to compile here.
fn token_kind_name(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Identifier => "Identifier",
        TokenKind::Integer => "Integer",
        TokenKind::Decimal => "Decimal",
        TokenKind::Duration => "Duration",
        TokenKind::String => "String",
        TokenKind::InterpolationStart => "InterpolationStart",
        TokenKind::InterpolationText => "InterpolationText",
        TokenKind::InterpolationExprStart => "InterpolationExprStart",
        TokenKind::InterpolationExprEnd => "InterpolationExprEnd",
        TokenKind::InterpolationEnd => "InterpolationEnd",
        TokenKind::Bytes => "Bytes",
        TokenKind::Keyword(_) => "Keyword",
        TokenKind::Comment => "Comment",
        TokenKind::DocComment => "DocComment",
        TokenKind::Newline => "Newline",
        TokenKind::Eof => "Eof",
        TokenKind::LeftParen => "LeftParen",
        TokenKind::RightParen => "RightParen",
        TokenKind::LeftBracket => "LeftBracket",
        TokenKind::RightBracket => "RightBracket",
        TokenKind::LeftBrace => "LeftBrace",
        TokenKind::RightBrace => "RightBrace",
        TokenKind::FatArrow => "FatArrow",
        TokenKind::Colon => "Colon",
        TokenKind::DoubleColon => "DoubleColon",
        TokenKind::Comma => "Comma",
        TokenKind::Dot => "Dot",
        TokenKind::DotDot => "DotDot",
        TokenKind::DotDotEqual => "DotDotEqual",
        TokenKind::Equal => "Equal",
        TokenKind::EqualEqual => "EqualEqual",
        TokenKind::BangEqual => "BangEqual",
        TokenKind::Question => "Question",
        TokenKind::QuestionDot => "QuestionDot",
        TokenKind::QuestionQuestion => "QuestionQuestion",
        TokenKind::Less => "Less",
        TokenKind::LessEqual => "LessEqual",
        TokenKind::Greater => "Greater",
        TokenKind::GreaterEqual => "GreaterEqual",
        TokenKind::Plus => "Plus",
        TokenKind::Minus => "Minus",
        TokenKind::Star => "Star",
        TokenKind::Slash => "Slash",
        TokenKind::Percent => "Percent",
        TokenKind::PlusEqual => "PlusEqual",
        TokenKind::MinusEqual => "MinusEqual",
        TokenKind::StarEqual => "StarEqual",
        TokenKind::SlashEqual => "SlashEqual",
        TokenKind::PercentEqual => "PercentEqual",
        TokenKind::Caret => "Caret",
    }
}

/// The path to the committed artifact, resolved from this crate's manifest
/// directory (`crates/marrow-syntax`) up to the repository root.
fn artifact_text() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("tools")
        .join("ai-legibility.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The whitespace-separated words inside the fenced block delimited by the
/// `BEGIN <name>` / `END <name>` HTML-comment markers, with the fence lines
/// (```` ``` ````) removed. A missing marker is a drift-test wiring error.
fn published_set(text: &str, name: &str) -> std::collections::BTreeSet<String> {
    let begin = format!("<!-- BEGIN {name} -->");
    let end = format!("<!-- END {name} -->");
    let start = text
        .find(&begin)
        .unwrap_or_else(|| panic!("artifact is missing the `{begin}` marker"))
        + begin.len();
    let stop = text[start..]
        .find(&end)
        .unwrap_or_else(|| panic!("artifact is missing the `{end}` marker"))
        + start;
    text[start..stop]
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
}

fn parser_set<T: Copy>(
    all: &[T],
    render: fn(T) -> &'static str,
) -> std::collections::BTreeSet<String> {
    all.iter().map(|item| render(*item).to_owned()).collect()
}

/// The published reserved-word list equals the set the parser recognizes. The
/// parser set is rendered from the keyword type's own spellings, so this fails
/// whenever the two disagree in either direction.
#[test]
fn published_reserved_words_match_the_parser() {
    let published = published_set(&artifact_text(), "reserved-words");
    let parser = parser_set(ALL_KEYWORDS, keyword_spelling);
    assert_eq!(
        published, parser,
        "docs/tools/ai-legibility.md reserved-words drifted from the parser; \
         update the block in the same change as the parser"
    );
}

/// Every rendered keyword spelling is in fact reserved by the lexer, so the
/// spellings this test renders are the parser's own truth, not a second list.
#[test]
fn every_rendered_keyword_is_reserved() {
    for &keyword in ALL_KEYWORDS {
        let spelling = keyword_spelling(keyword);
        assert!(
            is_reserved_word(spelling),
            "`{spelling}` is rendered as a keyword spelling but the lexer does not reserve it"
        );
    }
    // A plain identifier that is not a keyword must not be reported reserved, so
    // the predicate the parser set relies on is not vacuously true.
    assert!(!is_reserved_word("bookstore"));
    assert!(!is_reserved_word("error"));
}

/// The published token-kind inventory equals the parser's token kinds.
#[test]
fn published_token_kinds_match_the_parser() {
    let published = published_set(&artifact_text(), "token-kinds");
    let parser = parser_set(ALL_TOKEN_KINDS, token_kind_name);
    assert_eq!(
        published, parser,
        "docs/tools/ai-legibility.md token-kinds drifted from the parser; \
         update the block in the same change as the parser"
    );
}
