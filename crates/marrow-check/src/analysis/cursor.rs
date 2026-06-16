//! Cursor type and scope queries over a checked program: the read-only surface
//! editor tooling consumes for hover and completion. These walk the parse the
//! pipeline already built, reconstructing the cursor's lexical scope exactly as
//! the checker does, and record no diagnostics.

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::SourceSpan;

use crate::checks::{catch_frame, file_prelude, for_frame};
use crate::enums::resolve_type;
use crate::infer::{bind, infer_type, local_binding};
use crate::walk::for_each_child_expr;
use crate::{CheckedProgram, MarrowType};

/// The type of the expression at byte `offset` in `parsed` (a file of `program`),
/// or `None` when no expression covers the offset. Editor tooling uses this for
/// hover and type-aware actions. It reconstructs the cursor's lexical scope
/// exactly as the checker does — module constants and imports, the enclosing
/// function's parameters, the `const`/`var` bindings that precede the cursor, and
/// any loop or catch binding whose body the cursor sits in — then infers the
/// smallest expression covering the offset. It records no diagnostics.
pub fn type_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Option<MarrowType> {
    let prelude = file_prelude(program, file, parsed);
    let function = enclosing_function(parsed, offset)?;
    let mut scope = function_base_scope(
        program,
        function,
        &prelude.module_constants,
        &prelude.aliases,
        file,
    );
    walk_block_to_offset(
        program,
        &function.body,
        offset,
        &prelude.aliases,
        file,
        &mut scope,
    );
    let expr = smallest_expression_at(&function.body, offset)?;
    Some(infer_type(
        program,
        expr,
        &scope,
        &prelude.aliases,
        file,
        &mut Vec::new(),
    ))
}

/// The bindings visible at byte `offset` in `parsed` (a file of `program`), as
/// `(name, type)` pairs, for completion. The reconstructed scope is the same one
/// [`type_at`] infers against: module constants and imports, then — when the
/// offset is inside a function — that function's parameters, the `const`/`var`
/// locals declared before the cursor, and any loop or catch binding in scope.
/// Import aliases are surfaced with [`MarrowType::Unknown`] (they name modules,
/// not values). Inner bindings shadow outer ones. It records no diagnostics.
pub fn scope_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Vec<(String, MarrowType)> {
    let prelude = file_prelude(program, file, parsed);
    // Imports and module constants are the outermost frame; a later frame's
    // binding shadows them. Imports name modules, so they carry no value type.
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![
        prelude
            .aliases
            .keys()
            .map(|alias| (alias.clone(), MarrowType::Unknown))
            .collect(),
        prelude.module_constants.clone(),
    ];
    if let Some(function) = enclosing_function(parsed, offset) {
        scope.extend(function_base_scope(
            program,
            function,
            &prelude.module_constants,
            &prelude.aliases,
            file,
        ));
        walk_block_to_offset(
            program,
            &function.body,
            offset,
            &prelude.aliases,
            file,
            &mut scope,
        );
    }
    // Flatten outermost-first so an inner binding overwrites a shadowed outer one,
    // leaving each visible name once with the type that actually applies.
    let mut visible: HashMap<String, MarrowType> = HashMap::new();
    for frame in scope {
        visible.extend(frame);
    }
    let mut bindings: Vec<(String, MarrowType)> = visible.into_iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    bindings
}

/// The function declaration whose body span covers `offset`, if any. A cursor in a
/// function signature or at module level has no enclosing body and yields `None`.
fn enclosing_function(
    parsed: &marrow_syntax::ParsedSource,
    offset: usize,
) -> Option<&marrow_syntax::FunctionDecl> {
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            marrow_syntax::Declaration::Function(function)
                if span_covers(function.body.span, offset) =>
            {
                Some(function)
            }
            _ => None,
        })
}

/// The base scope frame for a function body: the module's constants overlaid with
/// the parameter list, mirroring [`check_function_types`] (a parameter shadows a
/// like-named constant).
fn function_base_scope(
    program: &CheckedProgram,
    function: &marrow_syntax::FunctionDecl,
    module_constants: &HashMap<String, MarrowType>,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Vec<HashMap<String, MarrowType>> {
    let mut base = module_constants.clone();
    for param in &function.params {
        base.insert(
            param.name.clone(),
            resolve_type(&param.ty, program, aliases, file),
        );
    }
    vec![base]
}

/// Replay the binding behavior of [`check_block_types`]/[`check_statement_types`]
/// up to `offset`: push a frame for `block`, record each `const`/`var` binding the
/// block introduces before the cursor, and descend into the one nested block (and
/// its loop or catch frame) that covers the cursor. Statements after the cursor
/// are not visible and are skipped. This shares the checker's binding primitives
/// (`local_binding`, the loop/catch frames) so the reconstructed scope cannot
/// drift from the one the checker builds.
fn walk_block_to_offset(
    program: &CheckedProgram,
    block: &marrow_syntax::Block,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
) {
    scope.push(HashMap::new());
    for statement in &block.statements {
        // A binding declared at or after the cursor is not yet in scope. Compared
        // against the statement's start so the cursor on a `const`'s own line does
        // not see that `const` (its initializer cannot reference itself).
        if statement.span().start_byte >= offset {
            break;
        }
        // Record the binding this statement introduces, exactly as the checker
        // does, before deciding whether to descend into it.
        if let Some((name, ty)) = local_binding(program, statement, scope, aliases, file) {
            bind(scope, &name, ty);
        }
        // Descend into the nested block (and its loop/catch frame) that the cursor
        // sits in. Only one statement can cover the cursor, so the walk stops here.
        if span_covers(statement.span(), offset)
            && let Some(body) = descend_target(program, statement, offset, aliases, file, scope)
        {
            walk_block_to_offset(program, body, offset, aliases, file, scope);
            return;
        }
    }
}

/// The nested block of `statement` that covers `offset`, pushing the loop or catch
/// frame that block runs under (a `for` binding, a `catch` error value) onto
/// `scope` first, mirroring [`check_statement_types`]. Returns `None` when the
/// cursor is in the statement but not in one of its bodies (for example in an `if`
/// condition), leaving `scope` untouched.
fn descend_target<'b>(
    program: &CheckedProgram,
    statement: &'b marrow_syntax::Statement,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    scope: &mut Vec<HashMap<String, MarrowType>>,
) -> Option<&'b marrow_syntax::Block> {
    use marrow_syntax::Statement;
    match statement {
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if span_covers(then_block.span, offset) {
                return Some(then_block);
            }
            for else_if in else_ifs {
                if span_covers(else_if.block.span, offset) {
                    return Some(&else_if.block);
                }
            }
            else_block
                .as_ref()
                .filter(|block| span_covers(block.span, offset))
        }
        Statement::While { body, .. } | Statement::Transaction { body, .. } => {
            span_covers(body.span, offset).then_some(body)
        }
        Statement::For {
            binding,
            iterable,
            body,
            ..
        } => {
            if !span_covers(body.span, offset) {
                return None;
            }
            let frame = for_frame(program, binding, iterable, scope, aliases, file);
            scope.push(frame);
            Some(body)
        }
        Statement::Try { body, catch, .. } => {
            if span_covers(body.span, offset) {
                return Some(body);
            }
            if let Some(clause) = catch
                && span_covers(clause.block.span, offset)
            {
                scope.push(catch_frame(clause));
                return Some(&clause.block);
            }
            None
        }
        _ => None,
    }
}

