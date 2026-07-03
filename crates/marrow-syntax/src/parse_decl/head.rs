//! Token-level parsers for declaration heads: the resource, store, and enum
//! headers, the index declaration, the parenthesized key-parameter list, and the
//! resource field-or-group member head.

use super::params::match_paren;
use super::tokens::{line_span_or, parse_type, split_top_level_commas, strip_comment_tokens};
use super::{MemberHead, ParseError, ParseResult};
use crate::ast::{IndexDecl, KeyParam, SavedRoot};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};

/// Parse an enum header line: `[pub] enum Name`. Returns the visibility flag and
/// the enum name. `pub` is recorded for consistency with `pub fn`; the body of
/// the enum is parsed separately from its indented block.
pub(super) fn parse_enum_head(
    source: &str,
    tokens: &[Token],
) -> ParseResult<(bool, String, SourceSpan)> {
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
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::EnumHeader),
            "expected enum declaration",
        ));
    }
    let rest = &rest[1..];
    let (name, name_span) = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::EnumName),
                "expected enum name",
            ));
        }
    };
    if rest.len() > 1 {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::EnumHeader),
            "an enum header is just `enum Name`",
        ));
    }
    Ok((public, name, name_span))
}

/// The name and category flag of an enum member from its header tokens: a bare
/// identifier, optionally led by a contextual `category` word that marks it a
/// grouping node. `category` is recognized positionally as the header lead, so it
/// never collides with `category` used as an ordinary identifier elsewhere.
/// Anything else — a type annotation, key parameters, or extra tokens — is the
/// resource-member surface, which an enum member does not have.
pub(super) fn enum_member_name(
    source: &str,
    tokens: &[Token],
) -> ParseResult<(String, bool, SourceSpan)> {
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
            Ok((token.text(source).to_string(), category, token.span))
        }
        [_] => Err(ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "expected an enum member name",
        )),
        _ => Err(ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "an enum member is a bare name; it takes no type or parameters",
        )),
    }
}

/// The `::`-separated identifier segments of a match-arm header, or `None` when
/// the header is not a member path (`identifier ("::" identifier)*`). The
/// scrutinee supplies the enum, so an arm header carries no enum prefix — it is a
/// relative path the checker walks against the scrutinee enum's member tree.
pub(super) fn arm_member_path(
    source: &str,
    tokens: &[Token],
) -> Option<(Vec<String>, Vec<SourceSpan>)> {
    if tokens.is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    let mut spans = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        // Even positions are identifiers, odd positions the `::` separators.
        if index % 2 == 0 {
            if token.kind != TokenKind::Identifier {
                return None;
            }
            segments.push(token.text(source).to_string());
            spans.push(token.span);
        } else if token.kind != TokenKind::DoubleColon {
            return None;
        }
    }
    // A trailing `::` (an even count of tokens) leaves a separator with no segment.
    if tokens.len().is_multiple_of(2) {
        return None;
    }
    Some((segments, spans))
}

/// Parse a resource header's tokens after the `resource` keyword: `Name`.
pub(super) fn parse_resource_head(
    source: &str,
    tokens: &[Token],
) -> ParseResult<(String, SourceSpan)> {
    let (name, name_span) = match tokens.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceName),
                "expected resource name",
            ));
        }
    };
    let rest = &tokens[1..];
    if rest.is_empty() {
        return Ok((name, name_span));
    }
    Err(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceHeader),
        "a resource header is just `resource Name`",
    ))
}

/// Parse a store header's tokens after the `store` keyword:
/// `^root [(key: type, ...)]: Resource`.
pub(super) fn parse_store_head(source: &str, tokens: &[Token]) -> ParseResult<(SavedRoot, String)> {
    if !matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Caret)
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::StoreRoot),
            "expected saved root beginning with `^`",
        ));
    }
    let caret_span = tokens[0].span;
    let (root, root_span) = match tokens.get(1) {
        Some(token) if token.kind == TokenKind::Identifier => (
            token.text(source).to_string(),
            join_spans(caret_span, token.span),
        ),
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::SavedRootName),
                "expected saved root name",
            ));
        }
    };
    let rest = &tokens[2..];
    let colon = rest
        .iter()
        .scan(0usize, |depth, token| {
            let top_level_colon = *depth == 0 && token.kind == TokenKind::Colon;
            match token.kind {
                TokenKind::LeftParen => *depth += 1,
                TokenKind::RightParen => *depth = depth.saturating_sub(1),
                _ => {}
            }
            Some(top_level_colon)
        })
        .position(|top_level_colon| top_level_colon)
        .ok_or(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::StoreResourceName),
            "expected `: Resource` after store root",
        ))?;
    let keys = if colon == 0 {
        Vec::new()
    } else {
        parse_paren_key_params(source, &rest[..colon])?
    };
    let resource_tokens = &rest[colon + 1..];
    let resource = match resource_tokens {
        [token] if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::StoreResourceName),
                "expected resource name after store root",
            ));
        }
    };
    Ok((
        SavedRoot {
            root,
            keys,
            span: root_span,
        },
        resource,
    ))
}

