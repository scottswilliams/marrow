//! Parsers for a single statement line once its boundaries are known: the
//! `parse_simple_statement` dispatch and the per-keyword line parsers (`const`,
//! `var`, `return`, `break`/`continue`, assignment, the `for` and `catch`
//! headers) it delegates to.

use super::head::parse_key_params_tokens;
use super::params::match_paren;
use super::tokens::{
    expr_of, expr_of_after, expr_of_before, expr_of_in_header, find_top_level,
    find_top_level_equal, line_span_or, push_parse_error, reject_structural_type_tokens,
    type_ref_from_tokens,
};
use super::{ParseError, ParseResult};
use crate::PARSE_SYNTAX;
use crate::ast::{Expression, ForBinding, KeyParam, Statement, TypeRef};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, Severity,
    UnsupportedSyntax,
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
            let value = expr_of_after(source, &line[1..], first.span, diagnostics)?;
            Some(Statement::Delete {
                span: join_spans(first.span, value.span()),
                path: value,
            })
        }
        TokenKind::Keyword(Keyword::Throw) => {
            let value = expr_of_after(source, &line[1..], first.span, diagnostics)?;
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
                span: line_span_or(line, line[0].span),
            });
            None
        }
        TokenKind::Keyword(Keyword::Break) => parse_break_or_continue(line, true, diagnostics),
        TokenKind::Keyword(Keyword::Continue) => parse_break_or_continue(line, false, diagnostics),
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
                push_parse_error(diagnostics, line_span_or(line, line[0].span), error);
                return None;
            }
        }
    }

    let mut ty = None;
    if line.get(index).map(|token| token.kind) == Some(TokenKind::Colon) {
        index += 1;
        let type_start = index;
        let type_end = find_top_level_equal(&line[type_start..])
            .map(|equal| type_start + equal)
            .unwrap_or(line.len());
        if type_end == type_start {
            return None;
        }
        let expected = if is_var {
            ExpectedSyntax::ParameterType
        } else {
            ExpectedSyntax::ConstType
        };
        let message = if is_var {
            "expected variable type annotation"
        } else {
            "expected const type annotation"
        };
        if let Err(error) =
            reject_structural_type_tokens(&line[type_start..type_end], expected, message)
        {
            push_parse_error(diagnostics, line_span_or(line, line[0].span), error);
            return None;
        }
        ty = Some(type_ref_from_tokens(source, &line[type_start..type_end]));
        index = type_end;
    }

    match line.get(index).map(|token| token.kind) {
        Some(TokenKind::Equal) => {
            let equal = line[index];
            let value = expr_of_after(source, &line[index + 1..], equal.span, diagnostics)?;
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
    let close = match_paren(&line[open_index..])
        .map(|close| open_index + close)
        .ok_or(ParseError::new(
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
    if matches!(
        line.get(1).map(|token| token.kind),
        Some(TokenKind::Keyword(Keyword::Absent))
    ) && line.len() == 2
    {
        return Some(Statement::ReturnAbsent {
            span: join_spans(keyword.span, line[1].span),
        });
    }
    let value = expr_of_after(source, &line[1..], keyword.span, diagnostics)?;
    Some(Statement::Return {
        span: join_spans(keyword.span, value.span()),
        value: Some(value),
    })
}

fn parse_break_or_continue(
    line: &[Token],
    is_break: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    let keyword = line[0];
    let span = match line.get(1) {
        None => keyword.span,
        Some(token) if token.kind == TokenKind::Identifier && line.len() == 2 => {
            diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Unsupported(
                    UnsupportedSyntax::LoopLabels,
                )),
                severity: Severity::Error,
                message: "loop labels were removed".to_string(),
                help: Some("extract a function and use return to leave nested loops".to_string()),
                span: token.span,
            });
            join_spans(keyword.span, token.span)
        }
        _ => return None,
    };
    Some(if is_break {
        Statement::Break { span }
    } else {
        Statement::Continue { span }
    })
}

fn parse_assign_or_expr(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Statement> {
    if let Some(equal) = find_top_level_equal(line) {
        let equal_span = line[equal].span;
        let target = expr_of_before(source, &line[..equal], equal_span, diagnostics)?;
        let value = expr_of_after(source, &line[equal + 1..], equal_span, diagnostics)?;
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

/// Parse an `if const name [: type] = place` head into the bound name, optional
/// type annotation, and the place expression. `line` starts at the `const`
/// keyword. The annotation is validated and stored exactly as `const`/`var` does;
/// the trailing `=` and value are required. Returns `None` (after reporting a
/// non-identifier name) when the head is not a binding, so the caller falls back
/// to treating the line as an ordinary condition expression.
pub(super) fn parse_if_const_head(
    source: &str,
    line: &[Token],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(String, Option<TypeRef>, Expression)> {
    let name_token = line.get(1)?;
    if name_token.kind != TokenKind::Identifier {
        if matches!(name_token.kind, TokenKind::Keyword(_)) {
            diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::ConstName,
                )),
                severity: Severity::Error,
                message: format!(
                    "expected const name; `{}` is a keyword",
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
    let mut ty = None;
    if line.get(index).map(|token| token.kind) == Some(TokenKind::Colon) {
        index += 1;
        let type_start = index;
        let type_end = find_top_level_equal(&line[type_start..])
            .map(|equal| type_start + equal)
            .unwrap_or(line.len());
        if type_end == type_start {
            return None;
        }
        if let Err(error) = reject_structural_type_tokens(
            &line[type_start..type_end],
            ExpectedSyntax::ConstType,
            "expected const type annotation",
        ) {
            push_parse_error(diagnostics, line_span_or(line, line[0].span), error);
            return None;
        }
        ty = Some(type_ref_from_tokens(source, &line[type_start..type_end]));
        index = type_end;
    }

    if line.get(index).map(|token| token.kind) != Some(TokenKind::Equal) {
        return None;
    }
    let equal = line[index];
    let value = expr_of_after(source, &line[index + 1..], equal.span, diagnostics)?;
    Some((name, ty, value))
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
            let step = expr_of_in_header(source, &after_in[by_index + 1..], diagnostics)?;
            (&after_in[..by_index], Some(step))
        }
        None => (after_in, None),
    };
    let iterable = expr_of_in_header(source, iterable_tokens, diagnostics)?;
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
pub(super) fn parse_catch_header(
    source: &str,
    header: &[Token],
) -> ParseResult<(String, Option<TypeRef>)> {
    let Some(name_token) = header.first() else {
        return Ok((String::new(), None));
    };
    if name_token.kind != TokenKind::Identifier {
        return Ok((String::new(), None));
    }
    let name = name_token.text(source).to_string();
    let ty = match header.get(1) {
        Some(colon) if colon.kind == TokenKind::Colon && header.len() > 2 => {
            reject_structural_type_tokens(
                &header[2..],
                ExpectedSyntax::ParameterType,
                "expected catch type annotation",
            )?;
            Some(type_ref_from_tokens(source, &header[2..]))
        }
        _ => None,
    };
    Ok((name, ty))
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
