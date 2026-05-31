//! Structural rules over the parsed tree.
//!
//! These checks read only the parsed syntax tree: control flow that escapes a
//! `finally` block, `catch` type annotations, assignment targets, and `const`
//! values that are not constant expressions. They do not need type or effect
//! facts, so they run directly on each declaration.

use std::collections::HashSet;
use std::path::Path;

use marrow_syntax::{
    BinaryOp, Block, CatchClause, Expression, FunctionDecl, InterpolationPart, ParamMode, Severity,
    SourceSpan, Statement, format_expression,
};

use crate::CheckDiagnostic;

/// A `finally` block must not let control flow escape it via `return`, `break`,
/// or `continue`.
pub const CHECK_FINALLY_CONTROL_FLOW: &str = "check.finally_control_flow";
/// A `break`/`continue` is outside any loop, or its label names no enclosing
/// loop, so it could never resolve at runtime.
pub const CHECK_LOOP_CONTROL_FLOW: &str = "check.loop_control_flow";
/// A `catch` annotation must be `Error`.
pub const CHECK_CATCH_TYPE: &str = "check.catch_type";
/// An assignment or merge target is not a writable place.
pub const CHECK_INVALID_ASSIGN_TARGET: &str = "check.invalid_assign_target";
/// An `out` parameter can return normally without being assigned.
pub const CHECK_OUT_PARAMETER_ASSIGNMENT: &str = "check.out_parameter_assignment";
/// A `const` value is not a constant expression.
pub const CHECK_NON_CONSTANT_CONST: &str = "check.non_constant_const";
/// A loop over a saved layer mutates that same layer (a direct write, delete,
/// append, or merge whose target is the layer being traversed). Collect the keys
/// into a local sequence first when a rewrite must change the traversed layer.
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
    walk_block(file, &function.body, &read_only_params, out);
    check_out_parameters_assigned(file, function, out);
    walk_loop_control_flow(file, &function.body, 0, &mut Vec::new(), out);
    walk_loop_layer_mutations(file, &function.body, &mut Vec::new(), out);
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
}

