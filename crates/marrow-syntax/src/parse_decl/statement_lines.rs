//! Parsers for a single statement line once its boundaries are known: the
//! `parse_simple_statement` dispatch and the per-keyword line parsers (`const`,
//! `var`, `return`, `break`/`continue`, assignment, the `for` and `catch`
//! headers) it delegates to.

use super::head::parse_key_params_tokens;
use super::params::match_paren;
use super::tokens::{
    expr_of, expr_of_after, expr_of_before, expr_of_in_header, find_top_level,
    find_top_level_compound_assign, find_top_level_equal, line_span_or, parse_type,
    push_parse_error,
};
use super::{ParseError, ParseResult};
use crate::PARSE_SYNTAX;
use crate::ast::{
    CompoundAssignOp, Expression, ForBinding, ForName, KeyParam, LoopOrder, Statement, TypeExpr,
};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, ReservedSyntax, Severity,
    UnsupportedSyntax,
};
use crate::parse_expr::join_spans;
use crate::token::{Keyword, Token, TokenKind};

/// Report that `line` does not form a statement, at the line span, and yield
/// `None`. The single owner of the generic statement-shape failure, so every
/// unstructured line carries one diagnostic without a separate fallback pass.
fn expected_statement(line: &[Token], diagnostics: &mut Vec<Diagnostic>) -> Option<Statement> {
    let span = line_span_or(line, line[0].span);
    diagnostics.push(Diagnostic {
        code: ParseDiagnosticReason::Expected(ExpectedSyntax::Statement).code(),
        reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
            ExpectedSyntax::Statement,
        )),
        severity: Severity::Error,
        message: "expected a statement".to_string(),
        help: None,
        span,
    });
    None
}

/// Report that `line` is missing the expression it needed, at the line span, and
/// yield `None`. Used where a malformed header could not be structured as either
/// its binding form or a condition expression.
fn expected_expression_line<T>(line: &[Token], diagnostics: &mut Vec<Diagnostic>) -> Option<T> {
    let span = line_span_or(line, line[0].span);
    diagnostics.push(Diagnostic {
        code: ParseDiagnosticReason::Expected(ExpectedSyntax::Expression).code(),
        reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
            ExpectedSyntax::Expression,
        )),
        severity: Severity::Error,
        message: "expected an expression".to_string(),
        help: None,
        span,
    });
    None
}

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
    let Some(name_token) = line.get(1) else {
        return expected_statement(line, diagnostics);
    };
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
            return None;
        }
        return expected_statement(line, diagnostics);
    }
    let name = name_token.text(source).to_string();
    let mut index = 2;

    // A keyed `var` declares a local keyed tree: `var counts(name: string): int`.
    // `const` has no key parameters.
    let mut keys = Vec::new();
    if line.get(index).map(|token| token.kind) == Some(TokenKind::LeftParen) {
        if !is_var {
            return expected_statement(line, diagnostics);
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
            return expected_statement(line, diagnostics);
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
        match parse_type(source, &line[type_start..type_end], expected, message) {
            Ok(parsed) => ty = Some(parsed),
            Err(error) => {
                push_parse_error(diagnostics, line_span_or(line, line[0].span), error);
                return None;
            }
        }
        index = type_end;
    }

    match line.get(index).map(|token| token.kind) {
        Some(TokenKind::Equal) => {
            let equal = line[index];
            let value = parse_rhs_value(source, &line[index + 1..], equal.span, diagnostics)?;
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
        _ => expected_statement(line, diagnostics),
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
    let value = parse_rhs_value(source, &line[1..], keyword.span, diagnostics)?;
    Some(Statement::Return {
        span: join_spans(keyword.span, value.span()),
        value: Some(value),
    })
}

/// Parse a statement's right-hand-side value expression, recognizing a leading
/// prefix `try`. Prefix `try` is a statement-level value form only, so it is
/// stripped and wrapped here rather than in the general expression grammar; a
/// `try` nested inside a larger expression stays a parse error.
fn parse_rhs_value(
    source: &str,
    tokens: &[Token],
    anchor: crate::diagnostic::SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    if let Some(first) = tokens.first()
        && first.kind == TokenKind::Keyword(Keyword::Try)
    {
        let inner = expr_of_after(source, &tokens[1..], first.span, diagnostics)?;
        let span = join_spans(first.span, inner.span());
        return Some(Expression::Try {
            inner: Box::new(inner),
            span,
        });
    }
    expr_of_after(source, tokens, anchor, diagnostics)
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
        _ => return expected_statement(line, diagnostics),
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
    if let Some(op_index) = find_top_level_compound_assign(line) {
        let op_token = line[op_index];
        let op = CompoundAssignOp::from_operator_token(op_token.kind)
            .expect("find_top_level_compound_assign yields a compound-assign token");
        let target = expr_of_before(source, &line[..op_index], op_token.span, diagnostics)?;
        let value = expr_of_after(source, &line[op_index + 1..], op_token.span, diagnostics)?;
        return Some(Statement::CompoundAssign {
            span: join_spans(target.span(), value.span()),
            target,
            op,
            op_span: op_token.span,
            value,
        });
    }
    if let Some(equal) = find_top_level_equal(line) {
        let equal_span = line[equal].span;
        // A compound operator lexes as one token, so an arithmetic operator with
        // a space before the `=` (`x * = y`) is the split spelling: reject it
        // rather than silently canonicalize.
        if equal > 0 && is_split_compound_operator(line[equal - 1].kind) {
            let op_span = line[equal - 1].span;
            diagnostics.push(Diagnostic {
                code: PARSE_SYNTAX,
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::SplitCompoundAssign),
                severity: Severity::Error,
                message: "write a compound assignment as one operator (`*=`), not a spaced `* =`"
                    .to_string(),
                help: None,
                span: join_spans(op_span, equal_span),
            });
            return None;
        }
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

/// Whether a bare arithmetic-operator token, sitting directly before a top-level
/// `=`, spells the split form of a compound assignment (`+ =`, `- =`, `* =`,
/// `/ =`, `% =`).
fn is_split_compound_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
    )
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
) -> Option<(String, Option<TypeExpr>, Expression)> {
    let Some(name_token) = line.get(1) else {
        return expected_expression_line(line, diagnostics);
    };
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
            return None;
        }
        return expected_expression_line(line, diagnostics);
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
            return expected_expression_line(line, diagnostics);
        }
        match parse_type(
            source,
            &line[type_start..type_end],
            ExpectedSyntax::ConstType,
            "expected const type annotation",
        ) {
            Ok(parsed) => ty = Some(parsed),
            Err(error) => {
                push_parse_error(diagnostics, line_span_or(line, line[0].span), error);
                return None;
            }
        }
        index = type_end;
    }

    if line.get(index).map(|token| token.kind) != Some(TokenKind::Equal) {
        return expected_expression_line(line, diagnostics);
    }
    let equal = line[index];
    let value = expr_of_after(source, &line[index + 1..], equal.span, diagnostics)?;
    Some((name, ty, value))
}

