use marrow_check::{CheckedBuiltinCall, CheckedCallTarget, CheckedExpr as ExecExpr};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::RuntimeError;
use crate::expr::eval_expr;
use crate::local_collection::materialize_local_collection_dir;
use crate::stdlib::check_key_collection;
use crate::value::Value;

use super::{Direction, durable_collection_value};

pub(crate) enum MaterializeKind {
    Values,
    Entries,
}

pub(crate) struct ValuesOrEntries<'a> {
    pub(crate) layer: &'a ExecExpr,
    pub(crate) kind: MaterializeKind,
}

pub(crate) fn values_or_entries(expr: &ExecExpr) -> Option<ValuesOrEntries<'_>> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    let kind = match target {
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Values) => MaterializeKind::Values,
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Entries) => MaterializeKind::Entries,
        _ => return None,
    };
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(ValuesOrEntries {
            layer: &arg.value,
            kind,
        }),
        _ => None,
    }
}

pub(crate) fn reversed_materialized(
    inner: ValuesOrEntries<'_>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if inner.layer.saved_place().is_some() {
        return Err(durable_collection_value(span));
    }
    let rows = materialize_local_collection_dir(
        eval_expr(inner.layer, env)?,
        Direction::Descending,
        span,
    )?;
    let values = match inner.kind {
        MaterializeKind::Values => rows.into_iter().map(|(_, value)| value).collect(),
        MaterializeKind::Entries => rows
            .into_iter()
            .map(|(key, value)| Value::Sequence(vec![key, value]))
            .collect(),
    };
    Ok(Value::Sequence(values))
}

pub(crate) fn reversed_keys(
    layer: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if layer.saved_place().is_none() {
        return Ok(Value::Sequence(
            crate::local_collection::enumerate_local_collection_dir(
                eval_expr(layer, env)?,
                Direction::Descending,
                span,
            )?,
        ));
    }
    check_key_collection(layer, span)?;
    Err(durable_collection_value(span))
}
