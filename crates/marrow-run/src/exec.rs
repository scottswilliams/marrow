//! The statement spine: blocks, statements, matches, and loops.

use crate::*;

/// Evaluate a block in its own scope, stopping at the first `return`. The scope
/// is popped on every exit, including when a statement raises an error, so the
/// environment is left balanced for reuse.
pub(crate) fn eval_block(block: &Block, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    env.push_scope();
    let result = eval_statements(&block.statements, env);
    env.pop_scope();
    result
}

/// Evaluate statements in order until one returns or the block ends.
pub(crate) fn eval_statements(
    statements: &[Statement],
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    for statement in statements {
        let flow = eval_statement(statement, env)?;
        if !matches!(flow, Flow::Normal) {
            return Ok(flow);
        }
    }
    Ok(Flow::Normal)
}

pub(crate) fn eval_statement(
    statement: &Statement,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    // Opt-in debugger step: offer this statement to an installed hook before it
    // runs. An ordinary run has no hook, so this is a single `is_some` check.
    // Taking the hook out of `env` frees the `&mut Env` to be reborrowed as the
    // `&Env` the read-only [`Frame`] needs, with no aliasing and no `unsafe`; it
    // is put back whether the call succeeds or aborts the run.
    if env.hook.is_some() {
        let mut hook = env.hook.take();
        let result = hook
            .as_deref_mut()
            .expect("hook is Some")
            .before_statement(statement.span(), Frame { env });
        env.hook = hook;
        result?;
    }
    match statement {
        Statement::Const { name, value, .. } => {
            let value = eval_expr(value, env)?;
            env.bind(name.clone(), value, false);
            Ok(Flow::Normal)
        }
        Statement::Var {
            name,
            keys,
            ty,
            value,
            span,
        } => {
            if !keys.is_empty() {
                return Err(unsupported("a keyed local variable", *span));
            }
            let value = match value {
                Some(expr) => eval_expr(expr, env)?,
                // An uninitialized var starts at its type's default — an empty
                // resource, an empty sequence, or a scalar zero — so a declared but
                // unwritten place (e.g. an `out` argument target, the documented
                // `var n: int` then `f(out n)` pattern) is usable before its first
                // assignment.
                None => match ty.as_ref().map(Type::resolve) {
                    Some(Type::Named(name)) if is_resource_type(env.program, env.module, &name) => {
                        Value::Resource(Vec::new())
                    }
                    Some(ty) => default_value(&ty).ok_or_else(|| {
                        unsupported("an uninitialized variable of this type", *span)
                    })?,
                    None => return Err(unsupported("an uninitialized variable", *span)),
                },
            };
            env.bind(name.clone(), value, true);
            Ok(Flow::Normal)
        }
        Statement::Assign {
            target,
            value,
            span,
        } => {
            // A dotted field off a saved record is a managed field write; a
            // `^root(key…)` or bare singleton `^root` target is a whole-resource
            // write; a bare name is a local reassignment.
            if let Expression::Field {
                base, name, quoted, ..
            } = target
            {
                if is_saved_path(base) {
                    eval_saved_field_write(base, name, *quoted, value, *span, env)?;
                } else {
                    eval_local_field_set(base, name, value, *span, env)?;
                }
            } else if let Expression::SavedRoot { .. } = target {
                eval_resource_write(target, value, *span, env)?;
            } else if let Expression::Call { callee, args, .. } = target {
                // `^root(key…).layer(key…) = v` (callee is a saved layer field) is a
                // whole-group-entry write; `^root(key…) = v` (callee is the saved
                // root) is a whole-resource write.
                if let Expression::Field { base, name, .. } = callee.as_ref()
                    && is_saved_path(base)
                {
                    eval_group_entry_write(base, name, args, value, *span, env)?;
                } else {
                    eval_resource_write(target, value, *span, env)?;
                }
            } else {
                let name = local_target(target, *span)?;
                let evaluated = eval_expr(value, env)?;
                env.assign(name, evaluated)
                    .map_err(|error| assign_error(name, error, *span))?;
            }
            Ok(Flow::Normal)
        }
        Statement::Delete { path, span } => {
            eval_delete(path, *span, env)?;
            Ok(Flow::Normal)
        }
        Statement::Merge {
            target,
            value,
            span,
        } => {
            // A `.layer` off a saved record is a keyed-layer merge; a bare local
            // name is a merge into a local resource var; a `^root(key…)` target is
            // a whole-resource saved merge.
            if let Expression::Field { base, name, .. } = target
                && is_saved_path(base)
            {
                eval_layer_merge(base, name, value, *span, env)?;
            } else if let Expression::Name { segments, .. } = target
                && let [name] = segments.as_slice()
            {
                eval_local_merge(name, value, *span, env)?;
            } else {
                eval_resource_merge(target, value, *span, env)?;
            }
            Ok(Flow::Normal)
        }
        Statement::Return { value, .. } => {
            let value = value
                .as_ref()
                .map(|expr| eval_expr(expr, env))
                .transpose()?;
            Ok(Flow::Return(value))
        }
        Statement::Expr { value, .. } => {
            // A call statement may invoke a function that returns nothing; only a
            // call in value position requires a return value.
            if let Expression::Call { callee, args, span } = value {
                eval_call(callee, args, *span, env)?;
            } else {
                eval_expr(value, env)?;
            }
            Ok(Flow::Normal)
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            span,
        } => {
            if eval_condition(condition.as_ref(), *span, env)? {
                return eval_block(then_block, env);
            }
            for else_if in else_ifs {
                if eval_condition(else_if.condition.as_ref(), *span, env)? {
                    return eval_block(&else_if.block, env);
                }
            }
            match else_block {
                Some(block) => eval_block(block, env),
                None => Ok(Flow::Normal),
            }
        }
        Statement::Match {
            scrutinee,
            arms,
            enum_name,
            enum_module,
            span,
        } => eval_match(
            scrutinee.as_ref(),
            arms,
            enum_module.as_deref(),
            enum_name.as_deref(),
            *span,
            env,
        ),
        Statement::Break { label, .. } => Ok(Flow::Break(label.clone())),
        Statement::Continue { label, .. } => Ok(Flow::Continue(label.clone())),
        Statement::While {
            label,
            condition,
            body,
            span,
        } => eval_while(label, condition.as_ref(), body, *span, env),
        Statement::For {
            label,
            binding,
            iterable,
            step,
            body,
            span,
        } => eval_for(label, binding, iterable, step.as_ref(), body, *span, env),
        Statement::Transaction { body, span, .. } => {
            // Open a backend transaction; the backend's savepoint stack handles
            // nesting. Any non-error exit (fall-through, `return`, `break`,
            // `continue`) commits; an escaping error rolls back. Local variables
            // and output already produced are not rewound.
            env.store
                .borrow_mut()
                .begin()
                .map_err(|error| error.located(*span))?;
            // A managed write inside this block now rides the open savepoint, so it
            // applies its steps in place rather than opening its own (see
            // `WritePlan::commit`'s `in_txn`). The depth tracks nesting so an inner
            // block still counts as "in a transaction".
            env.transaction_depth += 1;
            let result = eval_block(body, env);
            env.transaction_depth -= 1;
            match result {
                // A throw escapes the transaction, so it rolls back like an error
                // rather than committing. If the rollback itself fails, the store
                // is left in an indeterminate state — an integrity failure that
                // must not be masked by a catchable throw, so surface it as a
                // typed store error instead.
                Ok(Flow::Throw(value)) => match env.store.borrow_mut().rollback() {
                    Ok(()) => Ok(Flow::Throw(value)),
                    Err(rb_err) => Err(rb_err.located(*span)),
                },
                Ok(flow) => {
                    env.store
                        .borrow_mut()
                        .commit()
                        .map_err(|error| error.located(*span))?;
                    Ok(flow)
                }
                // The body errored, so the transaction rolls back. A failed
                // rollback is a store-integrity error that supersedes the original
                // cause (the staged writes may have partially survived), so report
                // it; otherwise surface the original error as before.
                Err(error) => match env.store.borrow_mut().rollback() {
                    Ok(()) => Err(error),
                    Err(rb_err) => Err(rb_err.located(*span)),
                },
            }
        }
        Statement::Throw { value, span } => {
            let thrown = eval_expr(value, env)?;
            // `throw` requires an `Error` value (resource-shaped). The checker does
            // not type-check throw operands, so guard here.
            if !matches!(thrown, Value::Resource(_)) {
                return Err(type_error("`throw` requires an `Error` value", *span));
            }
            Ok(Flow::Throw(thrown))
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            let outcome = eval_block(body, env);
            // The thrown `Error` to handle, drawn from one channel: a `throw`
            // statement (`Ok(Flow::Throw)`) or a throw/recoverable fault from a
            // called function or builtin (an `Err` carrying its value in `throw`).
            // `catch` handles only those; a fatal fault (an `Err` with no `throw`)
            // and other control flow pass through unchanged.
            let thrown = match &outcome {
                Ok(Flow::Throw(value)) => Some(value.clone()),
                Err(error) => error.throw.as_deref().cloned(),
                _ => None,
            };
            let handled = match (thrown, catch) {
                (Some(error), Some(clause)) => {
                    env.push_scope();
                    env.bind(clause.name.clone(), error, false);
                    let caught = eval_block(&clause.block, env);
                    env.pop_scope();
                    caught
                }
                // No `catch`, or a fatal fault: keep unwinding unchanged. The thrown
                // value already rides the `Err`'s `throw`, so an outer handler still
                // finds it — nothing to re-stash.
                (Some(_), None) | (None, _) => outcome,
            };
            // `finally` always runs. A throwing or faulting finally replaces the
            // pending outcome; a normal one is cleanup and the outcome proceeds.
            // (The checker forbids return/break/continue in `finally`.)
            match finally {
                Some(block) => match eval_block(block, env) {
                    Ok(Flow::Throw(error)) => Ok(Flow::Throw(error)),
                    Err(error) => Err(error),
                    Ok(_) => handled,
                },
                None => handled,
            }
        }
        Statement::Lock { body, .. } => {
            // A single-writer capability profile holds no contended lock, so there
            // is no lock state to acquire or release: `lock` is just its body. The
            // body runs in `eval_block`, which pops its scope on every exit
            // (including errors). The target path only matters for coordinating
            // concurrent writers, so it is not read here.
            eval_block(body, env)
        }
    }
}

