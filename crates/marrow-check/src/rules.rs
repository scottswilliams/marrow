//! Structural rules over the parsed tree.
//!
//! These checks read only the parsed syntax tree: control flow that escapes a
//! `finally` block, `catch` type annotations, assignment targets, and `const`
//! values that are not constant expressions. They do not need type or effect
//! facts, so they run directly on each declaration.

use std::collections::HashSet;
use std::path::Path;

use marrow_schema::Type;
use marrow_syntax::{
    Block, CatchClause, Expression, FunctionDecl, InterpolationPart, Severity, Statement,
    format_expression,
};

use crate::checks::check_range_value;
use crate::typerules::check_literal_range;
use crate::walk::for_each_child_expr;
use crate::{CHECK_TRY_HANDLER, CheckDiagnostic, DiagnosticPayload};

/// A `finally` block must not let control flow escape it via `return`, `break`,
/// or `continue`.
pub const CHECK_FINALLY_CONTROL_FLOW: &str = "check.finally_control_flow";
/// A `break`/`continue` is outside any loop, or its label names no enclosing
/// loop, so it could never resolve at runtime.
pub const CHECK_LOOP_CONTROL_FLOW: &str = "check.loop_control_flow";
/// A `catch` annotation must be `Error`.
pub const CHECK_CATCH_TYPE: &str = "check.catch_type";
/// An assignment target is not a writable place.
pub const CHECK_INVALID_ASSIGN_TARGET: &str = "check.invalid_assign_target";
/// A `const` value is not a constant expression.
pub const CHECK_NON_CONSTANT_CONST: &str = "check.non_constant_const";
/// A loop over a saved layer mutates that same layer. Collect the keys into a
/// local sequence first when a rewrite must change the traversed layer.
pub const CHECK_LOOP_MUTATES_TRAVERSED_LAYER: &str = "check.loop_mutates_traversed_layer";

/// Apply every structural statement rule to one function body.
pub(crate) fn check_function_body(
    file: &Path,
    function: &FunctionDecl,
    out: &mut Vec<CheckDiagnostic>,
) {
    let read_only_params: HashSet<String> = function
        .params
        .iter()
        .filter(|param| param.mode.is_none())
        .map(|param| param.name.clone())
        .collect();
    walk_block(
        file,
        &function.body,
        &read_only_params,
        &HashSet::new(),
        out,
    );
    walk_loop_control_flow(file, &function.body, 0, &mut Vec::new(), out);
    walk_loop_layer_mutations(file, &function.body, &mut Vec::new(), out);
}

/// Apply the structural body rules to an `evolve transform` block, which has no
/// function parameters and so no read-only bindings.
pub(crate) fn check_transform_body(file: &Path, body: &Block, out: &mut Vec<CheckDiagnostic>) {
    walk_block(file, body, &HashSet::new(), &HashSet::new(), out);
    walk_loop_control_flow(file, body, 0, &mut Vec::new(), out);
    walk_loop_layer_mutations(file, body, &mut Vec::new(), out);
}

/// A `const` value must be a compile-time constant expression: literals and
/// other constants combined with operators, never a host call or saved-data
/// read.
pub(crate) fn check_const_value(file: &Path, value: &Expression, out: &mut Vec<CheckDiagnostic>) {
    if !is_constant_expr(value) {
        out.push(diagnostic(
            CHECK_NON_CONSTANT_CONST,
            file,
            value,
            "a `const` value must be a constant expression, not a host call or saved-data read",
        ));
    }
    check_literal_ranges(file, value, out);
    check_range_value(file, value, out);
}

/// Range-check every literal in a `const` value, mirroring the constant-expression
/// shape walked by `is_constant_expr`.
fn check_literal_ranges(file: &Path, expr: &Expression, out: &mut Vec<CheckDiagnostic>) {
    match expr {
        Expression::Literal { kind, text, span } => {
            check_literal_range(*kind, text, *span, file, out);
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            check_literal_ranges(file, base, out)
        }
        Expression::Unary { operand, .. } => check_literal_ranges(file, operand, out),
        Expression::Binary { left, right, .. } => {
            check_literal_ranges(file, left, out);
            check_literal_ranges(file, right, out);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    check_literal_ranges(file, expr, out);
                }
            }
        }
        Expression::Name { .. } | Expression::SavedRoot { .. } | Expression::Call { .. } => {}
    }
}

