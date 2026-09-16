//! The active-call query: which call the cursor sits inside, that callee's rendered
//! signature, and which argument slot the offset occupies.
//!
//! Like [`completion`], a read-only pass over the query-local parse: it never drives the
//! compile-path resolver, so it runs on a broken file over recovered incomplete-call
//! nodes and leaks no diagnostic. The callee is resolved only against same-module
//! declarations in that parse; a cross-module callee, a built-in, or an unknown name
//! resolves to no local declaration and is a legitimate absence.

use marrow_syntax::{
    Argument, Block, Declaration, Expression, FunctionDecl, InterpolationPart, SourceSpan,
    Statement,
};

use super::completion::{contains, declaration_contains};
use super::{
    ActiveCall, ActiveCallOutcome, AnalysisResourceLimit, Fact, MAX_ACTIVE_CALL_RENDER_BYTES,
    ParamPiece, QueryFile,
};

/// One call node reached during collection: its callee expression, its arguments
/// parsed so far, and its full span. A recovered incomplete call carries the arguments
/// parsed before the missing delimiter and a span ending at the last parsed token.
struct CallSite<'a> {
    callee: &'a Expression,
    args: &'a [Argument],
    span: SourceSpan,
}

pub(super) fn resolve(file: &QueryFile<'_>, source: &[u8]) -> ActiveCallOutcome {
    let offset = file.offset;
    let Some(declaration) = file
        .declarations
        .iter()
        .find(|declaration| declaration_contains(declaration, offset))
    else {
        return ActiveCallOutcome::Ready(Fact::Absent);
    };
    let mut sites = Vec::new();
    collect_declaration_calls(declaration, &mut sites);
    // The innermost enclosing call is the smallest-span call whose argument region holds
    // the offset. A recovered incomplete call extends its region across trailing
    // whitespace to the cursor, so `f(` and `f(a, ` still resolve.
    let Some(site) = sites
        .into_iter()
        .filter(|site| region_contains(site, source, offset))
        .min_by_key(|site| site.span.end_byte - site.span.start_byte)
    else {
        return ActiveCallOutcome::Ready(Fact::Absent);
    };
    let Some(function) = resolve_callee(file, site.callee) else {
        return ActiveCallOutcome::Ready(Fact::Absent);
    };
    let (signature, params) = render_signature(function);
    let active = active_index(&site, offset, params.len());
    finish(signature, params, active)
}

/// Apply the per-query rendered-byte cap, then package the fact. An over-cap display is
/// a query-local refusal, never a truncated display.
fn finish(signature: String, params: Vec<ParamPiece>, active: Option<u16>) -> ActiveCallOutcome {
    let bytes = signature.len() as u64
        + params
            .iter()
            .map(|piece| piece.label.len() as u64)
            .sum::<u64>();
    if bytes > MAX_ACTIVE_CALL_RENDER_BYTES {
        return ActiveCallOutcome::Refused(AnalysisResourceLimit::ActiveCallRenderBytes {
            limit: MAX_ACTIVE_CALL_RENDER_BYTES,
        });
    }
    ActiveCallOutcome::Ready(Fact::Present(ActiveCall {
        signature,
        params,
        active,
    }))
}

/// Whether a call's argument region holds the offset. The region opens just past the
/// callee (the `(` and beyond) and closes at the parsed extent; a recovered,
/// unterminated call (whose last byte is not `)`) additionally reaches the cursor
/// across trailing whitespace only.
fn region_contains(site: &CallSite, source: &[u8], offset: u32) -> bool {
    let callee_end = site.callee.span().end_byte as u32;
    if offset <= callee_end {
        return false;
    }
    if offset <= site.span.end_byte as u32 {
        return true;
    }
    let end = site.span.end_byte;
    // A terminated call ends in its `)`; the cursor is then past the closed call.
    if end > 0 && source.get(end - 1) == Some(&b')') {
        return false;
    }
    match source.get(end..offset as usize) {
        Some(gap) => gap.iter().all(u8::is_ascii_whitespace),
        None => false,
    }
}

