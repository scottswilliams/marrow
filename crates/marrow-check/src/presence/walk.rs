use super::calls::{maybe_present_result, std_path_arg_mask};
use super::effects::{
    condition_narrowings, invalidate_removed_narrowings, negated_exists_narrowings, saved_targets,
    targets_invalidated_by_key_bindings, targets_invalidated_by_written_target,
    traversal_narrowing,
};
use super::keys::saved_place_key;
use super::proofs::{PresenceRecorder, ReadContext, read_proof, record_read};
use super::scope::NameScope;
use super::target::{ReadTarget, read_target_with_scope, saved_path_read_target_with_scope};
use super::writes::{call_writes_saved_data, expr_calls_saved_writer};
use crate::executable::CheckedExecutableContext;
use crate::{
    CheckDiagnostic, CheckedArg, CheckedBinaryOp, CheckedBody, CheckedBuiltinCall,
    CheckedCallTarget, CheckedCatchClause, CheckedElseIf, CheckedExpr, CheckedForBinding,
    CheckedInterpolationPart, CheckedMatchArm, CheckedProgram, CheckedStmt, MarrowType,
};
use marrow_schema::ReturnPresence;

pub(crate) fn check_presence(program: &mut CheckedProgram, diagnostics: &mut Vec<CheckDiagnostic>) {
    let mut proofs = Vec::new();
    {
        let program_view: &CheckedProgram = program;
        let mut recorder = PresenceRecorder {
            proofs: &mut proofs,
            diagnostics,
        };
        for (module_index, module) in program_view.modules.iter().enumerate() {
            for function in &module.functions {
                let mut scope = NameScope::for_function(function, &module.source_file);
                let mut narrowed = Vec::new();
                if let Some(body) = function.runtime_body() {
                    collect_block(program_view, body, &mut narrowed, &mut scope, &mut recorder);
                }
            }
            for constant in &module.constants {
                if let Some(value) = &constant.value {
                    let context = CheckedExecutableContext::new(program_view, module_index);
                    let mut lower_scope = Vec::new();
                    let Some(value) = CheckedExpr::lower(value, &context, &mut lower_scope) else {
                        continue;
                    };
                    let mut scope = NameScope::default();
                    let mut narrowed = Vec::new();
                    let mut events = InvalidationLog::default();
                    collect_expr(
                        program_view,
                        &value,
                        ReadContext::Bare,
                        &mut narrowed,
                        &mut scope,
                        &mut recorder,
                        &mut events,
                    );
                }
            }
        }
        for transform in &program_view.catalog.evolve_transforms {
            let Some(body) = transform.runtime_body() else {
                continue;
            };
            let mut scope = NameScope::for_transform(&transform.resource);
            let mut narrowed = Vec::new();
            collect_block(program_view, body, &mut narrowed, &mut scope, &mut recorder);
        }
    }
    for proof in proofs {
        program.facts.record_presence_proof(proof);
    }
}

fn collect_block(
    program: &CheckedProgram,
    block: &CheckedBody,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
) {
    let mut events = InvalidationLog::default();
    collect_block_with_bindings(program, block, &[], narrowed, scope, recorder, &mut events);
}

fn collect_block_with_bindings(
    program: &CheckedProgram,
    block: &CheckedBody,
    initial_bindings: &[BlockBinding<'_>],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    scope.push_frame();
    for binding in initial_bindings {
        scope.bind_with_type(binding.name, binding.ty.clone());
    }
    for statement in block.statements() {
        collect_statement(program, statement, narrowed, scope, recorder, events);
    }
    scope.pop_frame();
}

struct BlockBinding<'a> {
    name: &'a str,
    ty: Option<MarrowType>,
}

impl<'a> BlockBinding<'a> {
    fn typed(name: &'a str, ty: Option<MarrowType>) -> Self {
        Self { name, ty }
    }
}

#[derive(Default)]
struct InvalidationLog {
    tracked: Vec<ReadTarget>,
    invalidated: Vec<ReadTarget>,
}

impl InvalidationLog {
    fn tracking(tracked: Vec<ReadTarget>) -> Self {
        Self {
            tracked,
            invalidated: Vec::new(),
        }
    }

