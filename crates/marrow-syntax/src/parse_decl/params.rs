//! Token-level parsers for a function header and its parameter list, including
//! the multi-line parameter grouping that lets a line break separate parameters
//! the way a comma does.

use super::tokens::{
    doc_comment_text, find_top_level_equal, reject_structural_type_tokens, type_ref_from_tokens,
};
use super::{FunctionHead, ParseError, ParseResult};
use crate::ast::ParamDecl;
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, UnsupportedSyntax};
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
    let name = match rest.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionName),
                "expected function name",
            ));
        }
    };
    let rest = &rest[1..];
    if matches!(rest.first().map(|token| token.kind), Some(TokenKind::Less)) {
        return Err(ParseError::new(
            ParseDiagnosticReason::Unsupported(UnsupportedSyntax::UserDefinedGenerics),
            "user-defined generics are not used in Marrow",
        ));
    }
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
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionReturnType),
                "expected return type after `:`",
            ));
        }
        let ty_tokens = &after[1..];
        if ty_tokens.is_empty() {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionReturnType),
                "expected return type after `:`",
            ));
        }
        reject_structural_type_tokens(
            ty_tokens,
            ExpectedSyntax::FunctionReturnType,
            "expected return type after `:`",
        )?;
        let ty = type_ref_from_tokens(source, ty_tokens);
        Some(ty)
    };
    Ok(FunctionHead {
        public,
        name,
        params,
        return_type,
    })
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
        if rest.get(1).map(|token| token.kind) != Some(TokenKind::Colon) || rest.len() < 3 {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::ParameterType),
                "expected parameter type annotation",
            ));
        }
        let ty_tokens = &rest[2..];
        if let Some(equal) = find_top_level_equal(ty_tokens) {
            let ty_before_default = &ty_tokens[..equal];
            if ty_before_default.is_empty() {
                return Err(ParseError::new(
                    ParseDiagnosticReason::Expected(ExpectedSyntax::ParameterType),
                    "expected parameter type annotation",
                ));
            }
            reject_structural_type_tokens(
                ty_before_default,
                ExpectedSyntax::ParameterType,
                "expected parameter type annotation",
            )?;
            return Err(ParseError::new(
                ParseDiagnosticReason::Unsupported(UnsupportedSyntax::ParameterDefaults),
                "parameter defaults are not used in Marrow",
            ));
        }
        reject_structural_type_tokens(
            ty_tokens,
            ExpectedSyntax::ParameterType,
            "expected parameter type annotation",
        )?;
        let ty = type_ref_from_tokens(source, ty_tokens);
        params.push(ParamDecl { docs, name, ty });
    }
    Ok(params)
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
/// boundary is read from token spans rather than a separator token. A leading run
/// of `;;` doc comments attaches to the parameter it precedes.
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
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
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
        // A `;;` run with no parameter after it documents nothing. Surface it as a
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