/// Range-check every literal inside a `const` value so an out-of-range integer or
/// decimal literal is reported at check time, not only at runtime. Mirrors the
/// constant-expression shape walked by `is_constant_expr`.
fn check_literal_ranges(file: &Path, expr: &Expression, out: &mut Vec<CheckDiagnostic>) {
    match expr {
        Expression::Literal { kind, text, span } => {
            crate::check_literal_range(*kind, text, *span, file, out);
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
/// recursing into nested blocks. A `try`'s `finally` block is handed to the
/// dedicated finally walk.
fn walk_block(
    file: &Path,
    block: &Block,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    let mut read_only_params = read_only_params.clone();
    for statement in &block.statements {
        walk_statement(file, statement, &read_only_params, out);
        if let Some(name) = statement_binding_name(statement) {
            read_only_params.remove(name);
        }
    }
}

fn walk_statement(
    file: &Path,
    statement: &Statement,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    check_statement_out_argument_targets(file, statement, read_only_params, out);
    match statement {
        Statement::Assign { target, .. } | Statement::Merge { target, .. } => {
            check_assignment_target(file, target, read_only_params, out);
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_block(file, then_block, read_only_params, out);
            for else_if in else_ifs {
                walk_block(file, &else_if.block, read_only_params, out);
            }
            if let Some(block) = else_block {
                walk_block(file, block, read_only_params, out);
            }
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. }
        | Statement::Lock { body, .. } => walk_block(file, body, read_only_params, out),
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            walk_block(file, body, read_only_params, out);
            if let Some(catch) = catch {
                check_catch(file, catch, out);
                walk_block(file, &catch.block, read_only_params, out);
            }
            if let Some(finally) = finally {
                // The finally block is also an ordinary block for the other
                // rules, plus the escaping-control-flow rule.
                walk_block(file, finally, read_only_params, out);
                walk_finally(file, finally, 0, &mut Vec::new(), out);
            }
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                walk_block(file, &arm.block, read_only_params, out);
            }
        }
        _ => {}
    }
}

fn check_statement_out_argument_targets(
    file: &Path,
    statement: &Statement,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        Statement::Const { value, .. }
        | Statement::Throw { value, .. }
        | Statement::Expr { value, .. } => {
            check_expr_out_argument_targets(file, value, read_only_params, out);
        }
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            check_expr_out_argument_targets(file, target, read_only_params, out);
            check_expr_out_argument_targets(file, value, read_only_params, out);
        }
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                check_expr_out_argument_targets(file, value, read_only_params, out);
            }
        }
        Statement::Delete { path, .. } => {
            check_expr_out_argument_targets(file, path, read_only_params, out);
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                check_expr_out_argument_targets(file, value, read_only_params, out);
            }
        }
        Statement::If {
            condition,
            else_ifs,
            ..
        } => {
            if let Some(condition) = condition {
                check_expr_out_argument_targets(file, condition, read_only_params, out);
            }
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    check_expr_out_argument_targets(file, condition, read_only_params, out);
                }
            }
        }
        Statement::While { condition, .. } => {
            if let Some(condition) = condition {
                check_expr_out_argument_targets(file, condition, read_only_params, out);
            }
        }
        Statement::For { iterable, step, .. } => {
            check_expr_out_argument_targets(file, iterable, read_only_params, out);
            if let Some(step) = step {
                check_expr_out_argument_targets(file, step, read_only_params, out);
            }
        }
        Statement::Lock { path, .. } => {
            if let Some(path) = path {
                check_expr_out_argument_targets(file, path, read_only_params, out);
            }
        }
        Statement::Match { scrutinee, .. } => {
            if let Some(scrutinee) = scrutinee {
                check_expr_out_argument_targets(file, scrutinee, read_only_params, out);
            }
        }
        Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Transaction { .. }
        | Statement::Try { .. } => {}
    }
}