/// Parse a parenthesized `(name: type, ...)` key parameter list spanning the
/// whole token slice. Requires the parentheses to be the only content.
fn parse_paren_key_params(source: &str, tokens: &[Token]) -> ParseResult<Vec<KeyParam>> {
    if !matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::KeyParameterList),
            "expected key parameter list",
        ));
    }
    let close = match_paren(tokens).ok_or(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::KeyParameterList),
        "expected key parameter list",
    ))?;
    if close + 1 != tokens.len() {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::KeyParameterList),
            "unexpected text after key parameter list",
        ));
    }
    parse_key_params_tokens(source, &tokens[1..close])
}

/// Parse a comma-separated `name: type` key list. Requires at least one key.
pub(super) fn parse_key_params_tokens(source: &str, inner: &[Token]) -> ParseResult<Vec<KeyParam>> {
    let inner = strip_comment_tokens(inner);
    if inner.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::EmptyKeyParameters,
            "expected at least one key parameter",
        ));
    }
    let mut params = Vec::new();
    for part in split_top_level_commas(&inner) {
        let name = match part.first() {
            Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
            _ => {
                return Err(ParseError::new(
                    ParseDiagnosticReason::Expected(ExpectedSyntax::KeyName),
                    "expected key name",
                ));
            }
        };
        if part.get(1).map(|token| token.kind) != Some(TokenKind::Colon) || part.len() < 3 {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::KeyType),
                "expected key type annotation",
            ));
        }
        let ty = parse_type(
            source,
            &part[2..],
            ExpectedSyntax::KeyType,
            "expected key type annotation",
        )?;
        params.push(KeyParam { name, ty });
    }
    Ok(params)
}

/// Parse an `index name(field, ...) [unique]` declaration from the tokens after
/// the `index` keyword. The span is filled in by the caller.
pub(super) fn parse_index_tokens(source: &str, tokens: &[Token]) -> ParseResult<IndexDecl> {
    let (name, name_span) = match tokens.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::IndexName),
                "expected index name",
            ));
        }
    };
    let rest = &tokens[1..];
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::IndexArgumentList),
            "expected index argument list",
        ));
    }
    let close = match_paren(rest).ok_or(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::IndexArgumentList),
        "expected index argument list",
    ))?;
    let inner = strip_comment_tokens(&rest[1..close]);
    if inner.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::EmptyIndexArguments,
            "expected at least one index argument",
        ));
    }
    let mut args = Vec::new();
    let mut arg_spans = Vec::new();
    for part in split_top_level_commas(&inner) {
        let arg = field_path_text(source, part).ok_or(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::IndexFieldPath),
            "expected index field path",
        ))?;
        args.push(arg);
        arg_spans.push(line_span_or(part, part[0].span));
    }
    let tail = &rest[close + 1..];
    let unique = match tail {
        [] => false,
        [token] if token.kind == TokenKind::Keyword(Keyword::Unique) => true,
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::IndexTail),
                "expected `unique` or end of index declaration",
            ));
        }
    };
    Ok(IndexDecl {
        docs: Vec::new(),
        name,
        name_span,
        args,
        arg_spans,
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
/// or group.
pub(super) fn parse_field_or_group_tokens(
    source: &str,
    tokens: &[Token],
) -> ParseResult<MemberHead> {
    let (required, rest) = if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Required))
    ) {
        (true, &tokens[1..])
    } else {
        (false, tokens)
    };
    let (name, name_span) = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        // A line that begins with a keyed-layer clause spelling such as `unique`
        // — a keyword that does not go on to name a field (`:`) or keyed field
        // (`(`) — is a malformed member, not a missing name. Report the
        // member-shape rule naming what is allowed here, the same diagnostic a
        // non-keyword junk word reaches. A keyword followed by `:`/`(` is instead
        // a reserved word used as a member name, which keeps the member-name rule.
        Some(token)
            if matches!(token.kind, TokenKind::Keyword(_))
                && !matches!(
                    rest.get(1).map(|next| next.kind),
                    Some(TokenKind::Colon | TokenKind::LeftParen)
                ) =>
        {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceMemberSyntax),
                "expected resource field, keyed field, group, or index",
            ));
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceMemberName),
                "expected resource member name",
            ));
        }
    };
    let mut rest = &rest[1..];
    let keys = if matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        let close = match_paren(rest).ok_or(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::KeyParameterList),
            "expected closing `)` in keyed resource member",
        ))?;
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
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FieldType),
                "expected field type after `:`",
            ));
        }
        let ty = parse_type(
            source,
            ty_tokens,
            ExpectedSyntax::FieldType,
            "expected field type after `:`",
        )?;
        return Ok(MemberHead::Field {
            required,
            name,
            name_span,
            keys,
            ty,
        });
    }
    if required {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::FieldType),
            "required resource members must declare a field type",
        ));
    }
    if rest.is_empty() {
        return Ok(MemberHead::Group {
            name,
            name_span,
            keys,
        });
    }
    Err(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceMemberSyntax),
        "expected resource field, keyed field, group, or index",
    ))
}