/// The active argument index the offset sits at, or `None` when the callee declares no
/// parameters. A cursor inside an argument's own extent is that argument's slot;
/// otherwise the slot is the count of arguments whose extent ends before the offset —
/// the position the next argument would occupy.
fn active_index(site: &CallSite, offset: u32, param_count: usize) -> Option<u16> {
    if param_count == 0 {
        return None;
    }
    for (index, argument) in site.args.iter().enumerate() {
        if contains(argument.value.span(), offset) {
            return Some(index as u16);
        }
    }
    let index = site
        .args
        .iter()
        .filter(|argument| (argument.value.span().end_byte as u32) < offset)
        .count();
    Some(index as u16)
}

/// Resolve a callee expression to a same-module function or generic template in this
/// file's parse. A qualified (cross-module) name, a built-in, or an unknown name
/// resolves to no local declaration on this floor.
fn resolve_callee<'a>(file: &'a QueryFile<'_>, callee: &Expression) -> Option<&'a FunctionDecl> {
    let Expression::Name { segments, .. } = callee else {
        return None;
    };
    let [name] = &segments[..] else {
        return None;
    };
    file.declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.name == name.text() => Some(function),
            _ => None,
        })
}

/// The callee's canonical signature display and its parameter pieces, rendered from the
/// declared spellings. A generic callee carries its template `<...>` parameters. Each
/// piece composes the signature (`fn name<T>(piece, piece): ret`).
fn render_signature(function: &FunctionDecl) -> (String, Vec<ParamPiece>) {
    let params: Vec<ParamPiece> = function
        .params
        .iter()
        .map(|param| ParamPiece {
            label: format!("{}: {}", param.name, param.ty),
        })
        .collect();
    let type_params = if function.type_params.is_empty() {
        String::new()
    } else {
        let names = function
            .type_params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{names}>")
    };
    let joined = params
        .iter()
        .map(|piece| piece.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let signature = match &function.return_type {
        None => format!("fn {}{type_params}({joined})", function.name),
        Some(return_type) => {
            format!("fn {}{type_params}({joined}): {return_type}", function.name)
        }
    };
    (signature, params)
}

fn collect_declaration_calls<'a>(declaration: &'a Declaration, sink: &mut Vec<CallSite<'a>>) {
    match declaration {
        Declaration::Function(function) => collect_block_calls(&function.body, sink),
        Declaration::Test(test) => collect_block_calls(&test.body, sink),
        Declaration::Const(konst) => {
            if let Some(value) = &konst.value {
                collect_expression_calls(value, sink);
            }
        }
        // No other declaration carries call expressions in value position on this
        // floor: types, resources, stores, and enums are structural.
        Declaration::Alias(_)
        | Declaration::Nominal(_)
        | Declaration::Resource(_)
        | Declaration::Struct(_)
        | Declaration::Store(_)
        | Declaration::Enum(_) => {}
    }
}

fn collect_block_calls<'a>(block: &'a Block, sink: &mut Vec<CallSite<'a>>) {
    for statement in &block.statements {
        collect_statement_calls(statement, sink);
    }
}