/// Walk a block applying the catch and assign-target rules to each statement,
/// recursing into nested blocks. A `try`'s `finally` block also gets the finally walk.
fn walk_block(
    file: &Path,
    block: &Block,
    read_only_params: &HashSet<String>,
    inherited_local_collections: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    let mut read_only_params = read_only_params.clone();
    let mut local_collections = inherited_local_collections.clone();
    for statement in &block.statements {
        walk_statement(file, statement, &read_only_params, &local_collections, out);
        if let Some(name) = statement_binding_name(statement) {
            read_only_params.remove(name);
            local_collections.remove(name);
        }
        if let Some(name) = local_collection_binding_name(statement) {
            local_collections.insert(name);
        }
    }
}

fn walk_statement(
    file: &Path,
    statement: &Statement,
    read_only_params: &HashSet<String>,
    local_collections: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    check_statement_inout_argument_targets(file, statement, read_only_params, out);
    match statement {
        Statement::Assign { target, .. } => {
            check_assignment_target(file, target, read_only_params, local_collections, out);
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_block(file, then_block, read_only_params, local_collections, out);
            for else_if in else_ifs {
                walk_block(
                    file,
                    &else_if.block,
                    read_only_params,
                    local_collections,
                    out,
                );
            }
            if let Some(block) = else_block {
                walk_block(file, block, read_only_params, local_collections, out);
            }
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => {
            walk_block(file, body, read_only_params, local_collections, out)
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            if catch.is_none() && finally.is_none() {
                out.push(diagnostic_at(
                    CHECK_TRY_HANDLER,
                    file,
                    statement,
                    "a `try` block must have a `catch` or `finally` clause",
                ));
            }
            walk_block(file, body, read_only_params, local_collections, out);
            if let Some(catch) = catch {
                check_catch(file, catch, out);
                walk_block(file, &catch.block, read_only_params, local_collections, out);
            }
            if let Some(finally) = finally {
                walk_block(file, finally, read_only_params, local_collections, out);
                walk_finally(file, finally, 0, &mut Vec::new(), out);
            }
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                walk_block(file, &arm.block, read_only_params, local_collections, out);
            }
        }
        Statement::Const { .. }
        | Statement::Var { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. } => {}
    }
}

fn check_statement_inout_argument_targets(
    file: &Path,
    statement: &Statement,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        Statement::Const { value, .. }
        | Statement::Throw { value, .. }
        | Statement::Expr { value, .. } => {
            check_expr_inout_argument_targets(file, value, read_only_params, out);
        }
        Statement::Assign { target, value, .. } => {
            check_expr_inout_argument_targets(file, target, read_only_params, out);
            check_expr_inout_argument_targets(file, value, read_only_params, out);
        }
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                check_expr_inout_argument_targets(file, value, read_only_params, out);
            }
        }
        Statement::Delete { path, .. } => {
            check_expr_inout_argument_targets(file, path, read_only_params, out);
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                check_expr_inout_argument_targets(file, value, read_only_params, out);
            }
        }
        Statement::If {
            condition,
            else_ifs,
            ..
        } => {
            if let Some(condition) = condition {
                check_expr_inout_argument_targets(file, condition, read_only_params, out);
            }
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    check_expr_inout_argument_targets(file, condition, read_only_params, out);
                }
            }
        }
        Statement::While { condition, .. } => {
            if let Some(condition) = condition {
                check_expr_inout_argument_targets(file, condition, read_only_params, out);
            }
        }
        Statement::For { iterable, step, .. } => {
            check_expr_inout_argument_targets(file, iterable, read_only_params, out);
            if let Some(step) = step {
                check_expr_inout_argument_targets(file, step, read_only_params, out);
            }
        }
        Statement::Match { scrutinee, .. } => {
            if let Some(scrutinee) = scrutinee {
                check_expr_inout_argument_targets(file, scrutinee, read_only_params, out);
            }
        }
        Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Transaction { .. }
        | Statement::Try { .. } => {}
    }
}