    fn record(&mut self, targets: Vec<ReadTarget>) {
        if self.tracked.is_empty() {
            return;
        }
        let targets = targets
            .into_iter()
            .filter(|target| self.tracked.contains(target))
            .collect();
        super::util::extend_unique(&mut self.invalidated, targets);
    }

    fn removed(&mut self, before: &[ReadTarget], after: &[ReadTarget]) {
        self.record(
            before
                .iter()
                .filter(|target| !after.contains(target))
                .cloned()
                .collect(),
        );
    }
}

fn remove_invalidated(narrowed: &mut Vec<ReadTarget>, invalidated: &[ReadTarget]) {
    narrowed.retain(|target| !invalidated.contains(target));
}

fn invalidate_key_bindings(
    narrowed: &mut Vec<ReadTarget>,
    events: &mut InvalidationLog,
    bindings: &[u32],
) {
    let invalidated = targets_invalidated_by_key_bindings(narrowed, bindings);
    events.record(invalidated.clone());
    remove_invalidated(narrowed, &invalidated);
}

fn invalidate_written_target(
    narrowed: &mut Vec<ReadTarget>,
    events: &mut InvalidationLog,
    written: &ReadTarget,
) {
    let invalidated = targets_invalidated_by_written_target(narrowed, written);
    events.record(invalidated.clone());
    remove_invalidated(narrowed, &invalidated);
}

fn invalidate_saved_targets(narrowed: &mut Vec<ReadTarget>, events: &mut InvalidationLog) {
    let invalidated = saved_targets(narrowed);
    events.record(invalidated.clone());
    remove_invalidated(narrowed, &invalidated);
}

fn collect_statement(
    program: &CheckedProgram,
    statement: &CheckedStmt,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    match statement {
        CheckedStmt::Const {
            name,
            binding_type,
            value,
            ..
        } => {
            collect_bare_expr(program, value, narrowed, scope, recorder, events);
            scope.bind_with_type(name, binding_type.clone());
        }
        CheckedStmt::Throw { value, .. } | CheckedStmt::Expr { value, .. } => {
            collect_bare_expr(program, value, narrowed, scope, recorder, events);
        }
        CheckedStmt::Var {
            name,
            binding_type,
            value,
            ..
        } => {
            collect_optional_bare_expr(program, value.as_ref(), narrowed, scope, recorder, events);
            scope.bind_with_type(name, binding_type.clone());
        }
        CheckedStmt::Assign { target, value, .. } => {
            collect_assignment_statement(program, target, value, narrowed, scope, recorder, events);
        }
        CheckedStmt::Delete { path, .. } => {
            collect_delete_statement(program, path, narrowed, scope, recorder, events);
        }
        CheckedStmt::Return { value, .. } => {
            let context = match scope.return_presence() {
                ReturnPresence::Always => ReadContext::Bare,
                ReturnPresence::MaybePresent => ReadContext::Resolved,
            };
            collect_optional_expr(
                program,
                value.as_ref(),
                context,
                narrowed,
                scope,
                recorder,
                events,
            );
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
            recorder,
            events,
        ),
        CheckedStmt::IfConst {
            name,
            binding_type,
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => collect_if_const_statement(
            IfConstStatementParts {
                name,
                binding_type: binding_type.as_ref(),
                value,
                then_block,
                else_ifs,
                else_block: else_block.as_ref(),
            },
            program,
            narrowed,
            scope,
            recorder,
            events,
        ),
        CheckedStmt::While {
            condition, body, ..
        } => {
            collect_optional_bare_expr(
                program,
                condition.as_ref(),
                narrowed,
                scope,
                recorder,
                events,
            );
            let outer_start = narrowed.clone();
            let mut body_narrowed = outer_start.clone();
            let mut body_events = InvalidationLog::tracking(outer_start.clone());
            collect_block_with_bindings(
                program,
                body,
                &[],
                &mut body_narrowed,
                scope,
                recorder,
                &mut body_events,
            );
            events.record(body_events.invalidated.clone());
            *narrowed = outer_start;
            remove_invalidated(narrowed, &body_events.invalidated);
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
            recorder,
            events,
        ),
        CheckedStmt::Transaction { body, .. } => {
            collect_block_with_bindings(program, body, &[], narrowed, scope, recorder, events);
        }
        CheckedStmt::Try { body, catch, .. } => collect_try_statement(
            program,
            body,
            catch.as_ref(),
            narrowed,
            scope,
            recorder,
            events,
        ),
        CheckedStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_match_statement(
                program,
                scrutinee.as_ref(),
                arms,
                narrowed,
                scope,
                recorder,
                events,
            );
        }
        CheckedStmt::ReturnAbsent { .. }
        | CheckedStmt::Break { .. }
        | CheckedStmt::Continue { .. } => {}
    }
}

