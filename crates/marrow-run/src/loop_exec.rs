//! Checked loop, range, and collection iteration.

use std::cmp::Ordering;
use std::ops::ControlFlow;

use marrow_check::{
    CheckedBinaryOp as BinaryOp, CheckedBody as ExecBody, CheckedExpr as ExecExpr,
    CheckedForBinding as ForBinding,
};
use marrow_store::value::NANOS_PER_DAY;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, durable_collection_value, values_or_entries};
use crate::env::{Env, Flow};
use crate::error::{RuntimeError, overflow, type_error, unsupported};
use crate::exec::eval_block;
use crate::expr::{eval_condition, eval_expr};
use crate::local_collection::{enumerate_local_collection_dir, materialize_local_collection_dir};
use crate::read::{keys_argument, reversed_argument};
use crate::saved_iter::{SavedLoopRow, SavedLoopSpec};
use crate::value::Value;

pub(crate) enum LoopStep {
    Iterate,
    Stop,
    Propagate(Flow),
}

pub(crate) fn classify(flow: Flow) -> LoopStep {
    match flow {
        Flow::Normal => LoopStep::Iterate,
        Flow::Continue => LoopStep::Iterate,
        Flow::Break => LoopStep::Stop,
        other => LoopStep::Propagate(other),
    }
}

