use marrow_syntax::{Keyword, SourceSpan, Token, TokenKind};

use super::{SourceSemanticTokenRole, SourceSemanticTokenStyle};

pub(super) fn syntax_style(kind: TokenKind) -> Option<SourceSemanticTokenStyle> {
    let role = match kind {
        TokenKind::Identifier => SourceSemanticTokenRole::Variable,
        TokenKind::Integer | TokenKind::Decimal | TokenKind::Duration => {
            SourceSemanticTokenRole::NumberLiteral
        }
        TokenKind::String
        | TokenKind::Bytes
        | TokenKind::InterpolationStart
        | TokenKind::InterpolationText
        | TokenKind::InterpolationEnd => SourceSemanticTokenRole::StringLiteral,
        TokenKind::Comment | TokenKind::DocComment => SourceSemanticTokenRole::Comment,
        TokenKind::Keyword(keyword) => match keyword {
            Keyword::True | Keyword::False => SourceSemanticTokenRole::BooleanLiteral,
            _ if is_operator_keyword(keyword) => SourceSemanticTokenRole::Operator,
            _ if is_type_keyword(keyword) => SourceSemanticTokenRole::TypeKeyword,
            _ => SourceSemanticTokenRole::Keyword,
        },
        TokenKind::DoubleColon => SourceSemanticTokenRole::Namespace,
        TokenKind::Colon
        | TokenKind::Dot
        | TokenKind::DotDot
        | TokenKind::DotDotEqual
        | TokenKind::Equal
        | TokenKind::EqualEqual
        | TokenKind::BangEqual
        | TokenKind::QuestionDot
        | TokenKind::QuestionQuestion
        | TokenKind::Less
        | TokenKind::LessEqual
        | TokenKind::Greater
        | TokenKind::GreaterEqual
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::At => SourceSemanticTokenRole::Operator,
        _ => return None,
    };
    Some(SourceSemanticTokenStyle::plain(role))
}

pub(super) fn is_path_segment_token(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Identifier | TokenKind::Keyword(_))
}

pub(super) fn token_in_span(token: &Token, span: SourceSpan) -> bool {
    token.span.start_byte >= span.start_byte && token.span.end_byte <= span.end_byte
}

fn is_operator_keyword(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::Not | Keyword::And | Keyword::Or | Keyword::Is
    )
}

fn is_type_keyword(keyword: Keyword) -> bool {
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
            | Keyword::Sequence
            | Keyword::Unknown
            | Keyword::Error
            | Keyword::ErrorCode
    )
}
