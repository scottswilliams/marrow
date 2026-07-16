//! Token-level parsers for declaration heads: the resource, store, and enum
//! headers, the index declaration, the parenthesized key-parameter list, and the
//! resource field-or-group member head.

use super::params::{match_bracket, match_paren, parse_type_params_tokens};
use super::tokens::{line_span_or, parse_type, split_top_level_commas, strip_comment_tokens};
use super::{MemberHead, ParseError, ParseResult};
use crate::ast::{EnumPayloadField, IndexDecl, KeyParam, SavedRoot, TypeParamDecl};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};

/// Parse an enum header line: `[pub] enum Name`. Returns the visibility flag and
/// the enum name. `pub` is recorded for consistency with `pub fn`; the body of
/// the enum is parsed separately from its indented block.
pub(super) fn parse_enum_head(
    source: &str,
    tokens: &[Token],
) -> ParseResult<(bool, String, SourceSpan, Vec<TypeParamDecl>)> {
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
    let (type_params, rest) = parse_optional_type_params(source, &rest[1..])?;
    if !rest.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::EnumHeader),
            "an enum header is `enum Name` or `enum Name[T, ...]`",
        ));
    }
    Ok((public, name, name_span, type_params))
}

/// Parse a struct header's tokens after the `struct` keyword: `Name` or
/// `Name[T, ...]`. The generic type-parameter list uses the same bracket
/// convention as a type application (`List[T]`); a leading `[` after the name
/// introduces the parameters. A struct field's `name: Type` body is parsed
/// separately from the indented block, reusing the resource-member machinery.
pub(super) fn parse_struct_head(
    source: &str,
    tokens: &[Token],
) -> ParseResult<(String, SourceSpan, Vec<TypeParamDecl>)> {
    let (name, name_span) = match tokens.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceName),
                "expected struct name",
            ));
        }
    };
    let (type_params, rest) = parse_optional_type_params(source, &tokens[1..])?;
    if !rest.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceHeader),
            "a struct header is `struct Name` or `struct Name[T, ...]`",
        ));
    }
    Ok((name, name_span, type_params))
}

/// Parse an optional generic type-parameter list at the start of `tokens`: a
/// leading `[T, ...]` yields the parameters and the tokens after the `]`; anything
/// else yields an empty list and the unconsumed tokens.
fn parse_optional_type_params<'t>(
    source: &str,
    tokens: &'t [Token],
) -> ParseResult<(Vec<TypeParamDecl>, &'t [Token])> {
    if !matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::LeftBracket)
    ) {
        return Ok((Vec::new(), tokens));
    }
    let close = match_bracket(tokens).ok_or(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceHeader),
        "expected `]` to close the type-parameter list",
    ))?;
    let params = parse_type_params_tokens(source, &tokens[1..close])?;
    Ok((params, &tokens[close + 1..]))
}

/// A parsed enum member header: the member name and category flag plus its dense
/// payload fields (empty for a bare member).
pub(super) struct EnumMemberHead {
    pub name: String,
    pub category: bool,
    pub name_span: SourceSpan,
    pub payload: Vec<EnumPayloadField>,
}

/// The name, category flag, and payload of an enum member from its header tokens:
/// a bare identifier optionally led by a contextual `category` word, optionally
/// followed by a parenthesized `name: Type` payload list. `category` is recognized
/// positionally as the header lead, so it never collides with `category` used as
/// an ordinary identifier elsewhere.
pub(super) fn enum_member_name(source: &str, tokens: &[Token]) -> ParseResult<EnumMemberHead> {
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
    let (name, name_span) = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::EnumMemberMustBeBareName,
                "expected an enum member name",
            ));
        }
    };
    let rest = &rest[1..];
    // A bare member ends at its name; a payload member follows the name with a
    // parenthesized `name: Type` list and nothing else.
    if rest.is_empty() {
        return Ok(EnumMemberHead {
            name,
            category,
            name_span,
            payload: Vec::new(),
        });
    }
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "an enum member is a bare name or a payload member `name(field: Type, ...)`",
        ));
    }
    let close = match_paren(rest).ok_or(ParseError::new(
        ParseDiagnosticReason::EnumMemberMustBeBareName,
        "expected a closing `)` in the enum member payload",
    ))?;
    if rest.len() != close + 1 {
        return Err(ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "an enum member payload takes nothing after its `)`",
        ));
    }
    let payload = parse_enum_payload_tokens(source, &rest[1..close])?;
    Ok(EnumMemberHead {
        name,
        category,
        name_span,
        payload,
    })
}