pub(crate) fn eval_while(
    condition: Option<&ExecExpr>,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    while eval_condition(condition, span, env)? {
        match classify(eval_block(body, env)?) {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

pub(crate) fn eval_for(
    binding: &ForBinding,
    iterable: &ExecExpr,
    step: Option<&ExecExpr>,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if binding.second.is_some() {
        return eval_two_name_for(binding, iterable, body, span, env);
    }

    if is_range_expr(iterable) {
        return eval_range_for(binding, iterable, step, body, span, env);
    }

    eval_single_name_collection_for(binding, iterable, body, span, env)
}

fn eval_two_name_for(
    binding: &ForBinding,
    iterable: &ExecExpr,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let second = binding
        .second
        .as_ref()
        .expect("two-name loop helper only receives two-name bindings");
    if is_range_expr(iterable) {
        return Err(unsupported("a two-name binding over a range", span));
    }
    if let Some(saved_loop) = SavedLoopSpec::from_iterable(iterable, true) {
        return saved_loop.run(env, &mut |row, env| {
            let SavedLoopRow::Pair(key, value) = row else {
                return Err(unsupported(
                    "a two-name binding over a non-pair iterable (use entries(...))",
                    span,
                ));
            };
            loop_step_flow(run_two_name_body(
                &binding.first,
                second,
                key,
                value,
                body,
                env,
            )?)
        });
    }
    let entries = eval_collection_entry_rows(iterable, span, env)?;
    for (key, value) in entries {
        match run_two_name_body(&binding.first, second, key, value, body, env)? {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

fn eval_single_name_collection_for(
    binding: &ForBinding,
    iterable: &ExecExpr,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if let Some(saved_loop) = SavedLoopSpec::from_iterable(iterable, false) {
        return saved_loop.run(env, &mut |row, env| {
            let value = match row {
                SavedLoopRow::Single(value) => value,
                SavedLoopRow::Pair(_, _) => {
                    return Err(unsupported(
                        "entries(...) is only valid in a two-name loop head",
                        span,
                    ));
                }
            };
            loop_step_flow(run_single_name_body(&binding.first, value, body, env)?)
        });
    }
    let values = eval_collection(iterable, env)?;
    for value in values {
        match run_single_name_body(&binding.first, value, body, env)? {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

fn eval_range_for(
    binding: &ForBinding,
    iterable: &ExecExpr,
    step: Option<&ExecExpr>,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let mut range = range_iter(iterable, step, span, env)?;
    while let Some(value) = range.next_value(span)? {
        match run_single_name_body(&binding.first, value, body, env)? {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

fn is_range_expr(iterable: &ExecExpr) -> bool {
    matches!(
        iterable,
        ExecExpr::Binary {
            op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
            ..
        }
    )
}

fn run_single_name_body(
    first: &str,
    value: Value,
    body: &ExecBody,
    env: &mut Env<'_>,
) -> Result<LoopStep, RuntimeError> {
    env.push_scope();
    env.bind(first.to_string(), value, false);
    let flow = eval_block(body, env);
    env.pop_scope();
    Ok(classify(flow?))
}

fn run_two_name_body(
    first: &str,
    second: &str,
    key: Value,
    value: Value,
    body: &ExecBody,
    env: &mut Env<'_>,
) -> Result<LoopStep, RuntimeError> {
    env.push_scope();
    env.bind(first.to_string(), key, false);
    env.bind(second.to_string(), value, false);
    let flow = eval_block(body, env);
    env.pop_scope();
    Ok(classify(flow?))
}

fn loop_step_flow(step: LoopStep) -> Result<ControlFlow<Flow>, RuntimeError> {
    Ok(match step {
        LoopStep::Iterate => ControlFlow::Continue(()),
        LoopStep::Stop => ControlFlow::Break(Flow::Normal),
        LoopStep::Propagate(flow) => ControlFlow::Break(flow),
    })
}

enum RangeIter {
    Integer {
        current: i64,
        end: i64,
        inclusive: bool,
        step: i64,
        make: fn(i64) -> Value,
    },
    Instant {
        current: i128,
        end: i128,
        inclusive: bool,
        step: i128,
    },
}

impl RangeIter {
    fn next_value(&mut self, span: SourceSpan) -> Result<Option<Value>, RuntimeError> {
        match self {
            RangeIter::Integer {
                current,
                end,
                inclusive,
                step,
                make,
            } => {
                if !in_range(*current, *end, *inclusive, step_direction((*step).cmp(&0))) {
                    return Ok(None);
                }
                let value = make(*current);
                match current.checked_add(*step) {
                    Some(next) => *current = next,
                    None => *step = 0,
                }
                Ok(Some(value))
            }
            RangeIter::Instant {
                current,
                end,
                inclusive,
                step,
            } => {
                if !in_range(*current, *end, *inclusive, step_direction((*step).cmp(&0))) {
                    return Ok(None);
                }
                let value = Value::Instant(*current);
                *current = current.checked_add(*step).ok_or_else(|| overflow(span))?;
                Ok(Some(value))
            }
        }
    }
}

/// A zero step never advances, so it yields the empty range.
enum RangeDirection {
    Ascending,
    Descending,
    Empty,
}

fn step_direction(sign: Ordering) -> RangeDirection {
    match sign {
        Ordering::Greater => RangeDirection::Ascending,
        Ordering::Less => RangeDirection::Descending,
        Ordering::Equal => RangeDirection::Empty,
    }
}

fn in_range<T: Ord>(current: T, end: T, inclusive: bool, direction: RangeDirection) -> bool {
    match direction {
        RangeDirection::Ascending if inclusive => current <= end,
        RangeDirection::Ascending => current < end,
        RangeDirection::Descending if inclusive => current >= end,
        RangeDirection::Descending => current > end,
        RangeDirection::Empty => false,
    }
}

fn range_iter(
    iterable: &ExecExpr,
    step: Option<&ExecExpr>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<RangeIter, RuntimeError> {
    let (left, right, inclusive) = range_bounds(iterable)?;
    let start = eval_expr(left, env)?;
    let end = eval_expr(right, env)?;
    let step = step.map(|expr| eval_expr(expr, env)).transpose()?;
    match (start, end) {
        (Value::Int(start), Value::Int(end)) => int_range_iter(start, end, step, inclusive, span),
        (Value::Date(start), Value::Date(end)) => {
            date_range_iter(start, end, step, inclusive, span)
        }
        (Value::Instant(start), Value::Instant(end)) => {
            instant_range_iter(start, end, step, inclusive, span)
        }
        _ => Err(type_error(
            "a range needs two endpoints of the same type",
            span,
        )),
    }
}

fn range_bounds(iterable: &ExecExpr) -> Result<(&ExecExpr, &ExecExpr, bool), RuntimeError> {
    Ok(match iterable {
        ExecExpr::Binary {
            op: BinaryOp::RangeExclusive,
            left,
            right,
            ..
        } => (left, right, false),
        ExecExpr::Binary {
            op: BinaryOp::RangeInclusive,
            left,
            right,
            ..
        } => (left, right, true),
        other => return Err(unsupported("iterating this value", other.span())),
    })
}

fn int_range_iter(
    start: i64,
    end: i64,
    step: Option<Value>,
    inclusive: bool,
    span: SourceSpan,
) -> Result<RangeIter, RuntimeError> {
    let step = match step {
        Some(Value::Int(n)) => n,
        Some(_) => return Err(type_error("an int range steps by an int", span)),
        None => 1,
    };
    nonzero_range_step(step, span)?;
    Ok(RangeIter::Integer {
        current: start,
        end,
        inclusive,
        step,
        make: Value::Int,
    })
}

fn date_range_iter(
    start: i32,
    end: i32,
    step: Option<Value>,
    inclusive: bool,
    span: SourceSpan,
) -> Result<RangeIter, RuntimeError> {
    let step = match step {
        Some(Value::Duration(nanos)) => duration_whole_days(nanos, span)?,
        Some(_) => return Err(type_error("a date range steps by a duration", span)),
        None => 1,
    };
    nonzero_range_step(step, span)?;
    Ok(RangeIter::Integer {
        current: i64::from(start),
        end: i64::from(end),
        inclusive,
        step,
        make: |days| Value::Date(days as i32),
    })
}

fn instant_range_iter(
    start: i128,
    end: i128,
    step: Option<Value>,
    inclusive: bool,
    span: SourceSpan,
) -> Result<RangeIter, RuntimeError> {
    let step = match step {
        Some(Value::Duration(nanos)) => nanos,
        Some(_) => return Err(type_error("an instant range steps by a duration", span)),
        None => {
            return Err(type_error(
                "an instant range needs an explicit `by` step",
                span,
            ));
        }
    };
    if step == 0 {
        return Err(type_error("a range step cannot be zero", span));
    }
    Ok(RangeIter::Instant {
        current: start,
        end,
        inclusive,
        step,
    })
}

fn nonzero_range_step(step: i64, span: SourceSpan) -> Result<(), RuntimeError> {
    if step == 0 {
        return Err(type_error("a range step cannot be zero", span));
    }
    Ok(())
}

fn duration_whole_days(nanos: i128, span: SourceSpan) -> Result<i64, RuntimeError> {
    if nanos % NANOS_PER_DAY != 0 {
        return Err(type_error(
            "a date range step must be a whole number of days",
            span,
        ));
    }
    i64::try_from(nanos / NANOS_PER_DAY).map_err(|_| overflow(span))
}

pub(crate) fn eval_collection(
    iterable: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    if let Some(inner) = reversed_argument(iterable) {
        if let Some(layer) = keys_argument(inner) {
            if layer.saved_place().is_none() {
                return enumerate_local_collection_dir(
                    eval_expr(layer, env)?,
                    Direction::Descending,
                    iterable.span(),
                );
            }
            return Err(durable_collection_value(iterable.span()));
        }
        if inner.saved_place().is_some() {
            return Err(durable_collection_value(iterable.span()));
        }
    }
    if let Some(path) = keys_argument(iterable) {
        if path.saved_place().is_none() {
            return enumerate_local_collection_dir(
                eval_expr(path, env)?,
                Direction::Ascending,
                iterable.span(),
            );
        }
        return Err(durable_collection_value(iterable.span()));
    }
    if iterable.saved_place().is_some() {
        return Err(durable_collection_value(iterable.span()));
    }
    match eval_expr(iterable, env)? {
        Value::Sequence(items) => Ok(items),
        Value::LocalTree(entries) => Ok(entries.into_iter().map(|entry| entry.value).collect()),
        _ => Err(unsupported("iterating this value", iterable.span())),
    }
}

fn eval_collection_entry_rows(
    iterable: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    if let Some(inner) = reversed_argument(iterable)
        && let Some(entries) = values_or_entries(inner)
        && matches!(&entries.kind, crate::collection::MaterializeKind::Entries)
    {
        if entries.layer.saved_place().is_none() {
            return materialize_local_collection_dir(
                eval_expr(entries.layer, env)?,
                Direction::Descending,
                iterable.span(),
            );
        }
        return Err(durable_collection_value(iterable.span()));
    }
    if let Some(inner) = reversed_argument(iterable)
        && inner.saved_place().is_some()
        && keys_argument(inner).is_none()
    {
        return Err(durable_collection_value(iterable.span()));
    }
    if let Some(inner) = values_or_entries(iterable)
        && matches!(&inner.kind, crate::collection::MaterializeKind::Entries)
    {
        if inner.layer.saved_place().is_none() {
            return materialize_local_collection_dir(
                eval_expr(inner.layer, env)?,
                Direction::Ascending,
                iterable.span(),
            );
        }
        return Err(durable_collection_value(iterable.span()));
    }
    if iterable.saved_place().is_some() {
        return Err(durable_collection_value(iterable.span()));
    }
    match eval_expr(iterable, env)? {
        value @ Value::LocalTree(_) => {
            materialize_local_collection_dir(value, Direction::Ascending, span)
        }
        _ => Err(unsupported(
            "a two-name binding over a non-pair iterable (use entries(...))",
            span,
        )),
    }
}