fn collect_bare_expr(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    collect_expr(
        program,
        expr,
        ReadContext::Bare,
        narrowed,
        scope,
        recorder,
        events,
    );
}

fn collect_optional_bare_expr(
    program: &CheckedProgram,
    expr: Option<&CheckedExpr>,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    if let Some(expr) = expr {
        collect_bare_expr(program, expr, narrowed, scope, recorder, events);
    }
}

fn collect_optional_expr(
    program: &CheckedProgram,
    expr: Option<&CheckedExpr>,
    context: ReadContext,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    if let Some(expr) = expr {
        collect_expr(program, expr, context, narrowed, scope, recorder, events);
    }
}

fn collect_assignment_statement(
    program: &CheckedProgram,
    target: &CheckedExpr,
    value: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    let assigned = super::keys::assigned_bindings(target, scope);
    let written = read_target_with_scope(program, target, scope);
    collect_write_target(program, target, narrowed, scope, recorder, events);
    collect_bare_expr(program, value, narrowed, scope, recorder, events);
    invalidate_key_bindings(narrowed, events, &assigned);
    if let Some(written) = written {
        invalidate_written_target(narrowed, events, &written);
    }
}

fn collect_delete_statement(
    program: &CheckedProgram,
    path: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    let written = read_target_with_scope(program, path, scope);
    collect_write_target(program, path, narrowed, scope, recorder, events);
    if let Some(written) = written {
        invalidate_written_target(narrowed, events, &written);
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
    program: &CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    collect_optional_bare_expr(program, parts.condition, narrowed, scope, recorder, events);
    collect_guarded_block(
        program,
        parts.condition,
        parts.then_block,
        narrowed,
        scope,
        recorder,
        events,
    );
    for else_if in parts.else_ifs {
        collect_optional_bare_expr(
            program,
            else_if.condition.as_ref(),
            narrowed,
            scope,
            recorder,
            events,
        );
        collect_guarded_block(
            program,
            else_if.condition.as_ref(),
            &else_if.block,
            narrowed,
            scope,
            recorder,
            events,
        );
    }
    if let Some(block) = parts.else_block {
        collect_block_with_bindings(program, block, &[], narrowed, scope, recorder, events);
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
    binding_type: Option<&'a MarrowType>,
    value: &'a CheckedExpr,
    then_block: &'a CheckedBody,
    else_ifs: &'a [CheckedElseIf],
    else_block: Option<&'a CheckedBody>,
}

fn collect_if_const_statement(
    parts: IfConstStatementParts<'_>,
    program: &CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    collect_expr(
        program,
        parts.value,
        ReadContext::Resolved,
        narrowed,
        scope,
        recorder,
        events,
    );
    let mut branch_narrowed = narrowed.to_vec();
    if !expr_calls_saved_writer(program, parts.value)
        && let Some(target) = read_target_with_scope(program, parts.value, scope)
    {
        super::util::extend_unique(&mut branch_narrowed, vec![target]);
    }
    let branch_start = branch_narrowed.clone();
    let binding = BlockBinding::typed(parts.name, parts.binding_type.cloned());
    collect_block_with_bindings(
        program,
        parts.then_block,
        &[binding],
        &mut branch_narrowed,
        scope,
        recorder,
        events,
    );
    events.removed(&branch_start, &branch_narrowed);
    invalidate_removed_narrowings(narrowed, &branch_start, &branch_narrowed);
    for else_if in parts.else_ifs {
        collect_optional_bare_expr(
            program,
            else_if.condition.as_ref(),
            narrowed,
            scope,
            recorder,
            events,
        );
        collect_guarded_block(
            program,
            else_if.condition.as_ref(),
            &else_if.block,
            narrowed,
            scope,
            recorder,
            events,
        );
    }
    if let Some(block) = parts.else_block {
        collect_block_with_bindings(program, block, &[], narrowed, scope, recorder, events);
    }
}

fn collect_guarded_block(
    program: &CheckedProgram,
    condition: Option<&CheckedExpr>,
    block: &CheckedBody,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    let mut branch_narrowed = narrowed.to_vec();
    if let Some(condition) = condition {
        super::util::extend_unique(
            &mut branch_narrowed,
            condition_narrowings(program, condition, scope),
        );
    }
    let branch_start = branch_narrowed.clone();
    collect_block_with_bindings(
        program,
        block,
        &[],
        &mut branch_narrowed,
        scope,
        recorder,
        events,
    );
    events.removed(&branch_start, &branch_narrowed);
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
        | CheckedStmt::ReturnAbsent { .. }
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
    program: &CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    collect_expr(
        program,
        parts.iterable,
        ReadContext::AttachedData,
        narrowed,
        scope,
        recorder,
        events,
    );
    collect_optional_bare_expr(program, parts.step, narrowed, scope, recorder, events);
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
    let mut body_events = InvalidationLog::tracking(body_start.clone());
    for statement in parts.body.statements() {
        collect_statement(
            program,
            statement,
            &mut body_narrowed,
            scope,
            recorder,
            &mut body_events,
        );
    }
    events.record(body_events.invalidated.clone());
    remove_invalidated(narrowed, &body_events.invalidated);
    invalidate_removed_narrowings(narrowed, &body_start, &body_narrowed);
    scope.pop_frame();
}

fn collect_try_statement(
    program: &CheckedProgram,
    body: &CheckedBody,
    catch: Option<&CheckedCatchClause>,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    let body_start = narrowed.clone();
    let mut body_narrowed = body_start.clone();
    let mut body_events = InvalidationLog::tracking(body_start.clone());
    collect_block_with_bindings(
        program,
        body,
        &[],
        &mut body_narrowed,
        scope,
        recorder,
        &mut body_events,
    );
    events.record(body_events.invalidated.clone());
    remove_invalidated(narrowed, &body_events.invalidated);
    if let Some(catch) = catch {
        scope.push_frame();
        scope.bind(&catch.name);
        let catch_start = narrowed.clone();
        let mut catch_narrowed = catch_start.clone();
        let mut catch_events = InvalidationLog::tracking(catch_start.clone());
        for statement in catch.block.statements() {
            collect_statement(
                program,
                statement,
                &mut catch_narrowed,
                scope,
                recorder,
                &mut catch_events,
            );
        }
        events.record(catch_events.invalidated.clone());
        remove_invalidated(narrowed, &catch_events.invalidated);
        scope.pop_frame();
    }
}

fn collect_match_statement(
    program: &CheckedProgram,
    scrutinee: Option<&CheckedExpr>,
    arms: &[CheckedMatchArm],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    collect_optional_bare_expr(program, scrutinee, narrowed, scope, recorder, events);
    for arm in arms {
        let mut arm_narrowed = narrowed.clone();
        let arm_start = arm_narrowed.clone();
        collect_block_with_bindings(
            program,
            &arm.block,
            &[],
            &mut arm_narrowed,
            scope,
            recorder,
            events,
        );
        events.removed(&arm_start, &arm_narrowed);
        invalidate_removed_narrowings(narrowed, &arm_start, &arm_narrowed);
    }
}

fn collect_write_target(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    collect_path_key_exprs(program, expr, narrowed, scope, recorder, events);
}

fn collect_expr(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    context: ReadContext,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    if let Some(read) = read_proof(program, expr, context, narrowed.as_slice(), scope) {
        record_read(program, expr, read, context, recorder);
        collect_path_key_exprs(program, expr, narrowed, scope, recorder, events);
        return;
    }
    if saved_place_key(expr, scope).is_some() {
        collect_path_key_exprs(program, expr, narrowed, scope, recorder, events);
        return;
    }
    if let CheckedExpr::Call {
        callee,
        args,
        target,
        span,
        ..
    } = expr
        && maybe_present_result(target)
    {
        if context == ReadContext::Bare {
            recorder.diagnostics.push(CheckDiagnostic::error(
                crate::CHECK_BARE_MAYBE_PRESENT_READ,
                scope.source_file(),
                *span,
                "maybe-present value must be resolved at the read site",
            ));
        }
        collect_call_expr(
            CallExprParts {
                callee,
                args,
                target,
            },
            program,
            narrowed,
            scope,
            recorder,
            events,
        );
        return;
    }

    match expr {
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            collect_call_expr(
                CallExprParts {
                    callee,
                    args,
                    target,
                },
                program,
                narrowed,
                scope,
                recorder,
                events,
            );
        }
        CheckedExpr::Field { base, .. } => {
            collect_bare_expr(program, base, narrowed, scope, recorder, events);
        }
        CheckedExpr::OptionalField { base, .. } => collect_expr(
            program,
            base,
            ReadContext::Resolved,
            narrowed,
            scope,
            recorder,
            events,
        ),
        CheckedExpr::Unary { operand, .. } => {
            collect_bare_expr(program, operand, narrowed, scope, recorder, events);
        }
        CheckedExpr::Binary {
            op, left, right, ..
        } => {
            collect_binary_expr(
                BinaryExprParts {
                    op: *op,
                    left,
                    right,
                },
                program,
                narrowed,
                scope,
                recorder,
                events,
            );
        }
        CheckedExpr::Range {
            start, end, step, ..
        } => {
            for part in [start.as_deref(), end.as_deref(), step.as_deref()]
                .into_iter()
                .flatten()
            {
                collect_bare_expr(program, part, narrowed, scope, recorder, events);
            }
        }
        CheckedExpr::Interpolation { parts, .. } => {
            collect_interpolation_expr(program, parts, narrowed, scope, recorder, events);
        }
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {}
    }
}

#[derive(Clone, Copy)]
struct CallExprParts<'a> {
    callee: &'a CheckedExpr,
    args: &'a [CheckedArg],
    target: &'a CheckedCallTarget,
}