/// Dispatch a `match` over an enum-typed scrutinee. The scrutinee evaluates to
/// the selected member's ordinal (an int); the arm whose member has that ordinal
/// runs. The checker records the scrutinee's enum identity on the statement
/// (`enum_module`/`enum_name`), so dispatch reads that exact enum's ordinals —
/// two enums that share member names, even across modules with the same enum
/// name, never alias. The checker also proves a covering arm exists, so a missing
/// match is a defensive fault, not a reachable path.
pub(crate) fn eval_match(
    scrutinee: Option<&Expression>,
    arms: &[MatchArm],
    enum_module: Option<&str>,
    enum_name: Option<&str>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    let scrutinee = scrutinee.ok_or_else(|| unsupported("a match with no scrutinee", span))?;
    let Value::Int(ordinal) = eval_expr(scrutinee, env)? else {
        return Err(type_error("`match` requires an enum value", span));
    };
    let schema = enum_module
        .zip(enum_name)
        .and_then(|(module, name)| enum_in(env.program, module, name))
        .ok_or_else(|| unsupported("a match over a non-enum value", span))?;
    for arm in arms {
        // An arm is a member path relative to the scrutinee enum; the checker proved
        // it walks to a single member and that arms do not overlap. A concrete-leaf
        // arm covers its own ordinal; a category arm covers every descendant, so both
        // reduce to the inclusive `is_descendant` from the arm's ordinal. The walk is
        // the same one the checker used, so dispatch and coverage cannot drift.
        let segments: Vec<&str> = arm.path.iter().map(String::as_str).collect();
        if let MemberPathResolution::Found(arm_ordinal) = schema.walk_member_path(&segments)
            && schema.is_descendant(ordinal as usize, arm_ordinal)
        {
            return eval_block(&arm.block, env);
        }
    }
    // The checker proved exhaustiveness, so no covering arm means the stored
    // ordinal is outside the enum — corrupt data, not a reachable program path.
    Err(unsupported("a match with no arm for this enum value", span))
}

