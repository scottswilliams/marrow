//! Checked loop, range, and collection iteration.

use std::cmp::Ordering;
use std::ops::ControlFlow;

use marrow_check::{
    CheckedBody as ExecBody, CheckedExpr as ExecExpr, CheckedForBinding as ForBinding, LoopOrder,
};
use marrow_store::value::NANOS_PER_DAY;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::collection::{
    durable_collection_value, enumerate_local_keys_call_arg,
    enumerate_reversed_local_keys_call_arg, materialize_local_collection_dir,
};
use crate::env::{Env, Flow};
use crate::error::{RuntimeError, overflow, type_error, unsupported};
use crate::exec::eval_block;
use crate::expr::{eval_condition, eval_expr};
use crate::range_expr::checked_range;
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
    order: LoopOrder,
    iterable: &ExecExpr,
    step: Option<&ExecExpr>,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if is_range_expr(iterable) {
        // A range's direction is spelled with its endpoints and `by`; the checker
        // rejects `reversed` over a range, so a range head is always forward here.
        return eval_range_for(binding, iterable, step, body, span, env);
    }
    let dir = match order {
        LoopOrder::Forward => Direction::Ascending,
        LoopOrder::Reversed => Direction::Descending,
    };
    // A single binding is key-first (one streamed key column, no value); an
    // (n+1)-name head binds every remaining key column plus the leaf value.
    let names = &binding.names;
    let with_value = names.len() >= 2;
    let key_columns = if names.len() == 1 { 1 } else { names.len() - 1 };
    if let Some(saved_loop) = SavedLoopSpec::from_head(iterable, key_columns, with_value, dir) {
        return saved_loop.run(env, &mut |row, env| {
            loop_step_flow(run_row_body(names, row, body, env)?)
        });
    }
    eval_local_for(names, iterable, dir, body, span, env)
}

/// A `for` over a local collection: a single binding streams the collection's keys
/// (a sequence's 1-based positions), a two-name binding pairs each key with its
/// value. `dir` walks those keys ascending or descending.
fn eval_local_for(
    names: &[String],
    iterable: &ExecExpr,
    dir: Direction,
    body: &ExecBody,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if names.len() >= 2 {
        let rows = materialize_local_collection_dir(eval_expr(iterable, env)?, dir, span)?;
        for (key, value) in rows {
            match run_row_body(names, SavedLoopRow::Full(vec![key], value), body, env)? {
                LoopStep::Iterate => {}
                LoopStep::Stop => break,
                LoopStep::Propagate(flow) => return Ok(flow),
            }
        }
        return Ok(Flow::Normal);
    }
    let keys = match dir {
        Direction::Ascending => enumerate_local_keys_call_arg(iterable, span, env)?,
        Direction::Descending => enumerate_reversed_local_keys_call_arg(iterable, span, env)?,
    };
    let keys = keys.ok_or_else(|| durable_collection_value(span))?;
    for key in keys {
        match run_row_body(names, SavedLoopRow::Key(key), body, env)? {
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
    let first = &binding.names[0];
    let mut range = range_iter(iterable, step, span, env)?;
    while let Some(value) = range.next_value(span)? {
        match run_single_name_body(first, value, body, env)? {
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

/// Bind a streamed row to the head names and run the body. A key-only row binds the
/// single key-first name; a full row binds every key column outermost-first then the
/// leaf value to the final name.
fn run_row_body(
    names: &[String],
    row: SavedLoopRow,
    body: &ExecBody,
    env: &mut Env<'_>,
) -> Result<LoopStep, RuntimeError> {
    env.push_scope();
    match row {
        SavedLoopRow::Key(value) => env.bind(names[0].clone(), value, false),
        SavedLoopRow::Full(columns, leaf) => {
            for (name, value) in names.iter().zip(columns) {
                env.bind(name.clone(), value, false);
            }
            env.bind(names[names.len() - 1].clone(), leaf, false);
        }
    }
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
