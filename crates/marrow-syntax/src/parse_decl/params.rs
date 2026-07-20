//! Token-level parsers for a function header and its parameter list, including
//! the multi-line parameter grouping that lets a line break separate parameters
//! the way a comma does.

use super::head::parse_key_params_tokens;
use super::tokens::{doc_comment_text, find_top_level_equal, parse_type, split_top_level_commas};
use super::{FunctionHead, ParseError, ParseResult};
use crate::ast::{KeyParam, ParamDecl, TypeConstraint, TypeParamDecl};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan, UnsupportedSyntax};
use crate::token::{Keyword, Token, TokenKind};

/// Parse a function header's tokens: `pub? fn name(params) (: return)?`.
pub(super) fn parse_function_head(source: &str, tokens: &[Token]) -> ParseResult<FunctionHead> {
    let (public, rest) = if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Pub))
    ) {
        (true, &tokens[1..])
    } else if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Identifier)
    ) {
        // `internal fn`/`private fn`: the visibility word lexes as an
        // identifier; reject it with a pointed message.
        let word = tokens[0].text(source);
        if word == "internal" {
            return Err(ParseError::new(
                ParseDiagnosticReason::InvalidVisibility,
                "function visibility is only `pub` or module-private; remove `internal`",
            ));
        }
        if word == "private" {
            return Err(ParseError::new(
                ParseDiagnosticReason::InvalidVisibility,
                "function visibility is only `pub` or module-private; remove `private`",
            ));
        }
        (false, tokens)
    } else {
        (false, tokens)
    };
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Fn))
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionHeader),
            "expected fn declaration",
        ));
    }
    let rest = &rest[1..];
    let (name, name_span) = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => {
            (token.text(source).to_string(), token.span)
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionName),
                "expected function name",
            ));
        }
    };
    let rest = &rest[1..];
    if matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftBracket)
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Unsupported(UnsupportedSyntax::UserDefinedGenerics),
            "generic type parameters are written `fn name<T>(...)`, not with `[...]`",
        ));
    }
    // Optional generic type-parameter list, `<T, U supports order>`, before the
    // value-parameter list. The same angle convention spells a type application
    // (`List<T>`), so a leading `<` after the name introduces the type parameters.
    let (type_params, rest) =
        if matches!(rest.first().map(|token| token.kind), Some(TokenKind::Less)) {
            let close = match_angle(rest).ok_or(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
                "expected `>` to close the type-parameter list",
            ))?;
            let params = parse_type_params_tokens(source, &rest[1..close])?;
            (params, &rest[close + 1..])
        } else {
            (Vec::new(), rest)
        };
    if !matches!(
        rest.first().map(|token| token.kind),
        Some(TokenKind::LeftParen)
    ) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
            "expected function parameter list",
        ));
    }
    let close = match_paren(rest).ok_or(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
        "expected function parameter list",
    ))?;
    let params = parse_params_tokens(source, &rest[1..close])?;
    let after = &rest[close + 1..];
    let return_type = if after.is_empty() {
        None
    } else {
        if after[0].kind != TokenKind::Colon {
            return Err(ParseError::at(
                after[0].span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionReturnType),
                "expected return type after `:`",
            ));
        }
        let ty_tokens = &after[1..];
        if ty_tokens.is_empty() {
            return Err(ParseError::at(
                after[0].span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionReturnType),
                "expected return type after `:`",
            ));
        }
        Some(parse_type(
            source,
            ty_tokens,
            ExpectedSyntax::FunctionReturnType,
            "expected return type after `:`",
        )?)
    };
    Ok(FunctionHead {
        public,
        name,
        name_span,
        type_params,
        params,
        return_type,
    })
}