/// How a loop body's resulting flow affects a loop labelled `label`.
pub(crate) enum LoopStep {
    /// Run the next iteration (the body fell through, or `continue`d this loop).
    Iterate,
    /// Stop the loop (a `break` targeting this loop).
    Stop,
    /// Leave the loop carrying an outward jump: a `return`, or a `break` /
    /// `continue` aimed at an enclosing loop.
    Propagate(Flow),
}

/// Classify a loop body's flow for a loop labelled `label`.
pub(crate) fn classify(flow: Flow, label: &Option<String>) -> LoopStep {
    match flow {
        Flow::Normal => LoopStep::Iterate,
        Flow::Continue(ref target) if targets_this_loop(target, label) => LoopStep::Iterate,
        Flow::Break(ref target) if targets_this_loop(target, label) => LoopStep::Stop,
        other => LoopStep::Propagate(other),
    }
}

/// Whether a `break`/`continue` carrying `jump_label` targets a loop labelled
/// `loop_label`: an unlabelled jump targets the innermost (this) loop; a
/// labelled jump targets only the loop with the matching label.
pub(crate) fn targets_this_loop(jump_label: &Option<String>, loop_label: &Option<String>) -> bool {
    match jump_label {
        None => true,
        Some(name) => loop_label.as_deref() == Some(name.as_str()),
    }
}