fn check_expr_inout_argument_targets(
    file: &Path,
    expr: &Expression,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    // A moded argument's read-only check must land in source position: the callee
    // precedes the arguments in source, so its subtree is walked first, then each
    // argument is checked and recursed in turn. Non-call expressions carry no moded
    // arguments and defer their child shape to the shared visitor.
    if let Expression::Call { callee, args, .. } = expr {
        check_expr_inout_argument_targets(file, callee, read_only_params, out);
        for arg in args {
            if arg.mode.is_some() {
                check_read_only_inout_argument(file, &arg.value, read_only_params, out);
            }
            check_expr_inout_argument_targets(file, &arg.value, read_only_params, out);
        }
    } else {
        for_each_child_expr(expr, |child| {
            check_expr_inout_argument_targets(file, child, read_only_params, out)
        });
    }
}

fn check_read_only_inout_argument(
    file: &Path,
    value: &Expression,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    if is_assignable(value)
        && let Some(name) = place_root_name(value)
        && read_only_params.contains(name)
    {
        out.push(diagnostic(
            CHECK_INVALID_ASSIGN_TARGET,
            file,
            value,
            &format!("parameter `{name}` is read-only"),
        ));
    }
}

fn check_assignment_target(
    file: &Path,
    target: &Expression,
    read_only_params: &HashSet<String>,
    local_collections: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    if !is_assignable(target) && !is_local_collection_lookup(target, local_collections) {
        out.push(diagnostic(
            CHECK_INVALID_ASSIGN_TARGET,
            file,
            target,
            "assignment target is not a writable place",
        ));
        return;
    }

    if let Some(name) = place_root_name(target)
        && read_only_params.contains(name)
    {
        out.push(diagnostic(
            CHECK_INVALID_ASSIGN_TARGET,
            file,
            target,
            &format!("parameter `{name}` is read-only"),
        ));
    }
}

fn local_collection_binding_name(statement: &Statement) -> Option<String> {
    let Statement::Var { name, keys, ty, .. } = statement else {
        return None;
    };
    if !keys.is_empty()
        || ty
            .as_ref()
            .is_some_and(|ty| matches!(Type::resolve(ty), Type::Sequence(_)))
    {
        Some(name.clone())
    } else {
        None
    }
}

fn is_local_collection_lookup(target: &Expression, local_collections: &HashSet<String>) -> bool {
    let Expression::Call { callee, .. } = target else {
        return false;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return false;
    };
    let [name] = segments.as_slice() else {
        return false;
    };
    local_collections.contains(name)
}

fn statement_binding_name(statement: &Statement) -> Option<&str> {
    match statement {
        Statement::Const { name, .. } | Statement::Var { name, .. } => Some(name),
        Statement::Assign { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. }
        | Statement::If { .. }
        | Statement::While { .. }
        | Statement::For { .. }
        | Statement::Transaction { .. }
        | Statement::Try { .. }
        | Statement::Match { .. } => None,
    }
}

/// A `catch` annotation, if present, must name `Error`. A bare catch is fine.
fn check_catch(file: &Path, catch: &CatchClause, out: &mut Vec<CheckDiagnostic>) {
    if let Some(ty) = &catch.ty
        && ty.text != "Error"
    {
        out.push(CheckDiagnostic {
            code: CHECK_CATCH_TYPE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("catch type must be `Error`, found `{}`", ty.text),
            span: catch.block.span,
            payload: DiagnosticPayload::None,
        });
    }
}