/// Parse a generic type-parameter list's inner tokens (between `[` and `]`): a
/// comma-separated list of `Name` items, each optionally carrying one closed
/// constraint (`Name supports equality` / `Name supports order`). An empty list,
/// a missing name, a repeated `supports`, or an unknown capability is a pointed
/// parse error so a malformed header does not silently drop type parameters.
pub(super) fn parse_type_params_tokens(
    source: &str,
    inner: &[Token],
) -> ParseResult<Vec<TypeParamDecl>> {
    if inner.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
            "a type-parameter list names at least one type, `<T>`",
        ));
    }
    let mut params = Vec::new();
    for group in split_top_level_commas(inner) {
        let name_token = match group.first() {
            Some(token) if token.kind == TokenKind::Identifier => token,
            other => {
                return Err(other.map_or(
                    ParseError::new(
                        ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
                        "expected a type-parameter name",
                    ),
                    |token| {
                        ParseError::at(
                            token.span,
                            ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
                            "expected a type-parameter name",
                        )
                    },
                ));
            }
        };
        let name = name_token.text(source).to_string();
        let constraint = match &group[1..] {
            [] => None,
            [supports, capability] if supports.kind == TokenKind::Keyword(Keyword::Supports) => {
                Some(parse_type_constraint(source, capability)?)
            }
            [supports, ..] if supports.kind == TokenKind::Keyword(Keyword::Supports) => {
                return Err(ParseError::at(
                    supports.span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
                    "a type-parameter constraint is `supports equality` or `supports order`",
                ));
            }
            [extra, ..] => {
                return Err(ParseError::at(
                    extra.span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
                    "a type parameter is a name optionally followed by `supports equality` or \
                     `supports order`",
                ));
            }
        };
        let span = group.last().map_or(name_token.span, |last| SourceSpan {
            start_byte: name_token.span.start_byte,
            end_byte: last.span.end_byte,
            line: name_token.span.line,
            column: name_token.span.column,
        });
        params.push(TypeParamDecl {
            name,
            name_span: name_token.span,
            constraint,
            span,
        });
    }
    Ok(params)
}

/// Parse the capability word after `supports` in a type-parameter constraint. The
/// set is closed: only `equality` and `order` are admitted.
fn parse_type_constraint(source: &str, token: &Token) -> ParseResult<TypeConstraint> {
    if token.kind != TokenKind::Identifier {
        return Err(ParseError::at(
            token.span,
            ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
            "a type-parameter constraint is `supports equality` or `supports order`",
        ));
    }
    match token.text(source) {
        "equality" => Ok(TypeConstraint::Equality),
        "order" => Ok(TypeConstraint::Order),
        _ => Err(ParseError::at(
            token.span,
            ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionParameterList),
            "a type-parameter constraint is `supports equality` or `supports order`",
        )),
    }
}