/// Parse a `for` header `binding in [reversed] iterable [by step]` into the loop
/// binding, traversal order, the iterable expression, and the optional range step.
/// Returns `None` if the `in` keyword or binding is malformed, or if `reversed`
/// stands in the head slot with no iterable after it. `reversed` is a reserved
/// head-slot keyword: an identifier spelling `reversed` immediately after `in` is
/// always the order keyword, never the iterable. `by` is a contextual keyword: it
/// splits the header only as a bare top-level word, so a name `by` elsewhere is
/// unaffected.
pub(super) fn parse_for_header(
    source: &str,
    header: &[Token],
) -> Option<(ForBinding, LoopOrder, Expression, Option<Expression>)> {
    let in_index = find_top_level(header, TokenKind::Keyword(Keyword::In))?;
    let binding = parse_for_binding(source, &header[..in_index])?;
    let after_in = &header[in_index + 1..];
    let (order, rest) = match after_in.first() {
        Some(token) if is_reversed_keyword(source, token) => (LoopOrder::Reversed, &after_in[1..]),
        _ => (LoopOrder::Forward, after_in),
    };
    // A bare `reversed` in the head slot has no iterable to walk; the empty rest
    // fails `expr_of_in_header` below, which the caller reports as a for-header error.
    let (iterable_tokens, step) = match find_top_level_by(source, rest) {
        Some(by_index) => {
            let step = expr_of_in_header(source, &rest[by_index + 1..])?;
            (&rest[..by_index], Some(step))
        }
        None => (rest, None),
    };
    let iterable = expr_of_in_header(source, iterable_tokens)?;
    Some((binding, order, iterable, step))
}

/// Whether `token` is the head-slot `reversed` keyword: an ordinary identifier
/// spelling `reversed`. It is reserved only in the loop-head order slot; anywhere
/// else it is a normal name.
fn is_reversed_keyword(source: &str, token: &Token) -> bool {
    token.kind == TokenKind::Identifier && token.text(source) == "reversed"
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

/// Parse the comma-separated loop-head names `a`, `a, b`, `a, b, c`, ... into a
/// non-empty name vector, each name carrying its own span. Names alternate with
/// commas; any other shape (empty, trailing comma, non-identifier) fails the header.
fn parse_for_binding(source: &str, tokens: &[Token]) -> Option<ForBinding> {
    if tokens.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if index % 2 == 0 {
            if token.kind != TokenKind::Identifier {
                return None;
            }
            names.push(ForName {
                name: token.text(source).to_string(),
                span: token.span,
            });
        } else if token.kind != TokenKind::Comma {
            return None;
        }
    }
    // A trailing comma leaves the final token at an even index without a following
    // name, so the loop ends on a comma — reject that dangling separator.
    if tokens.len() % 2 == 0 {
        return None;
    }
    Some(ForBinding { names })
}
