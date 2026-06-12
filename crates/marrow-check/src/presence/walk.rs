use super::calls::std_path_arg_mask;
use super::effects::{
    condition_narrowings, invalidate_key_bindings, invalidate_removed_narrowings,
    invalidate_saved_narrowings, invalidate_written_target, negated_exists_narrowings,
    traversal_narrowing,
};
use super::keys::saved_path_parts;
use super::proofs::{ReadContext, read_proof, record_read};
use super::scope::NameScope;
use super::target::{ReadTarget, read_target_with_scope};
use super::writes::call_writes_saved_data;
use crate::executable::CheckedExecutableContext;
use crate::{
    CheckDiagnostic, CheckedArg, CheckedBinaryOp, CheckedBody, CheckedBuiltinCall,
    CheckedCallTarget, CheckedCatchClause, CheckedElseIf, CheckedExpr, CheckedForBinding,
    CheckedInterpolationPart, CheckedMatchArm, CheckedProgram, CheckedStmt,
};

pub(crate) fn check_presence(program: &mut CheckedProgram, diagnostics: &mut Vec<CheckDiagnostic>) {
    let modules = program.modules.clone();
    for (module_index, module) in modules.iter().enumerate() {
        for function in &module.functions {
            let mut scope = NameScope::for_function(function);
            let mut narrowed = Vec::new();
            if let Some(body) = function.runtime_body() {
                collect_block(program, body, &mut narrowed, &mut scope, diagnostics);
            }
        }
        for constant in &module.constants {
            if let Some(value) = &constant.value {
                let context = CheckedExecutableContext::new(program, module_index);
                let mut lower_scope = Vec::new();
                let Some(value) = CheckedExpr::lower(value, &context, &mut lower_scope) else {
                    continue;
                };
                let mut scope = NameScope::default();
                let mut narrowed = Vec::new();
                collect_expr(
                    program,
                    &value,
                    ReadContext::Bare,
                    &mut narrowed,
                    &mut scope,
                    diagnostics,
                );
            }
        }
    }
    let transforms = program.catalog.evolve_transforms.clone();
    for transform in &transforms {
        let Some(body) = transform.runtime_body() else {
            continue;
        };
        let mut scope = NameScope::for_transform(&transform.resource);
        let mut narrowed = Vec::new();
        collect_block(program, body, &mut narrowed, &mut scope, diagnostics);
    }
}

fn collect_block(
    program: &mut CheckedProgram,
    block: &CheckedBody,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_block_with_bindings(program, block, &[], narrowed, scope, diagnostics);
}

fn collect_block_with_bindings(
    program: &mut CheckedProgram,
    block: &CheckedBody,
    initial_bindings: &[String],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    scope.push_frame();
    for binding in initial_bindings {
        scope.bind(binding);
    }
    for statement in block.statements() {
        collect_statement(program, statement, narrowed, scope, diagnostics);
    }
    scope.pop_frame();
}

