//! Parsers for a single statement line once its boundaries are known: the
//! `parse_simple_statement` dispatch and the per-keyword line parsers (`const`,
//! `var`, `return`, `break`/`continue`, assignment, the `for` and `catch`
//! headers) it delegates to.

use super::head::parse_key_params_tokens;
use super::tokens::{
    expr_of, find_top_level, find_top_level_equal, line_span, push_parse_error,
    type_ref_from_tokens,
};
use super::{ParseError, ParseResult};
use crate::PARSE_SYNTAX;
use crate::ast::{Expression, ForBinding, KeyParam, Statement, TypeRef};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, Severity,
};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};

pub(super) fn parse_simple_statement(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let first = line.first()?;
    match first.kind {
        TokenKind::Keyword(Keyword::Const) => parse_const_or_var(source, line, false, diagnostics),
        TokenKind::Keyword(Keyword::Var) => parse_const_or_var(source, line, true, diagnostics),
        TokenKind::Keyword(Keyword::Return) => parse_return(source, line, diagnostics),
        TokenKind::Keyword(Keyword::Delete) => {
            let value = expr_of(source, &line[1..], diagnostics)?;
            Some(Statement::Delete {
                span: join_spans(first.span, value.span()),
                path: value,
            })
        }
        TokenKind::Keyword(Keyword::Throw) => {
            let value = expr_of(source, &line[1..], diagnostics)?;
            Some(Statement::Throw {
                span: join_spans(first.span, value.span()),
                value,
            })
        }
        TokenKind::Keyword(Keyword::Merge) => {
            diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Reserved(
                    ReservedSyntax::MergeStatement,
                )),
                severity: Severity::Error,
                message: "`merge` is reserved and is not a v0.1 statement".to_string(),
                help: None,
                span: line_span(line),
            });
            None
        }
        TokenKind::Keyword(Keyword::Break) => parse_break_or_continue(source, line, true),
        TokenKind::Keyword(Keyword::Continue) => parse_break_or_continue(source, line, false),
        _ => parse_assign_or_expr(source, line, diagnostics),
    }
}

fn parse_const_or_var(
    source: &str,
    line: &[Token],
    is_var: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let keyword = line[0];
    let name_token = line.get(1)?;
    if name_token.kind != TokenKind::Identifier {
        if matches!(name_token.kind, TokenKind::Keyword(_)) {
            let kind = if is_var { "variable" } else { "const" };
            diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(if is_var {
                    ExpectedSyntax::VariableName
                } else {
                    ExpectedSyntax::ConstName
                })),
                severity: Severity::Error,
                message: format!(
                    "expected {kind} name; `{}` is a keyword",
                    name_token.text(source)
                ),
                help: Some("choose an identifier that is not reserved".to_string()),
                span: name_token.span,
            });
        }
        return None;
    }
    let name = name_token.text(source).to_string();
    let mut index = 2;

    // A keyed `var` declares a local keyed tree: `var counts(name: string): int`.
    // `const` has no key parameters.
    let mut keys = Vec::new();
    if line.get(index).map(|token| token.kind) == Some(TokenKind::LeftParen) {
        if !is_var {
            return None;
        }
        match parse_var_keys(source, line, index) {
            Ok((parsed_keys, after)) => {
                keys = parsed_keys;
                index = after;
            }
            Err(error) => {
                push_parse_error(diagnostics, line_span(line), error);
                return None;
            }
        }
    }

    let mut ty = None;
    if line.get(index).map(|token| token.kind) == Some(TokenKind::Colon) {
        index += 1;
        let type_start = index;
        while index < line.len() && line[index].kind != TokenKind::Equal {
            index += 1;
        }
        if index == type_start {
            return None;
        }
        ty = Some(type_ref_from_tokens(source, &line[type_start..index]));
    }

    match line.get(index).map(|token| token.kind) {
        Some(TokenKind::Equal) => {
            let value = expr_of(source, &line[index + 1..], diagnostics)?;
            let span = join_spans(keyword.span, value.span());
            Some(if is_var {
                Statement::Var {
                    name,
                    keys,
                    ty,
                    value: Some(value),
                    span,
                }
            } else {
                Statement::Const {
                    name,
                    ty,
                    value,
                    span,
                }
            })
        }
        // `var name[(keys)][: type]` without an initializer is allowed; `const` is not.
        None if is_var => Some(Statement::Var {
            name,
            keys,
            ty,
            value: None,
            span: join_spans(keyword.span, line[line.len() - 1].span),
        }),
        _ => None,
    }
}

