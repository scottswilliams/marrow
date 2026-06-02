use marrow_syntax::{Argument, Block, Expression, InterpolationPart, Statement};

use super::calls::{is_append_call, is_attached_data_call, is_exists_call, is_neighbor_read};
use super::effects::{
    condition_narrowings, invalidate_key_bindings, invalidate_removed_narrowings,
    invalidate_saved_narrowings, invalidate_written_target, mutating_arg_bindings,
    traversal_narrowing,
};
use super::keys::saved_path_parts;
use super::proofs::{ReadContext, read_proof, record_read};
use super::scope::NameScope;
use super::target::{ReadTarget, read_target_with_scope};
use super::writes::call_writes_saved_data;
use crate::{CheckDiagnostic, CheckedProgram};

pub(crate) fn check_presence(program: &mut CheckedProgram, diagnostics: &mut Vec<CheckDiagnostic>) {
    let modules = program.modules.clone();
    for module in &modules {
        for function in &module.functions {
            let mut scope = NameScope::for_function(function);
            let mut narrowed = Vec::new();
            collect_block(
                program,
                &function.body,
                &mut narrowed,
                &mut scope,
                diagnostics,
            );
        }
        for constant in &module.constants {
            if let Some(value) = &constant.value {
                let mut scope = NameScope::default();
                let mut narrowed = Vec::new();
                collect_expr(
                    program,
                    value,
                    ReadContext::Bare,
                    &mut narrowed,
                    &mut scope,
                    diagnostics,
                );
            }
        }
    }
}

fn collect_block(
    program: &mut CheckedProgram,
    block: &Block,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    scope.push_frame();
    for statement in &block.statements {
        collect_statement(program, statement, narrowed, scope, diagnostics);
    }
    scope.pop_frame();
}

fn collect_statement(
    program: &mut CheckedProgram,
    statement: &Statement,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        Statement::Const { name, value, .. } => {
            collect_bare_expr(program, value, narrowed, scope, diagnostics);
            scope.bind(name);
        }
        Statement::Throw { value, .. } | Statement::Expr { value, .. } => {
            collect_bare_expr(program, value, narrowed, scope, diagnostics);
        }
        Statement::Var { name, value, .. } => {
            collect_optional_bare_expr(program, value.as_ref(), narrowed, scope, diagnostics);
            scope.bind(name);
        }
        Statement::Assign { target, value, .. } | Statement::Merge { target, value, .. } => {
            collect_assignment_statement(program, target, value, narrowed, scope, diagnostics);
        }
        Statement::Delete { path, .. } => {
            collect_delete_statement(program, path, narrowed, scope, diagnostics);
        }
        Statement::Return { value, .. } => {
            collect_optional_bare_expr(program, value.as_ref(), narrowed, scope, diagnostics);
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => collect_if_statement(
            IfStatementParts {
                condition: condition.as_ref(),
                then_block,
                else_ifs,
                else_block: else_block.as_ref(),
            },
            program,
            narrowed,
            scope,
            diagnostics,
        ),
        Statement::While {
            condition, body, ..
        } => {
            collect_optional_bare_expr(program, condition.as_ref(), narrowed, scope, diagnostics);
            collect_block(program, body, narrowed, scope, diagnostics);
        }
        Statement::For {
            binding,
            iterable,
            step,
            body,
            ..
        } => collect_for_statement(
            ForStatementParts {
                binding,
                iterable,
                step: step.as_ref(),
                body,
            },
            program,
            narrowed,
            scope,
            diagnostics,
        ),
        Statement::Transaction { body, .. } | Statement::Lock { body, .. } => {
            collect_block(program, body, narrowed, scope, diagnostics);
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => collect_try_statement(
            program,
            body,
            catch.as_ref(),
            finally.as_ref(),
            narrowed,
            scope,
            diagnostics,
        ),
        Statement::Match {
            scrutinee, arms, ..
        } => {
            collect_match_statement(
                program,
                scrutinee.as_ref(),
                arms,
                narrowed,
                scope,
                diagnostics,
            );
        }
        Statement::Break { .. } | Statement::Continue { .. } => {}
    }
}

fn collect_bare_expr(
    program: &mut CheckedProgram,
    expr: &Expression,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_expr(
        program,
        expr,
        ReadContext::Bare,
        narrowed,
        scope,
        diagnostics,
    );
}

fn collect_optional_bare_expr(
    program: &mut CheckedProgram,
    expr: Option<&Expression>,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if let Some(expr) = expr {
        collect_bare_expr(program, expr, narrowed, scope, diagnostics);
    }
}

fn collect_assignment_statement(
    program: &mut CheckedProgram,
    target: &Expression,
    value: &Expression,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let assigned = super::keys::assigned_bindings(target, scope);
    let written = read_target_with_scope(program, target, scope);
    collect_write_target(program, target, narrowed, scope, diagnostics);
    collect_bare_expr(program, value, narrowed, scope, diagnostics);
    invalidate_key_bindings(narrowed, assigned);
    if let Some(written) = written {
        invalidate_written_target(narrowed, &written);
    }
}

fn collect_delete_statement(
    program: &mut CheckedProgram,
    path: &Expression,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let written = read_target_with_scope(program, path, scope);
    collect_write_target(program, path, narrowed, scope, diagnostics);
    if let Some(written) = written {
        invalidate_written_target(narrowed, &written);
    }
}

struct IfStatementParts<'a> {
    condition: Option<&'a Expression>,
    then_block: &'a Block,
    else_ifs: &'a [marrow_syntax::ElseIf],
    else_block: Option<&'a Block>,
}