pub(crate) fn eval_while(
    label: &Option<String>,
    condition: Option<&Expression>,
    body: &Block,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    while eval_condition(condition, span, env)? {
        match classify(eval_block(body, env)?, label) {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

/// Run `loop_body` with `prefix` marked as an actively-traversed saved layer (if
/// any), popping it afterward whatever the body returns, so a self-mutating write
/// inside the loop is caught by [`Env::guard_traversed_layer`] and the guard never
/// outlives the loop.
pub(crate) fn iterate_saved_layer(
    prefix: Option<Vec<PathSegment>>,
    env: &mut Env<'_>,
    loop_body: impl FnOnce(&mut Env<'_>) -> Result<Flow, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let pushed = prefix.is_some();
    if let Some(prefix) = prefix {
        env.traversed_layers.push(encode_path(&prefix));
    }
    let result = loop_body(env);
    if pushed {
        env.traversed_layers.pop();
    }
    result
}

pub(crate) fn eval_for(
    label: &Option<String>,
    binding: &ForBinding,
    iterable: &Expression,
    step: Option<&Expression>,
    body: &Block,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    // A two-name binding over a saved collection binds its address and element
    // directly. Other iterables still need an explicit pair sequence, such as
    // `entries(...)`; ranges have no second name to bind.
    if let Some(second) = &binding.second {
        if matches!(
            iterable,
            Expression::Binary {
                op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
                ..
            }
        ) {
            return Err(unsupported("a two-name binding over a range", span));
        }
        let entries = eval_collection_entries(iterable, env)?;
        let prefix = traversed_layer_prefix(iterable, env)?;
        return iterate_saved_layer(prefix, env, |env| {
            for entry in entries {
                let Value::Sequence(pair) = entry else {
                    return Err(unsupported(
                        "a two-name binding over a non-pair iterable (use entries(...))",
                        span,
                    ));
                };
                let [key, value] = <[Value; 2]>::try_from(pair).map_err(|_| {
                    unsupported(
                        "a two-name binding over a non-pair iterable (use entries(...))",
                        span,
                    )
                })?;
                env.push_scope();
                env.bind(binding.first.clone(), key, false);
                env.bind(second.clone(), value, false);
                let flow = eval_block(body, env);
                env.pop_scope();
                match classify(flow?, label) {
                    LoopStep::Iterate => {}
                    LoopStep::Stop => break,
                    LoopStep::Propagate(flow) => return Ok(flow),
                }
            }
            Ok(Flow::Normal)
        });
    }
    // A non-range iterable (e.g. `keys(^books.byShelf("x"))`) materializes to a
    // sequence of values, which the loop binds one at a time.
    if !matches!(
        iterable,
        Expression::Binary {
            op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
            ..
        }
    ) {
        let values = eval_collection(iterable, env)?;
        let prefix = traversed_layer_prefix(iterable, env)?;
        return iterate_saved_layer(prefix, env, |env| {
            for value in values {
                env.push_scope();
                env.bind(binding.first.clone(), value, false);
                let flow = eval_block(body, env);
                env.pop_scope();
                match classify(flow?, label) {
                    LoopStep::Iterate => {}
                    LoopStep::Stop => break,
                    LoopStep::Propagate(flow) => return Ok(flow),
                }
            }
            Ok(Flow::Normal)
        });
    }
    let mut range = range_iter(iterable, step, span, env)?;
    while let Some(value) = range.next_value(span)? {
        env.push_scope();
        env.bind(binding.first.clone(), value, false);
        let flow = eval_block(body, env);
        env.pop_scope();
        match classify(flow?, label) {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

/// Nanoseconds in one calendar day, the span a `date` advances by per whole day.
const NANOS_PER_DAY: i128 = 86_400 * 1_000_000_000;

/// A typed, stepped range iterator over a `for` header's endpoints. Each variant
/// carries the live cursor, the resolved endpoint, the inclusive flag, and the
/// step. Direction is the step's sign: a wrong-direction or out-of-range step
/// yields no values rather than looping forever. The endpoints share one steppable
/// type (int, decimal, date, instant); a non-steppable endpoint never reaches here.
enum RangeIter {
    /// int and date both advance an integer cursor: int by its step, date by a
    /// whole number of calendar days (the date is days-since-epoch, so adding whole
    /// days is leap-safe). `make` keeps each wrapped in the right [`Value`].
    Integer {
        current: i64,
        end: i64,
        inclusive: bool,
        step: i64,
        make: fn(i64) -> Value,
    },
    Decimal {
        current: Decimal,
        end: Decimal,
        inclusive: bool,
        step: Decimal,
    },
    Instant {
        current: i128,
        end: i128,
        inclusive: bool,
        step: i128,
    },
}

impl RangeIter {
    /// The next value in the range, or `None` once the cursor passes the end. A
    /// step toward the endpoint that overflows ends the loop rather than faulting.
    fn next_value(&mut self, span: SourceSpan) -> Result<Option<Value>, RuntimeError> {
        match self {
            RangeIter::Integer {
                current,
                end,
                inclusive,
                step,
                make,
            } => {
                if !int_in_range(*current, *end, *inclusive, *step) {
                    return Ok(None);
                }
                let value = make(*current);
                match current.checked_add(*step) {
                    Some(next) => *current = next,
                    // Stepping past `i64::MIN`/`MAX` ends the loop; the value just
                    // read is still yielded. A zero step then stops the next call.
                    None => *step = 0,
                }
                Ok(Some(value))
            }
            RangeIter::Decimal {
                current,
                end,
                inclusive,
                step,
            } => {
                if !decimal_in_range(*current, *end, *inclusive, *step) {
                    return Ok(None);
                }
                let value = Value::Decimal(*current);
                *current = current.checked_add(*step).ok_or_else(|| overflow(span))?;
                Ok(Some(value))
            }
            RangeIter::Instant {
                current,
                end,
                inclusive,
                step,
            } => {
                if !instant_in_range(*current, *end, *inclusive, *step) {
                    return Ok(None);
                }
                let value = Value::Instant(*current);
                *current = current.checked_add(*step).ok_or_else(|| overflow(span))?;
                Ok(Some(value))
            }
        }
    }
}

/// Whether an integer cursor is still within the range, given the step's sign. A
/// zero step never progresses, so it yields nothing; an ascending step stops at the
/// end, a descending one stops below it.
fn int_in_range(current: i64, end: i64, inclusive: bool, step: i64) -> bool {
    match step.cmp(&0) {
        Ordering::Greater if inclusive => current <= end,
        Ordering::Greater => current < end,
        Ordering::Less if inclusive => current >= end,
        Ordering::Less => current > end,
        Ordering::Equal => false,
    }
}

fn decimal_in_range(current: Decimal, end: Decimal, inclusive: bool, step: Decimal) -> bool {
    let sign = step.coefficient().cmp(&0);
    match sign {
        Ordering::Greater if inclusive => current <= end,
        Ordering::Greater => current < end,
        Ordering::Less if inclusive => current >= end,
        Ordering::Less => current > end,
        Ordering::Equal => false,
    }
}

fn instant_in_range(current: i128, end: i128, inclusive: bool, step: i128) -> bool {
    match step.cmp(&0) {
        Ordering::Greater if inclusive => current <= end,
        Ordering::Greater => current < end,
        Ordering::Less if inclusive => current >= end,
        Ordering::Less => current > end,
        Ordering::Equal => false,
    }
}

/// Build the typed range iterator for a `for` header. The endpoints decide the
/// type; the step is the explicit `by` value or the type's default (int => 1,
/// date => one calendar day). Decimal and instant have no safe default, so a
/// missing step there is a fault — the checker reports it first, but a dynamically
/// built program still faults rather than guessing. A zero step is a fault for the
/// same reason. A date step must be a whole number of days.
fn range_iter(
    iterable: &Expression,
    step: Option<&Expression>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<RangeIter, RuntimeError> {
    let (left, right, inclusive) = match iterable {
        Expression::Binary {
            op: BinaryOp::RangeExclusive,
            left,
            right,
            ..
        } => (left, right, false),
        Expression::Binary {
            op: BinaryOp::RangeInclusive,
            left,
            right,
            ..
        } => (left, right, true),
        other => return Err(unsupported("iterating this value", other.span())),
    };
    let start = eval_expr(left, env)?;
    let end = eval_expr(right, env)?;
    let step = step.map(|expr| eval_expr(expr, env)).transpose()?;
    match (start, end) {
        (Value::Int(start), Value::Int(end)) => {
            let step = match step {
                Some(Value::Int(n)) => n,
                Some(_) => return Err(type_error("an int range steps by an int", span)),
                None => 1,
            };
            if step == 0 {
                return Err(type_error("a range step cannot be zero", span));
            }
            Ok(RangeIter::Integer {
                current: start,
                end,
                inclusive,
                step,
                make: Value::Int,
            })
        }
        (Value::Decimal(start), Value::Decimal(end)) => {
            let step = match step {
                Some(Value::Decimal(d)) => d,
                Some(_) => return Err(type_error("a decimal range steps by a decimal", span)),
                None => {
                    return Err(type_error(
                        "a decimal range needs an explicit `by` step",
                        span,
                    ));
                }
            };
            if step.is_zero() {
                return Err(type_error("a range step cannot be zero", span));
            }
            Ok(RangeIter::Decimal {
                current: start,
                end,
                inclusive,
                step,
            })
        }
        (Value::Date(start), Value::Date(end)) => {
            let step_days = match step {
                Some(Value::Duration(nanos)) => duration_whole_days(nanos, span)?,
                Some(_) => return Err(type_error("a date range steps by a duration", span)),
                None => 1,
            };
            if step_days == 0 {
                return Err(type_error("a range step cannot be zero", span));
            }
            Ok(RangeIter::Integer {
                current: i64::from(start),
                end: i64::from(end),
                inclusive,
                step: step_days,
                make: |days| Value::Date(days as i32),
            })
        }
        (Value::Instant(start), Value::Instant(end)) => {
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
        _ => Err(type_error(
            "a range needs two endpoints of the same type",
            span,
        )),
    }
}

/// A duration step for a `date` range as a whole number of calendar days. A date
/// has no time of day, so a sub-day step has no meaning and faults; an exact
/// multiple of a day is its day count (negative for a descending range).
fn duration_whole_days(nanos: i128, span: SourceSpan) -> Result<i64, RuntimeError> {
    if nanos % NANOS_PER_DAY != 0 {
        return Err(type_error(
            "a date range step must be a whole number of days",
            span,
        ));
    }
    i64::try_from(nanos / NANOS_PER_DAY).map_err(|_| overflow(span))
}

/// Materialize a non-range `for` iterable to a sequence of values. A saved-layer
/// path yields elements for value-bearing collections and identities for
/// key-only index branches. `keys(<layer>)` preserves address-only traversal.
/// Every other iterable must evaluate to an in-memory sequence (e.g.
/// `std::text::split(...)`).
pub(crate) fn eval_collection(
    iterable: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    // `for x in reversed(L)` walks the same layer in reverse key order. `keys(L)`
    // stays address-only; direct value-bearing layers materialize their elements.
    // `reversed(values(L))` / `reversed(entries(L))` and in-memory sequences fall
    // through to `eval_expr`, which shapes those rows.
    if let Some(inner) = reversed_argument(iterable) {
        if let Some(layer) = keys_argument(inner) {
            check_key_collection(layer, iterable.span(), env)?;
            return enumerate_layer_dir(layer, Direction::Descending, env);
        }
        if is_saved_path(inner) {
            if is_iterable_index_branch(inner, env) {
                return enumerate_layer_dir(inner, Direction::Descending, env);
            }
            return materialize_layer_dir(inner, Direction::Descending, env)
                .map(|rows| rows.into_iter().map(|(_, value)| value).collect());
        }
    }
    if let Some(path) = keys_argument(iterable) {
        check_key_collection(path, iterable.span(), env)?;
        return enumerate_layer(path, env);
    }
    if is_saved_path(iterable) {
        if is_iterable_index_branch(iterable, env) {
            return enumerate_layer(iterable, env);
        }
        return materialize_layer(iterable, env)
            .map(|rows| rows.into_iter().map(|(_, value)| value).collect());
    }
    match eval_expr(iterable, env)? {
        Value::Sequence(items) => Ok(items),
        _ => Err(unsupported("iterating this value", iterable.span())),
    }
}

fn eval_collection_entries(
    iterable: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    if let Some(inner) = reversed_argument(iterable)
        && is_saved_path(inner)
        && keys_argument(inner).is_none()
    {
        return materialize_entry_pairs(materialize_layer_dir(inner, Direction::Descending, env)?);
    }
    if is_saved_path(iterable) {
        return materialize_entry_pairs(materialize_layer(iterable, env)?);
    }
    eval_collection(iterable, env)
}

fn materialize_entry_pairs(rows: Vec<(Value, Value)>) -> Result<Vec<Value>, RuntimeError> {
    Ok(rows
        .into_iter()
        .map(|(key, value)| Value::Sequence(vec![key, value]))
        .collect())
}

/// The single local name an assignment targets, or an "unsupported" error for a
/// saved path or qualified name (those are dispatched before reaching here).
pub(crate) fn local_target(target: &Expression, span: SourceSpan) -> Result<&str, RuntimeError> {
    match target {
        Expression::Name { segments, .. } if segments.len() == 1 => Ok(&segments[0]),
        _ => Err(unsupported("assignment to this target", span)),
    }
}
