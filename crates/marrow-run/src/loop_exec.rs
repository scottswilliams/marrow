//! Checked loop, range, and collection iteration.

use std::cmp::Ordering;
use std::ops::ControlFlow;

use marrow_check::{
    CheckedBody as ExecBody, CheckedExpr as ExecExpr, CheckedForBinding as ForBinding,
};
use marrow_store::value::NANOS_PER_DAY;
use marrow_syntax::SourceSpan;

use crate::collection::{Direction, durable_collection_value, peel_reversed, values_or_entries};
use crate::env::{Env, Flow};
use crate::error::{RuntimeError, overflow, type_error, unsupported};
use crate::exec::eval_block;
use crate::expr::{eval_condition, eval_expr};
use crate::local_collection::{
    enumerate_local_keys_call_arg, enumerate_reversed_local_keys_call_arg,
    materialize_local_collection_dir,
};
use crate::range_expr::checked_range;
use crate::read::keys_argument;
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
    if let Some(second) = binding.second.as_deref() {
        return eval_two_name_for(&binding.first, second, iterable, body, span, env);
    }

    if is_range_expr(iterable) {
        return eval_range_for(binding, iterable, step, body, span, env);
    }

    eval_single_name_collection_for(binding, iterable, body, span, env)
}

fn eval_two_name_for(
    first: &str,
    second: &str,
    iterable: &ExecExpr,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if is_range_expr(iterable) {
        return Err(unsupported("a two-name binding over a range", span));
    }
    if let Some(saved_loop) = SavedLoopSpec::from_iterable(iterable, true) {
        return saved_loop.run(env, &mut |row, env| {
            // A two-name head always requests `Entries`, which only ever yields pair
            // rows; the `else` guards that shape contract rather than reachable input.
            let SavedLoopRow::Pair(key, value) = row else {
                return Err(non_pair_iterable(span));
            };
            loop_step_flow(run_two_name_body(first, second, key, value, body, env)?)
        });
    }
    let entries = eval_collection_entry_rows(iterable, span, env)?;
    for (key, value) in entries {
        match run_two_name_body(first, second, key, value, body, env)? {
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
    checked_range(iterable).is_some()
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
    let Some(range) = checked_range(iterable) else {
        return Err(unsupported("iterating this value", iterable.span()));
    };
    match (range.start, range.end) {
        (Some(start), Some(end)) => Ok((start, end, range.inclusive_end)),
        _ => Err(unsupported("iterating this value", iterable.span())),
    }
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
        make: date_from_days,
    })
}

/// Wrap a range cursor back into a date. The cursor advances in `i64` space but
/// `in_range` clamps it within the two `i32` endpoints, so every yielded day fits
/// `i32`; the checked conversion makes that bound explicit rather than truncating.
fn date_from_days(days: i64) -> Value {
    let day = i32::try_from(days).expect("date range cursor stays within its i32 endpoints");
    Value::Date(day)
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

/// What a single-name local-collection loop binds. A local collection streams its
/// keys: a local sequence is a 1-based integer-keyed tree, so `for x in xs` binds
/// its positions exactly as a keyed tree binds its keys and a saved sequence binds
/// its positions, with `reversed` walking those keys descending. `values(...)` is
/// the explicit value view, materialized to its element sequence.
pub(crate) fn eval_collection(
    iterable: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let (inner, direction) = peel_reversed(iterable);
    if is_value_view(inner) {
        return match eval_expr(inner, env)? {
            Value::Sequence(items) => {
                let mut values = items.into_values();
                if direction == Direction::Descending {
                    values.reverse();
                }
                Ok(values)
            }
            _ => Err(unsupported("iterating this value", iterable.span())),
        };
    }
    let layer = keys_argument(inner).unwrap_or(inner);
    let keys = match direction {
        Direction::Ascending => enumerate_local_keys_call_arg(layer, iterable.span(), env)?,
        Direction::Descending => {
            enumerate_reversed_local_keys_call_arg(layer, iterable.span(), env)?
        }
    };
    keys.ok_or_else(|| durable_collection_value(iterable.span()))
}

/// Whether `iterable` is an explicit value view — `values(...)` — that materializes
/// element values rather than keys. The caller peels any `reversed` wrapper first.
fn is_value_view(iterable: &ExecExpr) -> bool {
    matches!(
        values_or_entries(iterable),
        Some(call) if matches!(call.kind, crate::collection::MaterializeKind::Values)
    )
}

/// A two-name head materializes pair rows from a local keyed collection. Every
/// `reversed(...)` wrapper composes by parity into the walk direction, so the base
/// keyed collection is resolved directly rather than through a collapsed value —
/// keeping `reversed(reversed(x)) == x` and every key paired with its own value.
fn eval_collection_entry_rows(
    iterable: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    let (inner, direction) = peel_reversed(iterable);
    let layer = match values_or_entries(inner) {
        Some(call) if matches!(call.kind, crate::collection::MaterializeKind::Entries) => {
            call.layer
        }
        Some(_) => return Err(non_pair_iterable(span)),
        None => inner,
    };
    if layer.saved_place().is_some() {
        return Err(durable_collection_value(iterable.span()));
    }
    match eval_expr(layer, env)? {
        value @ (Value::LocalTree(_) | Value::Sequence(_)) => {
            materialize_local_collection_dir(value, direction, iterable.span())
        }
        _ => Err(non_pair_iterable(span)),
    }
}

/// A two-name binding requires pair rows; a scalar iterable has no keys to bind, so
/// the head must wrap it in `entries(...)`.
fn non_pair_iterable(span: SourceSpan) -> RuntimeError {
    unsupported(
        "a two-name binding over a non-pair iterable (use entries(...))",
        span,
    )
}