fn collect_if_statement(
    parts: IfStatementParts<'_>,
    program: &mut CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_optional_bare_expr(program, parts.condition, narrowed, scope, diagnostics);
    collect_guarded_block(
        program,
        parts.condition,
        parts.then_block,
        narrowed,
        scope,
        diagnostics,
    );
    for else_if in parts.else_ifs {
        collect_optional_bare_expr(
            program,
            else_if.condition.as_ref(),
            narrowed,
            scope,
            diagnostics,
        );
        collect_guarded_block(
            program,
            else_if.condition.as_ref(),
            &else_if.block,
            narrowed,
            scope,
            diagnostics,
        );
    }
    if let Some(block) = parts.else_block {
        collect_block(program, block, narrowed, scope, diagnostics);
    }
}

fn collect_guarded_block(
    program: &mut CheckedProgram,
    condition: Option<&Expression>,
    block: &Block,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let mut branch_narrowed = narrowed.to_vec();
    if let Some(condition) = condition {
        super::util::extend_unique(
            &mut branch_narrowed,
            condition_narrowings(program, condition, scope),
        );
    }
    let branch_start = branch_narrowed.clone();
    collect_block(program, block, &mut branch_narrowed, scope, diagnostics);
    invalidate_removed_narrowings(narrowed, &branch_start, &branch_narrowed);
}

struct ForStatementParts<'a> {
    binding: &'a marrow_syntax::ForBinding,
    iterable: &'a Expression,
    step: Option<&'a Expression>,
    body: &'a Block,
}

fn collect_for_statement(
    parts: ForStatementParts<'_>,
    program: &mut CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_expr(
        program,
        parts.iterable,
        ReadContext::AttachedData,
        narrowed,
        scope,
        diagnostics,
    );
    collect_optional_bare_expr(program, parts.step, narrowed, scope, diagnostics);
    scope.push_frame();
    scope.bind(&parts.binding.first);
    if let Some(second) = &parts.binding.second {
        scope.bind(second);
    }
    let mut body_narrowed = narrowed.clone();
    if let Some(target) = traversal_narrowing(program, parts.iterable, parts.binding, scope) {
        super::util::extend_unique(&mut body_narrowed, vec![target]);
    }
    let body_start = body_narrowed.clone();
    for statement in &parts.body.statements {
        collect_statement(program, statement, &mut body_narrowed, scope, diagnostics);
    }
    invalidate_removed_narrowings(narrowed, &body_start, &body_narrowed);
    scope.pop_frame();
}

fn collect_try_statement(
    program: &mut CheckedProgram,
    body: &Block,
    catch: Option<&marrow_syntax::CatchClause>,
    finally: Option<&Block>,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_block(program, body, narrowed, scope, diagnostics);
    if let Some(catch) = catch {
        scope.push_frame();
        scope.bind(&catch.name);
        for statement in &catch.block.statements {
            collect_statement(program, statement, narrowed, scope, diagnostics);
        }
        scope.pop_frame();
    }
    if let Some(finally) = finally {
        collect_block(program, finally, narrowed, scope, diagnostics);
    }
}

fn collect_match_statement(
    program: &mut CheckedProgram,
    scrutinee: Option<&Expression>,
    arms: &[marrow_syntax::MatchArm],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_optional_bare_expr(program, scrutinee, narrowed, scope, diagnostics);
    for arm in arms {
        let mut arm_narrowed = narrowed.clone();
        let arm_start = arm_narrowed.clone();
        collect_block(program, &arm.block, &mut arm_narrowed, scope, diagnostics);
        invalidate_removed_narrowings(narrowed, &arm_start, &arm_narrowed);
    }
}

fn collect_write_target(
    program: &mut CheckedProgram,
    expr: &Expression,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_path_key_exprs(program, expr, narrowed, scope, diagnostics);
}