fn collect_call_expr(
    parts: CallExprParts<'_>,
    program: &CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    if matches!(parts.target, CheckedCallTarget::IdentityConstructor(_)) {
        collect_args(
            program,
            &parts.args[parts.args.len().min(1)..],
            ReadContext::Bare,
            narrowed,
            scope,
            recorder,
            events,
        );
        return;
    }
    match builtin_of(parts.target) {
        Some(CheckedBuiltinCall::Exists) => {
            collect_exists_args(program, parts.args, narrowed, scope, recorder, events);
        }
        Some(CheckedBuiltinCall::Append) => {
            collect_append_args(program, parts.args, narrowed, scope, recorder, events);
        }
        Some(builtin) if builtin.reads_attached_data() => {
            collect_args(
                program,
                parts.args,
                ReadContext::AttachedData,
                narrowed,
                scope,
                recorder,
                events,
            );
        }
        _ => collect_plain_call(parts, program, narrowed, scope, recorder, events),
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
    program: &CheckedProgram,
    args: &[CheckedArg],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    if let Some(arg) = args.first() {
        collect_expr(
            program,
            &arg.value,
            ReadContext::Resolved,
            narrowed,
            scope,
            recorder,
            events,
        );
    }
    collect_args(
        program,
        &args[args.len().min(1)..],
        ReadContext::Bare,
        narrowed,
        scope,
        recorder,
        events,
    );
}

fn collect_append_args(
    program: &CheckedProgram,
    args: &[CheckedArg],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    if let Some(target) = args.first() {
        collect_write_target(program, &target.value, narrowed, scope, recorder, events);
    }
    collect_args(
        program,
        &args[args.len().min(1)..],
        ReadContext::Bare,
        narrowed,
        scope,
        recorder,
        events,
    );
}

fn collect_plain_call(
    parts: CallExprParts<'_>,
    program: &CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    let writes_saved = call_writes_saved_data(program, parts.target);
    collect_bare_expr(program, parts.callee, narrowed, scope, recorder, events);
    if let Some(path_args) = std_path_arg_mask(parts.target) {
        collect_std_args(
            program, parts.args, &path_args, narrowed, scope, recorder, events,
        );
    } else {
        collect_args(
            program,
            parts.args,
            ReadContext::Bare,
            narrowed,
            scope,
            recorder,
            events,
        );
    }
    if writes_saved {
        invalidate_saved_targets(narrowed, events);
    }
}

fn collect_args(
    program: &CheckedProgram,
    args: &[CheckedArg],
    context: ReadContext,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    for arg in args {
        collect_expr(
            program, &arg.value, context, narrowed, scope, recorder, events,
        );
    }
}

struct BinaryExprParts<'a> {
    op: CheckedBinaryOp,
    left: &'a CheckedExpr,
    right: &'a CheckedExpr,
}