fn collect_statement(
    program: &mut CheckedProgram,
    statement: &CheckedStmt,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        CheckedStmt::Const { name, value, .. } => {
            collect_bare_expr(program, value, narrowed, scope, diagnostics);
            scope.bind(name);
        }
        CheckedStmt::Throw { value, .. } | CheckedStmt::Expr { value, .. } => {
            collect_bare_expr(program, value, narrowed, scope, diagnostics);
        }
        CheckedStmt::Var { name, value, .. } => {
            collect_optional_bare_expr(program, value.as_ref(), narrowed, scope, diagnostics);
            scope.bind(name);
        }
        CheckedStmt::Assign { target, value, .. } => {
            collect_assignment_statement(program, target, value, narrowed, scope, diagnostics);
        }
        CheckedStmt::Delete { path, .. } => {
            collect_delete_statement(program, path, narrowed, scope, diagnostics);
        }
        CheckedStmt::Return { value, .. } => {
            collect_optional_bare_expr(program, value.as_ref(), narrowed, scope, diagnostics);
        }
        CheckedStmt::If {
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
        CheckedStmt::IfConst {
            name,
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => collect_if_const_statement(
            IfConstStatementParts {
                name,
                value,
                then_block,
                else_ifs,
                else_block: else_block.as_ref(),
            },
            program,
            narrowed,
            scope,
            diagnostics,
        ),
        CheckedStmt::While {
            condition, body, ..
        } => {
            collect_optional_bare_expr(program, condition.as_ref(), narrowed, scope, diagnostics);
            collect_block(program, body, narrowed, scope, diagnostics);
        }
        CheckedStmt::For {
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
        CheckedStmt::Transaction { body, .. } => {
            collect_block(program, body, narrowed, scope, diagnostics);
        }
        CheckedStmt::Try { body, catch, .. } => {
            collect_try_statement(program, body, catch.as_ref(), narrowed, scope, diagnostics)
        }
        CheckedStmt::Match {
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
        CheckedStmt::Break { .. } | CheckedStmt::Continue { .. } => {}
    }
}

fn collect_bare_expr(
    program: &mut CheckedProgram,
    expr: &CheckedExpr,
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
    expr: Option<&CheckedExpr>,
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
    target: &CheckedExpr,
    value: &CheckedExpr,
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
    path: &CheckedExpr,
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
    condition: Option<&'a CheckedExpr>,
    then_block: &'a CheckedBody,
    else_ifs: &'a [CheckedElseIf],
    else_block: Option<&'a CheckedBody>,
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
    if parts.else_ifs.is_empty()
        && parts.else_block.is_none()
        && block_prevents_fallthrough(parts.then_block)
        && let Some(condition) = parts.condition
    {
        super::util::extend_unique(
            narrowed,
            negated_exists_narrowings(program, condition, scope),
        );
    }
}

struct IfConstStatementParts<'a> {
    name: &'a str,
    value: &'a CheckedExpr,
    then_block: &'a CheckedBody,
    else_ifs: &'a [CheckedElseIf],
    else_block: Option<&'a CheckedBody>,
}

fn collect_if_const_statement(
    parts: IfConstStatementParts<'_>,
    program: &mut CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_expr(
        program,
        parts.value,
        ReadContext::Resolved,
        narrowed,
        scope,
        diagnostics,
    );
    let mut branch_narrowed = narrowed.to_vec();
    if let Some(target) = read_target_with_scope(program, parts.value, scope) {
        super::util::extend_unique(&mut branch_narrowed, vec![target]);
    }
    let branch_start = branch_narrowed.clone();
    collect_block_with_bindings(
        program,
        parts.then_block,
        &[parts.name.to_string()],
        &mut branch_narrowed,
        scope,
        diagnostics,
    );
    invalidate_removed_narrowings(narrowed, &branch_start, &branch_narrowed);
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
    condition: Option<&CheckedExpr>,
    block: &CheckedBody,
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

fn block_prevents_fallthrough(block: &CheckedBody) -> bool {
    block
        .statements()
        .last()
        .is_some_and(statement_prevents_fallthrough)
}

fn statement_prevents_fallthrough(statement: &CheckedStmt) -> bool {
    match statement {
        CheckedStmt::Return { .. }
        | CheckedStmt::Throw { .. }
        | CheckedStmt::Break { .. }
        | CheckedStmt::Continue { .. } => true,
        CheckedStmt::If {
            then_block,
            else_ifs,
            else_block,
            ..
        }
        | CheckedStmt::IfConst {
            then_block,
            else_ifs,
            else_block,
            ..
        } => else_block.as_ref().is_some_and(|else_block| {
            block_prevents_fallthrough(then_block)
                && else_ifs
                    .iter()
                    .all(|else_if| block_prevents_fallthrough(&else_if.block))
                && block_prevents_fallthrough(else_block)
        }),
        CheckedStmt::Transaction { body, .. } => block_prevents_fallthrough(body),
        CheckedStmt::Try { body, catch, .. } => {
            block_prevents_fallthrough(body)
                && catch
                    .as_ref()
                    .is_none_or(|clause| block_prevents_fallthrough(&clause.block))
        }
        CheckedStmt::Match { arms, .. } => {
            !arms.is_empty()
                && arms
                    .iter()
                    .all(|arm| block_prevents_fallthrough(&arm.block))
        }
        CheckedStmt::Const { .. }
        | CheckedStmt::Var { .. }
        | CheckedStmt::Assign { .. }
        | CheckedStmt::Delete { .. }
        | CheckedStmt::Expr { .. }
        | CheckedStmt::While { .. }
        | CheckedStmt::For { .. } => false,
    }
}

struct ForStatementParts<'a> {
    binding: &'a CheckedForBinding,
    iterable: &'a CheckedExpr,
    step: Option<&'a CheckedExpr>,
    body: &'a CheckedBody,
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
    for statement in parts.body.statements() {
        collect_statement(program, statement, &mut body_narrowed, scope, diagnostics);
    }
    invalidate_removed_narrowings(narrowed, &body_start, &body_narrowed);
    scope.pop_frame();
}

fn collect_try_statement(
    program: &mut CheckedProgram,
    body: &CheckedBody,
    catch: Option<&CheckedCatchClause>,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_block(program, body, narrowed, scope, diagnostics);
    if let Some(catch) = catch {
        scope.push_frame();
        scope.bind(&catch.name);
        for statement in catch.block.statements() {
            collect_statement(program, statement, narrowed, scope, diagnostics);
        }
        scope.pop_frame();
    }
}

fn collect_match_statement(
    program: &mut CheckedProgram,
    scrutinee: Option<&CheckedExpr>,
    arms: &[CheckedMatchArm],
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
    expr: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    collect_path_key_exprs(program, expr, narrowed, scope, diagnostics);
}

fn collect_expr(
    program: &mut CheckedProgram,
    expr: &CheckedExpr,
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
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            collect_call_expr(program, callee, args, target, narrowed, scope, diagnostics);
        }
        CheckedExpr::Field { base, .. } => {
            collect_bare_expr(program, base, narrowed, scope, diagnostics);
        }
        CheckedExpr::OptionalField { base, .. } => collect_expr(
            program,
            base,
            ReadContext::Resolved,
            narrowed,
            scope,
            diagnostics,
        ),
        CheckedExpr::Unary { operand, .. } => {
            collect_bare_expr(program, operand, narrowed, scope, diagnostics);
        }
        CheckedExpr::Binary {
            op, left, right, ..
        } => {
            collect_binary_expr(program, *op, left, right, narrowed, scope, diagnostics);
        }
        CheckedExpr::Interpolation { parts, .. } => {
            collect_interpolation_expr(program, parts, narrowed, scope, diagnostics);
        }
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {}
    }
}