fn collect_expr(
    program: &mut CheckedProgram,
    expr: &Expression,
    context: ReadContext,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if let Some(read) = read_proof(program, expr, context, narrowed.as_slice(), scope) {
        record_read(program, expr, read, context, diagnostics);
        collect_path_key_exprs(program, expr, narrowed, scope, diagnostics);
        return;
    }
    if saved_path_parts(expr, scope).is_some() {
        collect_path_key_exprs(program, expr, narrowed, scope, diagnostics);
        return;
    }

    match expr {
        Expression::Call { callee, args, .. } => {
            collect_call_expr(program, callee, args, narrowed, scope, diagnostics);
        }
        Expression::Field { base, .. } => {
            collect_bare_expr(program, base, narrowed, scope, diagnostics);
        }
        Expression::OptionalField { base, .. } => collect_expr(
            program,
            base,
            ReadContext::Resolved,
            narrowed,
            scope,
            diagnostics,
        ),
        Expression::Unary { operand, .. } => {
            collect_bare_expr(program, operand, narrowed, scope, diagnostics);
        }
        Expression::Binary {
            op, left, right, ..
        } => {
            collect_binary_expr(program, *op, left, right, narrowed, scope, diagnostics);
        }
        Expression::Interpolation { parts, .. } => {
            collect_interpolation_expr(program, parts, narrowed, scope, diagnostics);
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn collect_call_expr(
    program: &mut CheckedProgram,
    callee: &Expression,
    args: &[Argument],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if is_exists_call(callee) {
        collect_exists_args(program, args, narrowed, scope, diagnostics);
    } else if is_attached_data_call(callee) {
        collect_args(
            program,
            args,
            ReadContext::AttachedData,
            narrowed,
            scope,
            diagnostics,
        );
    } else if is_append_call(callee) {
        collect_append_args(program, args, narrowed, scope, diagnostics);
    } else {
        collect_plain_call(program, callee, args, narrowed, scope, diagnostics);
    }
}

fn collect_exists_args(
    program: &mut CheckedProgram,
    args: &[Argument],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if let Some(arg) = args.first() {
        collect_expr(
            program,
            &arg.value,
            ReadContext::Resolved,
            narrowed,
            scope,
            diagnostics,
        );
    }
    collect_args(
        program,
        &args[args.len().min(1)..],
        ReadContext::Bare,
        narrowed,
        scope,
        diagnostics,
    );
}

fn collect_append_args(
    program: &mut CheckedProgram,
    args: &[Argument],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if let Some(target) = args.first() {
        collect_write_target(program, &target.value, narrowed, scope, diagnostics);
    }
    collect_args(
        program,
        &args[args.len().min(1)..],
        ReadContext::Bare,
        narrowed,
        scope,
        diagnostics,
    );
}

fn collect_plain_call(
    program: &mut CheckedProgram,
    callee: &Expression,
    args: &[Argument],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let writes_saved = call_writes_saved_data(program, callee);
    collect_bare_expr(program, callee, narrowed, scope, diagnostics);
    collect_args(
        program,
        args,
        ReadContext::Bare,
        narrowed,
        scope,
        diagnostics,
    );
    if writes_saved {
        invalidate_saved_narrowings(narrowed);
    }
}

fn collect_args(
    program: &mut CheckedProgram,
    args: &[Argument],
    context: ReadContext,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let mutated = mutating_arg_bindings(args, scope);
    for arg in args {
        collect_expr(program, &arg.value, context, narrowed, scope, diagnostics);
    }
    invalidate_key_bindings(narrowed, mutated);
}

fn collect_binary_expr(
    program: &mut CheckedProgram,
    op: marrow_syntax::BinaryOp,
    left: &Expression,
    right: &Expression,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let left_context = if op == marrow_syntax::BinaryOp::Coalesce {
        ReadContext::Resolved
    } else {
        ReadContext::Bare
    };
    collect_expr(program, left, left_context, narrowed, scope, diagnostics);
    collect_bare_expr(program, right, narrowed, scope, diagnostics);
}

fn collect_interpolation_expr(
    program: &mut CheckedProgram,
    parts: &[InterpolationPart],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for part in parts {
        if let InterpolationPart::Expr(expr) = part {
            collect_bare_expr(program, expr, narrowed, scope, diagnostics);
        }
    }
}

fn collect_path_key_exprs(
    program: &mut CheckedProgram,
    expr: &Expression,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match expr {
        Expression::Call { callee, args, .. } if is_neighbor_read(callee) => {
            if let Some(arg) = args.first() {
                collect_path_key_exprs(program, &arg.value, narrowed, scope, diagnostics);
            }
            collect_args(
                program,
                &args[args.len().min(1)..],
                ReadContext::Bare,
                narrowed,
                scope,
                diagnostics,
            );
        }
        Expression::Call { callee, args, .. } => {
            collect_path_key_exprs(program, callee, narrowed, scope, diagnostics);
            collect_args(
                program,
                args,
                ReadContext::Bare,
                narrowed,
                scope,
                diagnostics,
            );
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            collect_path_key_exprs(program, base, narrowed, scope, diagnostics);
        }
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Unary { .. }
        | Expression::Binary { .. }
        | Expression::Interpolation { .. } => {}
    }
}