fn collect_binary_expr(
    parts: BinaryExprParts<'_>,
    program: &CheckedProgram,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    let left_context = if parts.op == CheckedBinaryOp::Coalesce {
        ReadContext::Resolved
    } else {
        ReadContext::Bare
    };
    collect_expr(
        program,
        parts.left,
        left_context,
        narrowed,
        scope,
        recorder,
        events,
    );
    collect_bare_expr(program, parts.right, narrowed, scope, recorder, events);
}

fn collect_interpolation_expr(
    program: &CheckedProgram,
    parts: &[CheckedInterpolationPart],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    for part in parts {
        if let CheckedInterpolationPart::Expr(expr) = part {
            collect_bare_expr(program, expr, narrowed, scope, recorder, events);
        }
    }
}

fn collect_path_key_exprs(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    match expr {
        CheckedExpr::Call { target, args, .. }
            if builtin_of(target).is_some_and(CheckedBuiltinCall::is_neighbor_read) =>
        {
            if let Some(read) = read_proof(program, expr, ReadContext::Resolved, narrowed, scope) {
                record_read(program, expr, read, ReadContext::Resolved, recorder);
            }
            if let Some(arg) = args.first() {
                collect_path_key_exprs(program, &arg.value, narrowed, scope, recorder, events);
            }
            collect_args(
                program,
                &args[args.len().min(1)..],
                ReadContext::Bare,
                narrowed,
                scope,
                recorder,
                events,
            );
        }
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            collect_path_key_exprs(program, callee, narrowed, scope, recorder, events);
            if let Some(path_args) = std_path_arg_mask(target) {
                collect_std_args(program, args, &path_args, narrowed, scope, recorder, events);
            } else {
                collect_args(
                    program,
                    args,
                    ReadContext::Bare,
                    narrowed,
                    scope,
                    recorder,
                    events,
                );
            }
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            collect_path_key_exprs(program, base, narrowed, scope, recorder, events);
        }
        CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::SavedRoot { .. }
        | CheckedExpr::Unary { .. }
        | CheckedExpr::Binary { .. }
        | CheckedExpr::Range { .. }
        | CheckedExpr::Interpolation { .. } => {}
    }
}

fn collect_std_args(
    program: &CheckedProgram,
    args: &[CheckedArg],
    path_args: &[bool],
    narrowed: &mut Vec<ReadTarget>,
    scope: &mut NameScope,
    recorder: &mut PresenceRecorder<'_>,
    events: &mut InvalidationLog,
) {
    for (index, arg) in args.iter().enumerate() {
        let context = if path_args.get(index).copied().unwrap_or(false)
            && saved_path_read_target_with_scope(program, &arg.value, scope).is_some()
        {
            ReadContext::AttachedData
        } else {
            ReadContext::Bare
        };
        collect_expr(
            program, &arg.value, context, narrowed, scope, recorder, events,
        );
    }
}