/// Walk a `finally` block reporting control flow that escapes it.
///
/// `return` always escapes. An unlabeled `break`/`continue` escapes only when no
/// loop encloses it within the finally (`loop_depth == 0`). A labeled
/// `break`/`continue` escapes unless its label names a loop introduced within
/// the finally block (`loop_labels`). A nested `try`'s own `finally` is a fresh
/// scope and is not walked here.
fn walk_finally(
    file: &Path,
    block: &Block,
    loop_depth: usize,
    loop_labels: &mut Vec<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        match statement {
            Statement::Return { .. } => out.push(diagnostic_at(
                CHECK_FINALLY_CONTROL_FLOW,
                file,
                statement,
                "`return` is not allowed in a `finally` block",
            )),
            Statement::Break { label, .. } | Statement::Continue { label, .. }
                if !jump_resolves_in_scope(label.as_deref(), loop_depth, loop_labels) =>
            {
                out.push(diagnostic_at(
                    CHECK_FINALLY_CONTROL_FLOW,
                    file,
                    statement,
                    "control flow may not leave a `finally` block",
                ));
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_finally(file, then_block, loop_depth, loop_labels, out);
                for else_if in else_ifs {
                    walk_finally(file, &else_if.block, loop_depth, loop_labels, out);
                }
                if let Some(block) = else_block {
                    walk_finally(file, block, loop_depth, loop_labels, out);
                }
            }
            Statement::While { label, body, .. } | Statement::For { label, body, .. } => {
                let pushed = label.clone();
                if let Some(label) = &pushed {
                    loop_labels.push(label.clone());
                }
                walk_finally(file, body, loop_depth + 1, loop_labels, out);
                if pushed.is_some() {
                    loop_labels.pop();
                }
            }
            Statement::Transaction { body, .. } => {
                walk_finally(file, body, loop_depth, loop_labels, out);
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                // The nested body and catch still sit inside this finally, so
                // their escaping jumps are also illegal. The nested finally is a
                // fresh scope handled by the ordinary walk.
                walk_finally(file, body, loop_depth, loop_labels, out);
                if let Some(catch) = catch {
                    walk_finally(file, &catch.block, loop_depth, loop_labels, out);
                }
                if let Some(finally) = finally {
                    walk_finally(file, finally, 0, &mut Vec::new(), out);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_finally(file, &arm.block, loop_depth, loop_labels, out);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::Delete { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. } => {}
        }
    }
}

/// Whether a `break`/`continue` resolves to a loop in scope. An unlabeled jump
/// resolves when some enclosing loop is in scope (`loop_depth > 0`); a labeled jump
/// resolves when its label names one of the enclosing loops (`loop_labels`). Both
/// the in-scope loop rule and the finally-escape rule are this one question: a jump
/// escapes a `finally` exactly when it does not resolve to a loop inside it.
fn jump_resolves_in_scope(label: Option<&str>, loop_depth: usize, loop_labels: &[String]) -> bool {
    match label {
        None => loop_depth > 0,
        Some(label) => loop_labels.iter().any(|known| known == label),
    }
}

/// Walk a block reporting a `break`/`continue` that resolves to no enclosing loop,
/// which the runtime would otherwise only catch late with `run.no_enclosing_loop`.
/// Descends into `finally` blocks with the surrounding loop context, since their
/// jumps still sit inside the function's loop nesting.
fn walk_loop_control_flow(
    file: &Path,
    block: &Block,
    loop_depth: usize,
    loop_labels: &mut Vec<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        match statement {
            Statement::Break { label, .. } | Statement::Continue { label, .. }
                if !jump_resolves_in_scope(label.as_deref(), loop_depth, loop_labels) =>
            {
                let message = match label {
                    Some(label) => {
                        format!("`{label}` names no enclosing loop")
                    }
                    None => "control flow statement is not inside a loop".to_string(),
                };
                out.push(diagnostic_at(
                    CHECK_LOOP_CONTROL_FLOW,
                    file,
                    statement,
                    &message,
                ));
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_loop_control_flow(file, then_block, loop_depth, loop_labels, out);
                for else_if in else_ifs {
                    walk_loop_control_flow(file, &else_if.block, loop_depth, loop_labels, out);
                }
                if let Some(block) = else_block {
                    walk_loop_control_flow(file, block, loop_depth, loop_labels, out);
                }
            }
            Statement::While { label, body, .. } | Statement::For { label, body, .. } => {
                if let Some(label) = label {
                    loop_labels.push(label.clone());
                }
                walk_loop_control_flow(file, body, loop_depth + 1, loop_labels, out);
                if label.is_some() {
                    loop_labels.pop();
                }
            }
            Statement::Transaction { body, .. } => {
                walk_loop_control_flow(file, body, loop_depth, loop_labels, out);
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                walk_loop_control_flow(file, body, loop_depth, loop_labels, out);
                if let Some(catch) = catch {
                    walk_loop_control_flow(file, &catch.block, loop_depth, loop_labels, out);
                }
                if let Some(finally) = finally {
                    walk_loop_control_flow(file, finally, loop_depth, loop_labels, out);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_loop_control_flow(file, &arm.block, loop_depth, loop_labels, out);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::Delete { .. }
            | Statement::Return { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. } => {}
        }
    }
}

/// Walk a block reporting a write, delete, or append that mutates the same saved
/// layer an enclosing `for` loop is traversing, forbidden because mutating a tree
/// layer while iterating it has undefined ordering. `traversed` holds the canonical
/// text of each enclosing loop's traversed saved layer.
fn walk_loop_layer_mutations(
    file: &Path,
    block: &Block,
    traversed: &mut Vec<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        if let Some(affected) = mutated_layer(statement)
            && traversed.iter().any(|layer| layer == &affected)
        {
            out.push(diagnostic_at(
                CHECK_LOOP_MUTATES_TRAVERSED_LAYER,
                file,
                statement,
                "this write changes the saved layer the enclosing loop is traversing; \
                 collect the keys into a local sequence first",
            ));
        }
        match statement {
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_loop_layer_mutations(file, then_block, traversed, out);
                for else_if in else_ifs {
                    walk_loop_layer_mutations(file, &else_if.block, traversed, out);
                }
                if let Some(block) = else_block {
                    walk_loop_layer_mutations(file, block, traversed, out);
                }
            }
            Statement::For { iterable, body, .. } => {
                let pushed = traversed_layer(iterable);
                if let Some(layer) = &pushed {
                    traversed.push(layer.clone());
                }
                walk_loop_layer_mutations(file, body, traversed, out);
                if pushed.is_some() {
                    traversed.pop();
                }
            }
            Statement::While { body, .. } | Statement::Transaction { body, .. } => {
                walk_loop_layer_mutations(file, body, traversed, out);
            }
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => {
                walk_loop_layer_mutations(file, body, traversed, out);
                if let Some(catch) = catch {
                    walk_loop_layer_mutations(file, &catch.block, traversed, out);
                }
                if let Some(finally) = finally {
                    walk_loop_layer_mutations(file, finally, traversed, out);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_loop_layer_mutations(file, &arm.block, traversed, out);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::Delete { .. }
            | Statement::Return { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. } => {}
        }
    }
}

/// The saved layer a `for` loop traverses, as canonical text, or `None` for a loop
/// over a range or a local value (the "collect keys first" pattern). Collection-view
/// wrappers such as `keys`/`values`/`entries`/`reversed` are peeled first.
fn traversed_layer(iterable: &Expression) -> Option<String> {
    let path = traversal_path(iterable);
    is_saved_path(path).then(|| format_expression(path))
}

/// Peel traversal-preserving wrappers until the saved layer expression remains.
fn traversal_path(expr: &Expression) -> &Expression {
    match traversal_argument(expr) {
        Some(inner) => traversal_path(inner),
        None => expr,
    }
}

/// The sole argument of a `keys`/`values`/`entries`/`reversed` call, or `None` for
/// any other expression. These wrap a saved layer without changing which layer is
/// traversed.
fn traversal_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1
        || !matches!(
            segments[0].as_str(),
            "keys" | "values" | "entries" | "reversed"
        )
    {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => {
            Some(traversal_argument(&arg.value).unwrap_or(&arg.value))
        }
        _ => None,
    }
}

/// The saved layer a statement adds keys to or removes keys from, as canonical
/// text, or `None` when the statement does not change a saved layer's key set. A
/// whole-record/keyed-entry write or `delete` of `^root(key…)` affects the parent
/// layer (the callee). `append(path, v)` affects the named layer. A scalar field
/// write or field delete keeps the layer's keys, so it is not reported here.
fn mutated_layer(statement: &Statement) -> Option<String> {
    match statement {
        Statement::Assign { target, .. } => keyed_entry_parent(target),
        Statement::Delete { path, .. } => keyed_entry_parent(path),
        Statement::Expr {
            value: Expression::Call { callee, args, .. },
            ..
        } => append_target(callee, args).map(format_expression),
        Statement::Expr { .. }
        | Statement::Const { .. }
        | Statement::Var { .. }
        | Statement::Return { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::If { .. }
        | Statement::While { .. }
        | Statement::For { .. }
        | Statement::Transaction { .. }
        | Statement::Try { .. }
        | Statement::Match { .. } => None,
    }
}

/// The layer that gains or loses a key when `target` (a `^root(key…)` or
/// `^root(key…).layer(key…)` place) is written or deleted whole: the
/// callee with the final key step dropped. A scalar field place
/// (`^root(key…).field`) is not a keyed entry, so it returns `None` — writing a
/// field does not change the parent layer's keys.
fn keyed_entry_parent(target: &Expression) -> Option<String> {
    match target {
        Expression::Call { callee, .. } if is_saved_path(callee) => Some(format_expression(callee)),
        _ => None,
    }
}

/// The saved layer argument of `append(path, value)`, or `None` for any other
/// call. `append` adds a key to its first argument's layer.
fn append_target<'a>(
    callee: &Expression,
    args: &'a [marrow_syntax::Argument],
) -> Option<&'a Expression> {
    let Expression::Name { segments, .. } = callee else {
        return None;
    };
    if segments.len() != 1 || segments[0] != "append" {
        return None;
    }
    match args {
        [path, _] if path.mode.is_none() && path.name.is_none() && is_saved_path(&path.value) => {
            Some(&path.value)
        }
        _ => None,
    }
}