fn check_expr_out_argument_targets(
    file: &Path,
    expr: &Expression,
    read_only_params: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    match expr {
        Expression::Call { callee, args, .. } => {
            check_expr_out_argument_targets(file, callee, read_only_params, out);
            for arg in args {
                if arg.mode.is_some() {
                    check_read_only_out_argument(file, &arg.value, read_only_params, out);
                }
                check_expr_out_argument_targets(file, &arg.value, read_only_params, out);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            check_expr_out_argument_targets(file, base, read_only_params, out);
        }
        Expression::Unary { operand, .. } => {
            check_expr_out_argument_targets(file, operand, read_only_params, out);
        }
        Expression::Binary { left, right, .. } => {
            check_expr_out_argument_targets(file, left, read_only_params, out);
            check_expr_out_argument_targets(file, right, read_only_params, out);
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    check_expr_out_argument_targets(file, expr, read_only_params, out);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn check_read_only_out_argument(
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
    out: &mut Vec<CheckDiagnostic>,
) {
    if !is_assignable(target) {
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

fn check_out_parameters_assigned(
    file: &Path,
    function: &FunctionDecl,
    out: &mut Vec<CheckDiagnostic>,
) {
    let out_params: HashSet<String> = function
        .params
        .iter()
        .filter(|param| matches!(param.mode, Some(ParamMode::Out)))
        .map(|param| param.name.clone())
        .collect();
    if out_params.is_empty() {
        return;
    }

    let initial = OutState {
        assigned: HashSet::new(),
        visible: out_params.clone(),
    };
    let flow = walk_out_block(file, &function.body, &out_params, vec![initial], out);
    let mut reported = HashSet::new();
    for state in flow.fallthrough.into_iter().chain(flow.returns) {
        for name in &out_params {
            if !state.assigned.contains(name) && reported.insert(name.clone()) {
                out.push(out_assignment_diagnostic(file, function.span, name));
            }
        }
    }
}

#[derive(Clone)]
struct OutState {
    assigned: HashSet<String>,
    visible: HashSet<String>,
}

#[derive(Default)]
struct OutFlow {
    fallthrough: Vec<OutState>,
    returns: Vec<OutState>,
}

impl OutFlow {
    fn append(&mut self, other: OutFlow) {
        self.fallthrough.extend(other.fallthrough);
        self.returns.extend(other.returns);
    }
}

fn walk_out_block(
    file: &Path,
    block: &Block,
    out_params: &HashSet<String>,
    mut states: Vec<OutState>,
    out: &mut Vec<CheckDiagnostic>,
) -> OutFlow {
    let mut returns = Vec::new();
    for statement in &block.statements {
        let mut next = Vec::new();
        for state in states {
            let flow = walk_out_statement(file, statement, out_params, state, out);
            next.extend(flow.fallthrough);
            returns.extend(flow.returns);
        }
        if let Some(name) = statement_binding_name(statement) {
            for state in &mut next {
                state.visible.remove(name);
            }
        }
        states = next;
        if states.is_empty() {
            break;
        }
    }
    OutFlow {
        fallthrough: states,
        returns,
    }
}

fn walk_out_statement(
    file: &Path,
    statement: &Statement,
    out_params: &HashSet<String>,
    mut state: OutState,
    out: &mut Vec<CheckDiagnostic>,
) -> OutFlow {
    match statement {
        Statement::Const { value, .. } | Statement::Throw { value, .. } => {
            mark_expr_out_assignments(value, &mut state);
            if matches!(statement, Statement::Throw { .. }) {
                OutFlow::default()
            } else {
                OutFlow {
                    fallthrough: vec![state],
                    returns: Vec::new(),
                }
            }
        }
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                mark_expr_out_assignments(value, &mut state);
            }
            OutFlow {
                fallthrough: vec![state],
                returns: Vec::new(),
            }
        }
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            mark_expr_out_assignments(target, &mut state);
            mark_expr_out_assignments(value, &mut state);
            mark_place_assignment(target, &mut state);
            OutFlow {
                fallthrough: vec![state],
                returns: Vec::new(),
            }
        }
        Statement::Delete { path, .. } => {
            mark_expr_out_assignments(path, &mut state);
            OutFlow {
                fallthrough: vec![state],
                returns: Vec::new(),
            }
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                mark_expr_out_assignments(value, &mut state);
            }
            OutFlow {
                fallthrough: Vec::new(),
                returns: vec![state],
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } => OutFlow::default(),
        Statement::Expr { value, .. } => {
            mark_expr_out_assignments(value, &mut state);
            OutFlow {
                fallthrough: vec![state],
                returns: Vec::new(),
            }
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                mark_expr_out_assignments(condition, &mut state);
            }
            let mut flow = walk_out_block(file, then_block, out_params, vec![state.clone()], out);
            for else_if in else_ifs {
                let mut else_if_state = state.clone();
                if let Some(condition) = &else_if.condition {
                    mark_expr_out_assignments(condition, &mut else_if_state);
                }
                flow.append(walk_out_block(
                    file,
                    &else_if.block,
                    out_params,
                    vec![else_if_state],
                    out,
                ));
            }
            if let Some(block) = else_block {
                flow.append(walk_out_block(file, block, out_params, vec![state], out));
            } else {
                flow.fallthrough.push(state);
            }
            flow
        }
        Statement::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                mark_expr_out_assignments(condition, &mut state);
            }
            let mut flow = walk_out_block(file, body, out_params, vec![state.clone()], out);
            flow.fallthrough.push(state);
            flow
        }
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => {
            mark_expr_out_assignments(iterable, &mut state);
            if let Some(step) = step {
                mark_expr_out_assignments(step, &mut state);
            }
            let mut flow = walk_out_block(file, body, out_params, vec![state.clone()], out);
            flow.fallthrough.push(state);
            flow
        }
        Statement::Transaction { body, .. } => {
            walk_out_block(file, body, out_params, vec![state], out)
        }
        Statement::Lock { path, body, .. } => {
            if let Some(path) = path {
                mark_expr_out_assignments(path, &mut state);
            }
            walk_out_block(file, body, out_params, vec![state], out)
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            let mut flow = walk_out_block(file, body, out_params, vec![state.clone()], out);
            if let Some(catch) = catch {
                flow.append(walk_out_block(
                    file,
                    &catch.block,
                    out_params,
                    vec![state],
                    out,
                ));
            }
            if let Some(finally) = finally {
                apply_finally(file, finally, out_params, flow, out)
            } else {
                flow
            }
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                mark_expr_out_assignments(scrutinee, &mut state);
            }
            let mut flow = OutFlow::default();
            for arm in arms {
                flow.append(walk_out_block(
                    file,
                    &arm.block,
                    out_params,
                    vec![state.clone()],
                    out,
                ));
            }
            flow
        }
    }
}

