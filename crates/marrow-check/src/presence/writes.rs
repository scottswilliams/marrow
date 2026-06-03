use crate::{
    CheckedBody, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedFunction,
    CheckedFunctionRef, CheckedInterpolationPart, CheckedProgram, CheckedStmt,
};

pub(super) fn call_writes_saved_data(program: &CheckedProgram, target: &CheckedCallTarget) -> bool {
    call_target_writes_saved_data(program, target, &mut Vec::new())
}

fn call_target_writes_saved_data(
    program: &CheckedProgram,
    target: &CheckedCallTarget,
    visiting: &mut Vec<CheckedFunctionRef>,
) -> bool {
    match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Append) => true,
        CheckedCallTarget::Function(function_ref) => {
            function_ref_writes_saved_data(program, *function_ref, visiting)
        }
        CheckedCallTarget::SavedIndexLookup
        | CheckedCallTarget::SavedLayerRead
        | CheckedCallTarget::SavedResourceRead
        | CheckedCallTarget::ErrorConstructor
        | CheckedCallTarget::Builtin(_)
        | CheckedCallTarget::Std(_)
        | CheckedCallTarget::ResourceConstructor(_)
        | CheckedCallTarget::LocalCollection { .. } => false,
    }
}

fn function_ref_writes_saved_data(
    program: &CheckedProgram,
    function_ref: CheckedFunctionRef,
    visiting: &mut Vec<CheckedFunctionRef>,
) -> bool {
    if visiting.contains(&function_ref) {
        return false;
    }
    let Some(function) = function_by_ref(program, function_ref) else {
        return false;
    };
    visiting.push(function_ref);
    let writes = function_directly_writes_saved_data(program, function_ref, function)
        || function
            .runtime_body()
            .is_some_and(|body| block_calls_saved_writer(program, body, visiting));
    visiting.pop();
    writes
}

fn function_directly_writes_saved_data(
    program: &CheckedProgram,
    function_ref: CheckedFunctionRef,
    function: &CheckedFunction,
) -> bool {
    let module = crate::facts::ModuleId(function_ref.module);
    program
        .facts
        .function_id(module, &function.name)
        .is_some_and(|id| {
            !program
                .facts
                .function(id)
                .direct_effects
                .saved_writes
                .is_empty()
        })
}

fn function_by_ref(
    program: &CheckedProgram,
    function_ref: CheckedFunctionRef,
) -> Option<&CheckedFunction> {
    program
        .modules
        .get(function_ref.module as usize)?
        .functions
        .get(function_ref.function as usize)
}

fn block_calls_saved_writer(
    program: &CheckedProgram,
    body: &CheckedBody,
    visiting: &mut Vec<CheckedFunctionRef>,
) -> bool {
    body.statements()
        .iter()
        .any(|statement| statement_calls_saved_writer(program, statement, visiting))
}

fn statement_calls_saved_writer(
    program: &CheckedProgram,
    statement: &CheckedStmt,
    visiting: &mut Vec<CheckedFunctionRef>,
) -> bool {
    match statement {
        CheckedStmt::Const { value, .. }
        | CheckedStmt::Var {
            value: Some(value), ..
        }
        | CheckedStmt::Throw { value, .. }
        | CheckedStmt::Expr { value, .. } => expr_calls_saved_writer(program, value, visiting),
        CheckedStmt::Var { value: None, .. } => false,
        CheckedStmt::Assign { target, value, .. } => {
            expr_calls_saved_writer(program, target, visiting)
                || expr_calls_saved_writer(program, value, visiting)
        }
        CheckedStmt::Delete { path, .. } => expr_calls_saved_writer(program, path, visiting),
        CheckedStmt::Return { value, .. } => value
            .as_ref()
            .is_some_and(|value| expr_calls_saved_writer(program, value, visiting)),
        CheckedStmt::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            condition
                .as_ref()
                .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
                || block_calls_saved_writer(program, then_block, visiting)
                || else_ifs.iter().any(|else_if| {
                    else_if.condition.as_ref().is_some_and(|condition| {
                        expr_calls_saved_writer(program, condition, visiting)
                    }) || block_calls_saved_writer(program, &else_if.block, visiting)
                })
                || else_block
                    .as_ref()
                    .is_some_and(|block| block_calls_saved_writer(program, block, visiting))
        }
        CheckedStmt::While {
            condition, body, ..
        } => {
            condition
                .as_ref()
                .is_some_and(|condition| expr_calls_saved_writer(program, condition, visiting))
                || block_calls_saved_writer(program, body, visiting)
        }
        CheckedStmt::For {
            iterable,
            step,
            body,
            ..
        } => {
            expr_calls_saved_writer(program, iterable, visiting)
                || step
                    .as_ref()
                    .is_some_and(|step| expr_calls_saved_writer(program, step, visiting))
                || block_calls_saved_writer(program, body, visiting)
        }
        CheckedStmt::Transaction { body, .. } => block_calls_saved_writer(program, body, visiting),
        CheckedStmt::Try {
            body,
            catch,
            finally,
            ..
        } => {
            block_calls_saved_writer(program, body, visiting)
                || catch
                    .as_ref()
                    .is_some_and(|catch| block_calls_saved_writer(program, &catch.block, visiting))
                || finally
                    .as_ref()
                    .is_some_and(|finally| block_calls_saved_writer(program, finally, visiting))
        }
        CheckedStmt::Match {
            scrutinee, arms, ..
        } => {
            scrutinee
                .as_ref()
                .is_some_and(|scrutinee| expr_calls_saved_writer(program, scrutinee, visiting))
                || arms
                    .iter()
                    .any(|arm| block_calls_saved_writer(program, &arm.block, visiting))
        }
        CheckedStmt::Break { .. } | CheckedStmt::Continue { .. } => false,
    }
}

pub(super) fn expr_calls_saved_writer(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    visiting: &mut Vec<CheckedFunctionRef>,
) -> bool {
    match expr {
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            call_target_writes_saved_data(program, target, visiting)
                || expr_calls_saved_writer(program, callee, visiting)
                || args
                    .iter()
                    .any(|arg| expr_calls_saved_writer(program, &arg.value, visiting))
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            expr_calls_saved_writer(program, base, visiting)
        }
        CheckedExpr::Unary { operand, .. } => expr_calls_saved_writer(program, operand, visiting),
        CheckedExpr::Binary { left, right, .. } => {
            expr_calls_saved_writer(program, left, visiting)
                || expr_calls_saved_writer(program, right, visiting)
        }
        CheckedExpr::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            CheckedInterpolationPart::Text { .. } => false,
            CheckedInterpolationPart::Expr(expr) => {
                expr_calls_saved_writer(program, expr, visiting)
            }
        }),
        CheckedExpr::Literal { .. } | CheckedExpr::Name { .. } | CheckedExpr::SavedRoot { .. } => {
            false
        }
    }
}