/// Index of the `]` matching the leading `[` of `tokens`, if balanced.
pub(super) fn match_bracket(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftBracket => depth += 1,
            TokenKind::RightBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Index of the `>` matching the leading `<` of `tokens`, if balanced. Tracks `<`/`>`
/// depth so a nested generic type argument closes correctly; within a declaration
/// header a nested close is always a bare `>` (no `>>` token exists).
pub(super) fn match_angle(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Less => depth += 1,
            TokenKind::Greater => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a `name: type` parameter list. Parameters are separated by
/// commas, and in a multi-line list a line break separates one from the next just
/// as a comma does, so the list reads cleanly written with commas, without them,
/// or mixed. A run of `;;` doc lines directly above a parameter is its
/// documentation, captured in source order.
fn parse_params_tokens(source: &str, inner: &[Token]) -> ParseResult<Vec<ParamDecl>> {
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for group in split_param_groups(inner) {
        // A doc run with no parameter after it documents nothing; report the
        // misplaced doc rather than dropping it.
        if group.body.is_empty() {
            return Err(ParseError::new(
                ParseDiagnosticReason::DocCommentBeforeParameter,
                "a doc comment must precede a parameter",
            ));
        }
        let docs = group
            .docs
            .iter()
            .map(|token| doc_comment_text(token.text(source)))
            .collect();
        reject_removed_parameter_mode(source, group.body)?;
        let rest = group.body;
        let name = match rest.first() {
            Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
            _ => {
                return Err(ParseError::new(
                    ParseDiagnosticReason::Expected(ExpectedSyntax::ParameterName),
                    "expected parameter name",
                ));
            }
        };
        // A keyed-collection parameter spells its key columns like the local
        // declaration head — `scores[player: string]: int` — reusing the same
        // key-parameter parse as a keyed `var`, field, or store root.
        let (keys, after_keys) = parse_param_keys(source, rest)?;
        if rest.get(after_keys).map(|token| token.kind) != Some(TokenKind::Colon)
            || rest.len() < after_keys + 2
        {
            // Point at where the `: type` annotation should begin — the first
            // token past the name and any key list, or the final body token when
            // the annotation is missing entirely — not the declaration header.
            let span = rest.get(after_keys).unwrap_or(&rest[rest.len() - 1]).span;
            return Err(ParseError::at(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ParameterType),
                "expected parameter type annotation",
            ));
        }
        let ty_tokens = &rest[after_keys + 1..];
        if let Some(equal) = find_top_level_equal(ty_tokens) {
            let ty_before_default = &ty_tokens[..equal];
            if ty_before_default.is_empty() {
                return Err(ParseError::at(
                    rest[after_keys].span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::ParameterType),
                    "expected parameter type annotation",
                ));
            }
            parse_type(
                source,
                ty_before_default,
                ExpectedSyntax::ParameterType,
                "expected parameter type annotation",
            )?;
            return Err(ParseError::new(
                ParseDiagnosticReason::Unsupported(UnsupportedSyntax::ParameterDefaults),
                "parameter defaults are not used in Marrow",
            ));
        }
        let ty = parse_type(
            source,
            ty_tokens,
            ExpectedSyntax::ParameterType,
            "expected parameter type annotation",
        )?;
        params.push(ParamDecl {
            docs,
            name,
            keys,
            ty,
        });
    }
    Ok(params)
}

/// Parse an optional `[key: type, ...]` key-parameter list that follows a
/// parameter name, marking it a local keyed collection. Returns the parsed keys
/// (empty when no `[` follows the name) and the index in `rest` of the first
/// token past the key list, where the `: value-type` annotation begins.
fn parse_param_keys(source: &str, rest: &[Token]) -> ParseResult<(Vec<KeyParam>, usize)> {
    if rest.get(1).map(|token| token.kind) != Some(TokenKind::LeftBracket) {
        return Ok((Vec::new(), 1));
    }
    let close = match_bracket(&rest[1..])
        .map(|close| close + 1)
        .ok_or(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::KeyParameterList),
            "expected key parameter list",
        ))?;
    let keys = parse_key_params_tokens(source, &rest[2..close])?;
    Ok((keys, close + 1))
}

fn reject_removed_parameter_mode(source: &str, tokens: &[Token]) -> ParseResult<()> {
    let [mode, name, colon, ..] = tokens else {
        return Ok(());
    };
    if mode.kind != TokenKind::Identifier
        || name.kind != TokenKind::Identifier
        || colon.kind != TokenKind::Colon
    {
        return Ok(());
    }
    let text = mode.text(source);
    if text != "inout" && text != "out" {
        return Ok(());
    }
    Err(ParseError::new(
        ParseDiagnosticReason::Unsupported(UnsupportedSyntax::ParameterModes),
        "parameter modes were removed; parameters are read-only by value; return the new value",
    ))
}

/// One parameter's tokens: its leading `;;` doc-comment run and the body tokens
/// that spell `name: type`.
struct ParamGroup<'a> {
    docs: Vec<&'a Token>,
    body: &'a [Token],
}