/// Parse the inside of an enum member payload list: a comma-separated run of
/// `name: Type` fields. An empty payload (`circle()`) is rejected — a payload
/// member declares at least one field.
fn parse_enum_payload_tokens(source: &str, inner: &[Token]) -> ParseResult<Vec<EnumPayloadField>> {
    if inner.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "an enum member payload declares at least one `name: Type` field",
        ));
    }
    let mut fields = Vec::new();
    for group in split_top_level_commas(inner) {
        if group.is_empty() {
            return Err(ParseError::new(
                ParseDiagnosticReason::EnumMemberMustBeBareName,
                "an enum member payload field is `name: Type`",
            ));
        }
        let (name, name_span) = match group.first() {
            Some(token) if token.kind == TokenKind::Identifier => {
                (token.text(source).to_string(), token.span)
            }
            _ => {
                return Err(ParseError::new(
                    ParseDiagnosticReason::EnumMemberMustBeBareName,
                    "an enum member payload field is `name: Type`",
                ));
            }
        };
        if !matches!(group.get(1).map(|token| token.kind), Some(TokenKind::Colon)) {
            return Err(ParseError::new(
                ParseDiagnosticReason::EnumMemberMustBeBareName,
                "an enum member payload field is `name: Type`",
            ));
        }
        let ty_tokens = &group[2..];
        if ty_tokens.is_empty() {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FieldType),
                "expected payload field type after `:`",
            ));
        }
        let ty = parse_type(
            source,
            ty_tokens,
            ExpectedSyntax::FieldType,
            "expected payload field type after `:`",
        )?;
        let span = join_spans(name_span, ty.span());
        fields.push(EnumPayloadField {
            name,
            name_span,
            ty,
            span,
        });
    }
    Ok(fields)
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

/// A parsed match-arm header: the member path relative to the scrutinee enum and
/// its positional payload bindings (empty for a bare arm).
pub(super) struct ArmPattern {
    pub path: Vec<String>,
    pub path_spans: Vec<SourceSpan>,
    pub bindings: Vec<(String, SourceSpan)>,
}

/// Parse a match-arm header: a member path optionally followed by a positional
/// binding list `(a, b, ...)`. Returns `None` when the header is not a member
/// path or the binding list is malformed, so the caller reports one arm error.
pub(super) fn arm_pattern(source: &str, tokens: &[Token]) -> Option<ArmPattern> {
    // Split off a trailing `(...)` binding list, if any. The path is everything
    // before the first top-level `(`.
    let paren = tokens
        .iter()
        .position(|token| token.kind == TokenKind::LeftParen);
    let (path_tokens, bindings) = match paren {
        None => (tokens, Vec::new()),
        Some(open) => {
            let close = match_paren(&tokens[open..])? + open;
            // Nothing may follow the binding list.
            if close + 1 != tokens.len() {
                return None;
            }
            let bindings = parse_arm_bindings(source, &tokens[open + 1..close])?;
            (&tokens[..open], bindings)
        }
    };
    let (path, path_spans) = arm_member_path(source, path_tokens)?;
    Some(ArmPattern {
        path,
        path_spans,
        bindings,
    })
}

/// Parse the inside of a match-arm binding list: a comma-separated run of bare
/// identifiers. An empty list (`circle()`) is rejected.
fn parse_arm_bindings(source: &str, inner: &[Token]) -> Option<Vec<(String, SourceSpan)>> {
    if inner.is_empty() {
        return None;
    }
    let mut bindings = Vec::new();
    for group in split_top_level_commas(inner) {
        match group {
            [token] if token.kind == TokenKind::Identifier => {
                bindings.push((token.text(source).to_string(), token.span));
            }
            _ => return None,
        }
    }
    Some(bindings)
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