fn collect_statement_calls<'a>(statement: &'a Statement, sink: &mut Vec<CallSite<'a>>) {
    match statement {
        Statement::Const { value, .. } => collect_expression_calls(value, sink),
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                collect_expression_calls(value, sink);
            }
        }
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            collect_expression_calls(target, sink);
            collect_expression_calls(value, sink);
        }
        Statement::Delete { path, .. } => collect_expression_calls(path, sink),
        Statement::PlaceBinding { place, .. } | Statement::Unset { place, .. } => {
            collect_expression_calls(place, sink)
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                collect_expression_calls(value, sink);
            }
        }
        Statement::Assert { value, .. } | Statement::Expr { value, .. } => {
            collect_expression_calls(value, sink)
        }
        Statement::Require {
            condition, value, ..
        } => {
            collect_expression_calls(condition, sink);
            collect_expression_calls(value, sink);
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            collect_expression_calls(condition, sink);
            collect_block_calls(then_block, sink);
            for else_if in else_ifs {
                collect_expression_calls(&else_if.condition, sink);
                collect_block_calls(&else_if.block, sink);
            }
            if let Some(else_block) = else_block {
                collect_block_calls(else_block, sink);
            }
        }
        Statement::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            collect_expression_calls(value, sink);
            collect_block_calls(then_block, sink);
            for else_if in else_ifs {
                collect_expression_calls(&else_if.condition, sink);
                collect_block_calls(&else_if.block, sink);
            }
            if let Some(else_block) = else_block {
                collect_block_calls(else_block, sink);
            }
        }
        Statement::IfConstChain {
            bindings,
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            for binding in bindings {
                collect_expression_calls(&binding.value, sink);
            }
            if let Some(condition) = condition {
                collect_expression_calls(condition, sink);
            }
            collect_block_calls(then_block, sink);
            for else_if in else_ifs {
                collect_expression_calls(&else_if.condition, sink);
                collect_block_calls(&else_if.block, sink);
            }
            if let Some(else_block) = else_block {
                collect_block_calls(else_block, sink);
            }
        }
        Statement::LetElse {
            value, else_block, ..
        } => {
            collect_expression_calls(value, sink);
            collect_block_calls(else_block, sink);
        }
        Statement::While {
            condition, body, ..
        } => {
            collect_expression_calls(condition, sink);
            collect_block_calls(body, sink);
        }
        Statement::For {
            iterable,
            step,
            bound,
            body,
            ..
        } => {
            collect_expression_calls(iterable, sink);
            if let Some(step) = step {
                collect_expression_calls(step, sink);
            }
            if let Some(bound) = bound {
                collect_expression_calls(&bound.limit, sink);
                if let Some(from) = &bound.from {
                    collect_expression_calls(from, sink);
                }
                if let Some(on_more) = &bound.on_more {
                    collect_block_calls(on_more, sink);
                }
            }
            collect_block_calls(body, sink);
        }
        Statement::Transaction { body, .. } => collect_block_calls(body, sink),
        Statement::Match {
            scrutinee, arms, ..
        } => {
            collect_expression_calls(scrutinee, sink);
            for arm in arms {
                collect_block_calls(&arm.block, sink);
            }
        }
        Statement::Checked {
            op,
            out_of_range,
            zero_divisor,
            ..
        } => {
            collect_expression_calls(op, sink);
            for block in [out_of_range, zero_divisor].into_iter().flatten() {
                collect_block_calls(block, sink);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
    }
}

/// Collect every call in an expression and its sub-expressions. Unlike the completion
/// module's `expression_children`, this descends into a field receiver and every other
/// compositional form so a call nested anywhere under the expression is reached.
fn collect_expression_calls<'a>(expression: &'a Expression, sink: &mut Vec<CallSite<'a>>) {
    match expression {
        Expression::Call {
            callee, args, span, ..
        } => {
            sink.push(CallSite {
                callee: callee.as_ref(),
                args,
                span: *span,
            });
            collect_expression_calls(callee, sink);
            for argument in args {
                collect_expression_calls(&argument.value, sink);
            }
        }
        Expression::Keyed { base, keys, .. } => {
            collect_expression_calls(base, sink);
            for key in keys {
                collect_expression_calls(key, sink);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            collect_expression_calls(base, sink);
        }
        Expression::Unary { operand, .. } => collect_expression_calls(operand, sink),
        Expression::Binary { operands, .. } => {
            collect_expression_calls(&operands.left, sink);
            collect_expression_calls(&operands.right, sink);
        }
        Expression::Membership { value, range, .. } => {
            collect_expression_calls(value, sink);
            collect_expression_calls(range, sink);
        }
        Expression::Range {
            start, end, step, ..
        } => {
            for part in [start, end, step].into_iter().flatten() {
                collect_expression_calls(part, sink);
            }
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expression) = part {
                    collect_expression_calls(expression, sink);
                }
            }
        }
        Expression::Try { inner, .. } => collect_expression_calls(inner, sink),
        // Leaves carry no sub-expression. The match stays exhaustive so a new
        // child-bearing `Expression` variant is a compile error here rather than a
        // silently unreached nested call.
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. }
        | Expression::Error { .. } => {}
    }
}