/// A saved-data path: a `^root`, a key lookup on a saved path, or a field of one.
fn is_saved_path(expr: &Expression) -> bool {
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_saved_path(base),
        Expression::Call { callee, .. } => is_saved_path(callee),
        _ => false,
    }
}

/// A writable place: a bare name, a saved root, a field of a place, or a key
/// lookup on a saved place. Local collection lookups are scope-sensitive and are
/// handled by `check_assignment_target`.
pub(crate) fn is_assignable(target: &Expression) -> bool {
    match target {
        Expression::Name { segments, .. } => segments.len() == 1,
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_assignable(base),
        Expression::Call { callee, .. } => is_key_lookup_target(callee),
        _ => false,
    }
}

fn place_root_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::Name { segments, .. } if segments.len() == 1 => Some(&segments[0]),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            place_root_name(base)
        }
        Expression::Call { callee, .. } => place_root_name(callee),
        _ => None,
    }
}

/// The callee of a key-lookup place: a saved root, a field of a place, or a
/// further key lookup.
fn is_key_lookup_target(callee: &Expression) -> bool {
    match callee {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_assignable(base),
        Expression::Call { callee, .. } => is_key_lookup_target(callee),
        _ => false,
    }
}

/// A constant expression: a literal or a name (another constant) combined with
/// field access, operators, or interpolation. A saved-data read or a call is
/// never constant, so neither is any expression containing one. Text the parser
/// did not structure is treated as constant to avoid false positives.
fn is_constant_expr(expr: &Expression) -> bool {
    match expr {
        Expression::Literal { .. } | Expression::Name { .. } => true,
        // `?.` is a possibly-absent read, never a compile-time constant.
        Expression::SavedRoot { .. }
        | Expression::Call { .. }
        | Expression::OptionalField { .. } => false,
        Expression::Field { base, .. } => is_constant_expr(base),
        Expression::Unary { operand, .. } => is_constant_expr(operand),
        Expression::Binary { left, right, .. } => is_constant_expr(left) && is_constant_expr(right),
        Expression::Interpolation { parts, .. } => parts.iter().all(|part| match part {
            InterpolationPart::Text { .. } => true,
            InterpolationPart::Expr(expr) => is_constant_expr(expr),
        }),
    }
}

fn diagnostic(
    code: &'static str,
    file: &Path,
    expr: &Expression,
    message: &str,
) -> CheckDiagnostic {
    let span = expr.span();
    CheckDiagnostic {
        code,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: message.to_string(),
        span,
        payload: DiagnosticPayload::None,
    }
}

fn diagnostic_at(
    code: &'static str,
    file: &Path,
    statement: &Statement,
    message: &str,
) -> CheckDiagnostic {
    let span = statement.span();
    CheckDiagnostic {
        code,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: message.to_string(),
        span,
        payload: DiagnosticPayload::None,
    }
}