/// Whether `span` covers `offset`, inclusive of the end byte so a cursor at the
/// closing edge of an expression still resolves.
pub(crate) fn span_covers(span: SourceSpan, offset: usize) -> bool {
    span.start_byte <= offset && offset <= span.end_byte
}

/// The smallest expression in a function `body` whose span covers `offset`, the
/// expression the cursor sits on. Walks every expression the type pass would
/// visit, keeping the tightest span. Statement-level structure (conditions,
/// initializers, call arguments, nested blocks) is traversed so the cursor lands
/// on the leaf expression rather than an enclosing one.
fn smallest_expression_at(
    body: &marrow_syntax::Block,
    offset: usize,
) -> Option<&marrow_syntax::Expression> {
    let mut best: Option<&marrow_syntax::Expression> = None;
    collect_block_expression(body, offset, &mut best);
    best
}

fn collect_block_expression<'b>(
    block: &'b marrow_syntax::Block,
    offset: usize,
    best: &mut Option<&'b marrow_syntax::Expression>,
) {
    use marrow_syntax::Statement;
    for statement in &block.statements {
        match statement {
            Statement::Const { value, .. } | Statement::Throw { value, .. } => {
                collect_expression(value, offset, best);
            }
            Statement::Expr { value, .. } => collect_expression(value, offset, best),
            Statement::Var { value, .. } => {
                if let Some(value) = value {
                    collect_expression(value, offset, best);
                }
            }
            Statement::Assign { target, value, .. } => {
                collect_expression(target, offset, best);
                collect_expression(value, offset, best);
            }
            Statement::Delete { path, .. } => collect_expression(path, offset, best),
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    collect_expression(value, offset, best);
                }
            }
            Statement::ReturnAbsent { .. } => {}
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(condition) = condition {
                    collect_expression(condition, offset, best);
                }
                collect_block_expression(then_block, offset, best);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        collect_expression(condition, offset, best);
                    }
                    collect_block_expression(&else_if.block, offset, best);
                }
                if let Some(block) = else_block {
                    collect_block_expression(block, offset, best);
                }
            }
            Statement::IfConst {
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_expression(value, offset, best);
                collect_block_expression(then_block, offset, best);
                for else_if in else_ifs {
                    if let Some(condition) = &else_if.condition {
                        collect_expression(condition, offset, best);
                    }
                    collect_block_expression(&else_if.block, offset, best);
                }
                if let Some(block) = else_block {
                    collect_block_expression(block, offset, best);
                }
            }
            Statement::While {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_expression(condition, offset, best);
                }
                collect_block_expression(body, offset, best);
            }
            Statement::For { iterable, body, .. } => {
                collect_expression(iterable, offset, best);
                collect_block_expression(body, offset, best);
            }
            Statement::Transaction { body, .. } => collect_block_expression(body, offset, best),
            Statement::Try { body, catch, .. } => {
                collect_block_expression(body, offset, best);
                if let Some(clause) = catch {
                    collect_block_expression(&clause.block, offset, best);
                }
            }
            Statement::Match {
                scrutinee, arms, ..
            } => {
                if let Some(scrutinee) = scrutinee {
                    collect_expression(scrutinee, offset, best);
                }
                for arm in arms {
                    collect_block_expression(&arm.block, offset, best);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } => {}
        }
    }
}

/// Keep `expr` as the best match when its span covers `offset` and is no wider
/// than the current best, then recurse into its subexpressions so the tightest
/// covering leaf wins.
fn collect_expression<'e>(
    expr: &'e marrow_syntax::Expression,
    offset: usize,
    best: &mut Option<&'e marrow_syntax::Expression>,
) {
    let span = expr.span();
    if !span_covers(span, offset) {
        return;
    }
    let width = span.end_byte.saturating_sub(span.start_byte);
    let replace = best.is_none_or(|current| {
        let current = current.span();
        width <= current.end_byte.saturating_sub(current.start_byte)
    });
    if replace {
        *best = Some(expr);
    }
    for_each_child_expr(expr, |child| collect_expression(child, offset, best));
}
