//! Checked block and match execution.

use marrow_check::{
    CheckedBody as ExecBody, CheckedEnumRef, CheckedExpr as ExecExpr,
    CheckedMatchArm as ExecMatchArm, CheckedStmt as ExecStmt,
};
use marrow_syntax::SourceSpan;

use crate::env::{Env, Flow};
use crate::error::{RuntimeError, type_error, unsupported};
use crate::expr::eval_expr;
use crate::value::{Value, enum_id_from_ref};

/// The scope is popped on every exit, including the error path, so the
/// environment stays balanced for reuse.
pub(crate) fn eval_block(block: &ExecBody, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    env.push_scope();
    let result = eval_statements(block.statements(), env);
    env.pop_scope();
    result
}

pub(crate) fn eval_statements(
    statements: &[ExecStmt],
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    for statement in statements {
        let flow = crate::statement::eval_statement(statement, env)?;
        if !matches!(flow, Flow::Normal) {
            return Ok(flow);
        }
    }
    Ok(Flow::Normal)
}

/// The checker proves a covering arm exists, so a missing match is a defensive
/// fault, not a reachable path.
pub(crate) fn eval_match(
    scrutinee: Option<&ExecExpr>,
    arms: &[ExecMatchArm],
    enum_ref: Option<CheckedEnumRef>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let scrutinee = scrutinee.ok_or_else(|| unsupported("a match with no scrutinee", span))?;
    let Value::Enum(value) = eval_expr(scrutinee, env)? else {
        return Err(type_error("`match` requires an enum value", span));
    };
    let enum_id = enum_ref
        .map(enum_id_from_ref)
        .ok_or_else(|| unsupported("a match over a non-enum value", span))?;
    if value.enum_id != enum_id {
        return Err(type_error("`match` requires this enum value", span));
    }
    for arm in arms {
        if let Some(arm_member_id) = arm.member_id
            && let Some(arm_member) = env.program.facts().enum_member(arm_member_id)
            && arm_member.enum_id == enum_id
            && env
                .program
                .facts()
                .enum_member_is_descendant(value.member_id, arm_member_id)
        {
            return eval_block(&arm.block, env);
        }
    }
    Err(unsupported("a match with no arm for this enum value", span))
}

/// Saved paths and qualified names are dispatched before reaching here, so only
/// a single local name is a valid target.
pub(crate) fn local_target(target: &ExecExpr, span: SourceSpan) -> Result<&str, RuntimeError> {
    match target {
        ExecExpr::Name { segments, .. } if segments.len() == 1 => Ok(&segments[0]),
        _ => Err(unsupported("assignment to this target", span)),
    }
}