fn collect_call_expr(
    program: &mut CheckedProgram,
    callee: &CheckedExpr,
    args: &[CheckedArg],
    target: &CheckedCallTarget,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if matches!(target, CheckedCallTarget::IdentityConstructor(_)) {
        collect_args(
            program,
            &args[args.len().min(1)..],
            ReadContext::Bare,
            narrowed,
            scope,
            diagnostics,
        );
        return;
    }
    match builtin_of(target) {
        Some(CheckedBuiltinCall::Exists) => {
            collect_exists_args(program, args, narrowed, scope, diagnostics);
        }
        Some(CheckedBuiltinCall::Append) => {
            collect_append_args(program, args, narrowed, scope, diagnostics);
        }
        Some(builtin) if builtin.reads_attached_data() => {
            collect_args(
                program,
                args,
                ReadContext::AttachedData,
                narrowed,
                scope,
                diagnostics,
            );
        }
        _ => collect_plain_call(program, callee, args, target, narrowed, scope, diagnostics),
    }
}

/// The typed builtin a call resolved to, if any. The presence pass branches on
/// this rather than re-matching the callee's name strings, so builtin identity has
/// one owner.
fn builtin_of(target: &CheckedCallTarget) -> Option<CheckedBuiltinCall> {
    match target {
        CheckedCallTarget::Builtin(builtin) => Some(*builtin),
        _ => None,
    }
}