/// Parse `(name: type, ...)` key parameters of a keyed `var`, starting at the
/// `(` token at `open_index`. Returns the parsed keys and the line index just
/// past the closing `)`.
fn parse_var_keys(
    source: &str,
    line: &[Token],
    open_index: usize,
) -> ParseResult<(Vec<KeyParam>, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (offset, token) in line[open_index..].iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen => depth += 1,
            TokenKind::RightParen => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open_index + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.ok_or(ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::KeyParameterList),
        "expected key parameter list",
    ))?;
    let keys = parse_key_params_tokens(source, &line[open_index + 1..close])?;
    Ok((keys, close + 1))
}

fn parse_return(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let keyword = line[0];
    if line.len() == 1 {
        return Some(Statement::Return {
            value: None,
            span: keyword.span,
        });
    }
    let value = expr_of(source, &line[1..], diagnostics)?;
    Some(Statement::Return {
        span: join_spans(keyword.span, value.span()),
        value: Some(value),
    })
}

fn parse_break_or_continue(source: &str, line: &[Token], is_break: bool) -> Option<Statement> {
    let keyword = line[0];
    let (label, span) = match line.get(1) {
        None => (None, keyword.span),
        Some(token) if token.kind == TokenKind::Identifier && line.len() == 2 => (
            Some(token.text(source).to_string()),
            join_spans(keyword.span, token.span),
        ),
        _ => return None,
    };
    Some(if is_break {
        Statement::Break { label, span }
    } else {
        Statement::Continue { label, span }
    })
}

fn parse_assign_or_expr(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    if let Some(equal) = find_top_level_equal(line) {
        let target = expr_of(source, &line[..equal], diagnostics)?;
        let value = expr_of(source, &line[equal + 1..], diagnostics)?;
        Some(Statement::Assign {
            span: join_spans(target.span(), value.span()),
            target,
            value,
        })
    } else {
        let value = expr_of(source, line, diagnostics)?;
        Some(Statement::Expr {
            span: value.span(),
            value,
        })
    }
}

/// Parse a `for` header `binding in iterable [by step]` into the loop binding,
/// the iterable expression, and the optional range step. Returns `None` if the
/// `in` keyword or binding is malformed. `by` is a contextual keyword: it splits
/// the header only as a bare top-level word, so a name `by` elsewhere is unaffected.
pub(super) fn parse_for_header(
    source: &str,
    header: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(ForBinding, Expression, Option<Expression>)> {
    let in_index = find_top_level(header, TokenKind::Keyword(Keyword::In))?;
    let binding = parse_for_binding(source, &header[..in_index])?;
    let after_in = &header[in_index + 1..];
    let (iterable_tokens, step) = match find_top_level_by(source, after_in) {
        Some(by_index) => {
            let step = expr_of(source, &after_in[by_index + 1..], diagnostics)?;
            (&after_in[..by_index], Some(step))
        }
        None => (after_in, None),
    };
    let iterable = expr_of(source, iterable_tokens, diagnostics)?;
    Some((binding, iterable, step))
}

/// Index of a top-level contextual `by` in a range-for header. `by` is a plain
/// identifier, not a reserved word, so it splits the header only when it stands at
/// bracket depth 0 — never inside a call's arguments or a name `by` used as a value.
fn find_top_level_by(source: &str, tokens: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            TokenKind::Identifier if depth == 0 && token.text(source) == "by" => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// Parse a `catch` header `name` or `name: Type` into the bound name and an
/// optional type annotation. A malformed header yields an empty name.
pub(super) fn parse_catch_header(source: &str, header: &[Token]) -> (String, Option<TypeRef>) {
    let Some(name_token) = header.first() else {
        return (String::new(), None);
    };
    if name_token.kind != TokenKind::Identifier {
        return (String::new(), None);
    }
    let name = name_token.text(source).to_string();
    let ty = match header.get(1) {
        Some(colon) if colon.kind == TokenKind::Colon && header.len() > 2 => {
            Some(type_ref_from_tokens(source, &header[2..]))
        }
        _ => None,
    };
    (name, ty)
}

fn parse_for_binding(source: &str, tokens: &[Token]) -> Option<ForBinding> {
    let ident = |token: &Token| {
        (token.kind == TokenKind::Identifier).then(|| token.text(source).to_string())
    };
    match tokens {
        [first] => Some(ForBinding {
            first: ident(first)?,
            second: None,
        }),
        [first, comma, second] if comma.kind == TokenKind::Comma => Some(ForBinding {
            first: ident(first)?,
            second: Some(ident(second)?),
        }),
        _ => None,
    }
}