fn apply_finally(
    file: &Path,
    finally: &Block,
    out_params: &HashSet<String>,
    flow: OutFlow,
    out: &mut Vec<CheckDiagnostic>,
) -> OutFlow {
    let fallthrough = walk_out_block(file, finally, out_params, flow.fallthrough, out);
    let returns = walk_out_block(file, finally, out_params, flow.returns, out);
    OutFlow {
        fallthrough: fallthrough.fallthrough,
        returns: fallthrough
            .returns
            .into_iter()
            .chain(returns.fallthrough)
            .chain(returns.returns)
            .collect(),
    }
}

fn mark_expr_out_assignments(expr: &Expression, state: &mut OutState) {
    match expr {
        Expression::Call { callee, args, .. } => {
            mark_expr_out_assignments(callee, state);
            for arg in args {
                mark_expr_out_assignments(&arg.value, state);
                if arg.mode.is_some() {
                    mark_place_assignment(&arg.value, state);
                }
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            mark_expr_out_assignments(base, state);
        }
        Expression::Unary { operand, .. } => mark_expr_out_assignments(operand, state),
        Expression::Binary {
            op, left, right, ..
        } => {
            mark_expr_out_assignments(left, state);
            if !matches!(op, BinaryOp::And | BinaryOp::Or) {
                mark_expr_out_assignments(right, state);
            }
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    mark_expr_out_assignments(expr, state);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn mark_place_assignment(target: &Expression, state: &mut OutState) {
    if is_assignable(target)
        && let Some(name) = place_root_name(target)
        && state.visible.contains(name)
    {
        state.assigned.insert(name.to_string());
    }
}

fn out_assignment_diagnostic(file: &Path, span: SourceSpan, name: &str) -> CheckDiagnostic {
    CheckDiagnostic {
        code: CHECK_OUT_PARAMETER_ASSIGNMENT,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: format!("out parameter `{name}` must be assigned before returning"),
        span,
    }
}

fn statement_binding_name(statement: &Statement) -> Option<&str> {
    match statement {
        Statement::Const { name, .. } | Statement::Var { name, .. } => Some(name),
        _ => None,
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
                if finally_jump_escapes(label.as_deref(), loop_depth, loop_labels) =>
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
            Statement::Transaction { body, .. } | Statement::Lock { body, .. } => {
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
            _ => {}
        }
    }
}

/// Does a `break`/`continue` in a finally block escape it? An unlabeled jump
/// escapes when no enclosing loop is within the finally. A labeled jump escapes
/// unless its target loop was introduced within the finally.
fn finally_jump_escapes(label: Option<&str>, loop_depth: usize, loop_labels: &[String]) -> bool {
    match label {
        None => loop_depth == 0,
        Some(label) => !loop_labels.iter().any(|known| known == label),
    }
}

/// Walk a block reporting a `break`/`continue` that names no loop it can reach:
/// an unlabeled jump with no enclosing loop (`loop_depth == 0`), or a labeled
/// jump whose label names no enclosing loop (`loop_labels`). Mirrors the runtime,
/// which otherwise only fails late with `run.no_enclosing_loop`. A `finally`
/// block's own escaping jumps are the `walk_finally` rule's concern, but they
/// still sit inside the function's loop nesting, so this walk descends into them
/// with the surrounding loop context.
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
                if loop_jump_unresolved(label.as_deref(), loop_depth, loop_labels) =>
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
            Statement::Transaction { body, .. } | Statement::Lock { body, .. } => {
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
            _ => {}
        }
    }
}

/// Does a `break`/`continue` name no loop it can reach? An unlabeled jump is
/// unresolved when no loop encloses it; a labeled jump is unresolved unless its
/// label names an enclosing loop.
fn loop_jump_unresolved(label: Option<&str>, loop_depth: usize, loop_labels: &[String]) -> bool {
    match label {
        None => loop_depth == 0,
        Some(label) => !loop_labels.iter().any(|known| known == label),
    }
}

/// Walk a block reporting a write/delete/append/merge that mutates the same saved
/// layer an enclosing `for` loop is traversing, which is forbidden because mutating
/// a tree layer while iterating it has undefined ordering. `traversed` holds the
/// canonical text of each enclosing loop's traversed saved layer; a mutation whose
/// affected layer matches one of them removes or adds keys from a layer being iterated.
fn walk_loop_layer_mutations(
    file: &Path,
    block: &Block,
    traversed: &mut Vec<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        // Flag a mutation whose affected layer is one a loop is iterating.
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
            Statement::While { body, .. }
            | Statement::Transaction { body, .. }
            | Statement::Lock { body, .. } => {
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
            _ => {}
        }
    }
}

/// The saved layer a `for` loop traverses, as canonical text, or `None` for a
/// loop over a range or a local value. A loop traverses a saved layer only when
/// its iterable is a saved path directly or wrapped in `keys`/`values`/`entries`;
/// iterating a local (the "collect keys first" pattern) traverses no saved layer.
fn traversed_layer(iterable: &Expression) -> Option<String> {
    let path = traversal_argument(iterable).unwrap_or(iterable);
    is_saved_path(path).then(|| format_expression(path))
}

/// The sole argument of a `keys`/`values`/`entries` call, or `None` for any other
/// expression. These wrap a saved layer without changing which layer is traversed.
fn traversal_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1 || !matches!(segments[0].as_str(), "keys" | "values" | "entries") {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The saved layer a statement adds keys to or removes keys from, as canonical
/// text, or `None` when the statement does not change a saved layer's key set. A
/// whole-record/keyed-entry write, `delete`, or whole-record `merge` of
/// `^root(key…)` affects the parent layer (the callee). `append(path, v)` and a
/// keyed-leaf/layer `merge` affect the named layer. A scalar field write or field
/// delete keeps the layer's keys, so it is not reported here.
fn mutated_layer(statement: &Statement) -> Option<String> {
    match statement {
        Statement::Assign { target, .. } => keyed_entry_parent(target),
        Statement::Delete { path, .. } => keyed_entry_parent(path),
        Statement::Merge { target, .. } => match target {
            // A keyed-layer merge `merge ^root(key…).layer = …` overlays entries
            // into that layer, so the layer itself is the affected key set.
            Expression::Field { base, .. } if is_saved_path(base) => {
                Some(format_expression(target))
            }
            other => keyed_entry_parent(other),
        },
        Statement::Expr {
            value: Expression::Call { callee, args, .. },
            ..
        } => append_target(callee, args).map(format_expression),
        _ => None,
    }
}

/// The layer that gains or loses a key when `target` (a `^root(key…)` or
/// `^root(key…).layer(key…)` place) is written, deleted, or merged whole: the
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
/// lookup on a saved place. A parenthesized suffix on a bare name is a function
/// call, not a place, so `f(x)` is rejected while `^books(id).title` is allowed.
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
/// further key lookup. A bare name callee is a function call, not a place.
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

/// A diagnostic located at an expression's span.
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
    }
}

/// A diagnostic located at a statement's span.
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
    }
}