fn collect_exists_args(
    program: &mut CheckedProgram,
    args: &[CheckedArg],
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
    args: &[CheckedArg],
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
    callee: &CheckedExpr,
    args: &[CheckedArg],
    target: &CheckedCallTarget,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let writes_saved = call_writes_saved_data(program, target);
    collect_bare_expr(program, callee, narrowed, scope, diagnostics);
    if let Some(path_args) = std_path_arg_mask(target) {
        collect_std_args(program, args, &path_args, narrowed, scope, diagnostics);
    } else {
        collect_call_args(program, args, narrowed, scope, diagnostics);
    }
    if writes_saved {
        invalidate_saved_narrowings(narrowed);
    }
}

fn collect_call_args(
    program: &mut CheckedProgram,
    args: &[CheckedArg],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        collect_bare_expr(program, &arg.value, narrowed, scope, diagnostics);
    }
}

fn collect_args(
    program: &mut CheckedProgram,
    args: &[CheckedArg],
    context: ReadContext,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        collect_expr(program, &arg.value, context, narrowed, scope, diagnostics);
    }
}

fn collect_binary_expr(
    program: &mut CheckedProgram,
    op: CheckedBinaryOp,
    left: &CheckedExpr,
    right: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let left_context = if op == CheckedBinaryOp::Coalesce {
        ReadContext::Resolved
    } else {
        ReadContext::Bare
    };
    collect_expr(program, left, left_context, narrowed, scope, diagnostics);
    collect_bare_expr(program, right, narrowed, scope, diagnostics);
}

fn collect_interpolation_expr(
    program: &mut CheckedProgram,
    parts: &[CheckedInterpolationPart],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for part in parts {
        if let CheckedInterpolationPart::Expr(expr) = part {
            collect_bare_expr(program, expr, narrowed, scope, diagnostics);
        }
    }
}

fn collect_path_key_exprs(
    program: &mut CheckedProgram,
    expr: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match expr {
        CheckedExpr::Call { target, args, .. }
            if builtin_of(target).is_some_and(CheckedBuiltinCall::is_neighbor_read) =>
        {
            if let Some(read) = read_proof(program, expr, ReadContext::Resolved, narrowed, scope) {
                record_read(program, expr, read, ReadContext::Resolved, diagnostics);
            }
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
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            collect_path_key_exprs(program, callee, narrowed, scope, diagnostics);
            if let Some(path_args) = std_path_arg_mask(target) {
                collect_std_args(program, args, &path_args, narrowed, scope, diagnostics);
            } else {
                collect_args(
                    program,
                    args,
                    ReadContext::Bare,
                    narrowed,
                    scope,
                    diagnostics,
                );
            }
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            collect_path_key_exprs(program, base, narrowed, scope, diagnostics);
        }
        CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::SavedRoot { .. }
        | CheckedExpr::Unary { .. }
        | CheckedExpr::Binary { .. }
        | CheckedExpr::Interpolation { .. } => {}
    }
}

fn collect_std_args(
    program: &mut CheckedProgram,
    args: &[CheckedArg],
    path_args: &[bool],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for (index, arg) in args.iter().enumerate() {
        let context = if path_args.get(index).copied().unwrap_or(false) {
            ReadContext::AttachedData
        } else {
            ReadContext::Bare
        };
        collect_expr(program, &arg.value, context, narrowed, scope, diagnostics);
    }
}