/// Split a parameter list's inner tokens into per-parameter groups. A top-level
/// comma ends a parameter, and so does a line break in a multi-line list: a body
/// token that opens on a later source line than the parameter in progress starts
/// the next one. Newlines are suppressed inside the parentheses, so the line
/// boundary is read from token spans rather than a separator token. Depth counts
/// `(`/`[`/`<` opens so a comma or wrap inside a nested type — a `(...)` identity,
/// a `[...]` key list, or a `<...>` generic argument list — stays with its
/// parameter. A leading run of `;;` doc comments attaches to the parameter it
/// precedes.
fn split_param_groups(inner: &[Token]) -> Vec<ParamGroup<'_>> {
    let mut groups = Vec::new();
    let mut docs: Vec<&Token> = Vec::new();
    let mut body_start: Option<usize> = None;
    let mut depth = 0usize;

    let mut index = 0;
    while index < inner.len() {
        let token = &inner[index];
        // The depth before this token's own bracket is what places the token: a
        // closing `]` or `)` still belongs to the type it closes, so it reads at
        // the deeper level even though it drops the depth back afterwards.
        let depth_before = depth;
        match token.kind {
            // A generic argument list (`Map<int, string>`) carries an internal
            // comma; its `<`/`>` count toward depth exactly like `(`/`[` so that
            // comma does not end the parameter. A parameter body is a type slice
            // (defaults are rejected), so `<`/`>` here are never comparison.
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::Less => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::Greater => {
                depth = depth.saturating_sub(1)
            }
            _ => {}
        }

        if depth == 0 && token.kind == TokenKind::Comma {
            if let Some(start) = body_start.take() {
                push_param_group(&mut groups, &mut docs, &inner[start..index]);
            }
            index += 1;
            continue;
        }

        if token.kind == TokenKind::DocComment {
            // A doc comment that opens a new parameter's documentation follows a
            // completed parameter body, so close that body before collecting it.
            if let Some(start) = body_start.take() {
                push_param_group(&mut groups, &mut docs, &inner[start..index]);
            }
            docs.push(token);
            index += 1;
            continue;
        }

        if token.kind == TokenKind::Comment {
            // A `;` comment inside the parentheses documents nothing and, like a
            // blank line, neither separates nor closes a parameter; the line break
            // to the next parameter is read from the following token's span.
            index += 1;
            continue;
        }

        match body_start {
            None => body_start = Some(index),
            // A body token on a later source line than the parameter in progress
            // begins the next parameter, so a line break separates parameters the
            // same way a comma does. Only a top-level line break ends a parameter;
            // a parameter occupies one logical line, and its type may still wrap
            // across physical lines inside `(` or `[`, where the deeper depth keeps
            // the wrap from splitting the parameter.
            Some(start) if depth_before == 0 && token.span.line > inner[start].span.line => {
                push_param_group(&mut groups, &mut docs, &inner[start..index]);
                body_start = Some(index);
            }
            Some(_) => {}
        }
        index += 1;
    }

    match body_start {
        Some(start) => push_param_group(&mut groups, &mut docs, &inner[start..]),
        // A `;;` run with no parameter after it documents nothing. Report it as a
        // body-less group so the caller can report the misplaced doc rather than
        // drop it.
        None if !docs.is_empty() => push_param_group(&mut groups, &mut docs, &inner[inner.len()..]),
        None => {}
    }
    groups
}

/// Close one parameter group, pairing `body` with the doc run accumulated so far
/// and clearing the doc buffer so the next parameter starts with an empty run.
fn push_param_group<'a>(
    groups: &mut Vec<ParamGroup<'a>>,
    docs: &mut Vec<&'a Token>,
    body: &'a [Token],
) {
    groups.push(ParamGroup {
        docs: std::mem::take(docs),
        body,
    });
}

/// Index of the `)` matching the leading `(` of `tokens`, if balanced.
pub(super) fn match_paren(tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen => depth += 1,
            TokenKind::RightParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}
