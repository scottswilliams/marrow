//! Builtin and `std::` dispatch, conversions, and host capabilities.

use crate::*;

/// Evaluate a `print`/`write` output builtin: render the single argument to text
/// and append it to the output stream (`print` adds a trailing newline). Neither
/// produces a value.
pub(crate) fn eval_output(
    name: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!("`{name}` takes one argument"),
            span,
        });
    };
    let text = render(eval_expr(&arg.value, env)?, span)?;
    let mut output = env.output.borrow_mut();
    output.push_str(&text);
    if name == "print" {
        output.push('\n');
    }
    Ok(None)
}

/// Evaluate `exists(path)`: whether a saved value or child exists at the path.
pub(crate) fn eval_exists(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`exists` takes one argument".into(),
            span,
        });
    };
    Ok(Value::Bool(saved_path_present(&arg.value, span, env)?))
}

/// Evaluate `count(path)`: the number of immediate children when
/// the path has any, otherwise `1` for a present scalar value and `0` when the
/// path is absent. A path with both a value and children counts only its
/// children (its own value is `exists(path)` territory).
pub(crate) fn eval_count(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`count` takes one argument".into(),
            span,
        });
    };
    // A declared index branch `^root.index(args…)` addresses an `Index`/`IndexKey`
    // layer that has no record/layer segment form. Count its entries through the same
    // enumeration `keys(...)` uses over that branch, so `count` and `keys(...).len()`
    // agree. Scalar, record, and keyed-leaf/group layer paths fall through to the
    // direct read/child-keys path below.
    if is_index_branch(&arg.value, env) {
        let entries = enumerate_layer(&arg.value, env)?.len();
        return Ok(Value::Int(entries as i64));
    }
    let path = encode_path(&node_segments(&arg.value, env)?);
    let store = env.store.borrow();
    let children = store
        .child_keys(&path)
        .map_err(|error| error.located(span))?
        .len();
    let count = if children > 0 {
        children
    } else {
        store
            .read(&path)
            .map_err(|error| error.located(span))?
            .is_some() as usize
    };
    Ok(Value::Int(count as i64))
}

/// Whether `expr` is a declared index branch `^root.index(args…)` — a `Call`
/// whose callee names an index off a saved root. An index branch has no
/// record/layer segment form, so `count`, `exists`, and `std::assert::absent`
/// route it through [`enumerate_layer`] instead, matching the same classification
/// [`eval_call`] uses for index lookups.
pub(crate) fn is_index_branch(expr: &Expression, env: &Env<'_>) -> bool {
    let Expression::Call { callee, .. } = expr else {
        return false;
    };
    let Expression::Field { base, name, .. } = callee.as_ref() else {
        return false;
    };
    let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return false;
    };
    find_resource(env.program, root)
        .is_some_and(|resource| resource.indexes.iter().any(|index| &index.name == name))
}

/// Evaluate a `std::assert::*` testing builtin (`isTrue`, `isFalse`, `absent`,
/// `fail`). A failed assertion raises a `run.assertion` error carrying the call
/// span, which `marrow test` reports as a located failure. `absent` reports a
/// populated path as a failed assertion rather than silently treating it as
/// absent. None of these produce a value.
pub(crate) fn eval_assert(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match op {
        "isTrue" | "isFalse" => {
            let [arg] = args else {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_TYPE,
                    message: format!("`std::assert::{op}` takes one boolean"),
                    span,
                });
            };
            let Value::Bool(actual) = eval_expr(&arg.value, env)? else {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_TYPE,
                    message: format!("`std::assert::{op}` takes a boolean"),
                    span,
                });
            };
            if actual != (op == "isTrue") {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_ASSERT,
                    message: format!("assertion failed: {op}({actual})"),
                    span,
                });
            }
            Ok(None)
        }
        "absent" => {
            let [arg] = args else {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_TYPE,
                    message: "`std::assert::absent` takes one path".into(),
                    span,
                });
            };
            if saved_path_present(&arg.value, span, env)? {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_ASSERT,
                    message: "assertion failed: expected the path to be absent".into(),
                    span,
                });
            }
            Ok(None)
        }
        "fail" => {
            let [arg] = args else {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_TYPE,
                    message: "`std::assert::fail` takes one message".into(),
                    span,
                });
            };
            let Value::Str(message) = eval_expr(&arg.value, env)? else {
                return Err(RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_TYPE,
                    message: "`std::assert::fail` takes a string message".into(),
                    span,
                });
            };
            Err(RuntimeError {
                throw: None,
                origin: None,
                code: RUN_ASSERT,
                message,
                span,
            })
        }
        other => Err(unsupported(&format!("std::assert::{other}"), span)),
    }
}

/// Evaluate a pure `std::text::*` or `std::math::*` helper. These take positional
/// arguments and return a value; they need no host capability.
pub(crate) fn eval_std(
    module: &str,
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match (module, op) {
        ("text", "length") => {
            let [text] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Int(
                eval_text(text, env, span)?.chars().count() as i64
            ))
        }
        ("text", "trim") => {
            let [text] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Str(eval_text(text, env, span)?.trim().to_string()))
        }
        ("text", "contains") => {
            let [text, needle] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(text, env, span)?;
            let needle = eval_text(needle, env, span)?;
            Ok(Value::Bool(text.contains(&needle)))
        }
        ("text", "split") => {
            let [text, separator] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(text, env, span)?;
            let separator = eval_text(separator, env, span)?;
            let parts = text
                .split(separator.as_str())
                .map(|part| Value::Str(part.to_string()))
                .collect();
            Ok(Value::Sequence(parts))
        }
        ("math", "absInt") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Int(
                eval_int(&value.value, env)?
                    .checked_abs()
                    .ok_or_else(|| overflow(span))?,
            ))
        }
        ("math", "remainder") => {
            let [a, b] = args else {
                return Err(std_arity(module, op, span));
            };
            let remainder =
                int_remainder(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
            Ok(Value::Int(remainder))
        }
        ("math", "modulo") => {
            let [a, b] = args else {
                return Err(std_arity(module, op, span));
            };
            let modulo = int_modulo(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
            Ok(Value::Int(modulo))
        }
        ("math", "absDecimal") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Decimal(eval_decimal_arg(value, env, span)?.abs()))
        }
        ("math", "floor") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let floored = eval_decimal_arg(value, env, span)?.floor();
            i64::try_from(floored)
                .map(Value::Int)
                .map_err(|_| overflow(span))
        }
        ("bytes", "length") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Int(eval_bytes_arg(value, env, span)?.len() as i64))
        }
        ("bytes", "base64Encode") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Str(base64::encode(&eval_bytes_arg(
                value, env, span,
            )?)))
        }
        ("bytes", "base64Decode") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            base64::decode(&text)
                .map(Value::Bytes)
                .ok_or_else(|| type_error("base64Decode: invalid base64 text", span))
        }
        // An instant has no direct text form; format and parse go through its
        // canonical UTC representation (reusing the store's value codec).
        ("clock", "formatInstant") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let nanos = eval_instant_arg(value, env, span)?;
            let bytes =
                encode_value(&SavedValue::Instant(nanos)).map_err(|error| error.located(span))?;
            let text = String::from_utf8(bytes).expect("a canonical instant encodes as UTF-8 text");
            Ok(Value::Str(text))
        }
        ("clock", "parseInstant") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            match decode_value(text.as_bytes(), ScalarType::Instant) {
                Some(SavedValue::Instant(nanos)) => Ok(Value::Instant(nanos)),
                _ => Err(type_error("parseInstant: invalid instant text", span)),
            }
        }
        // Dates and durations share the instant codec route: format and parse go
        // through their canonical text (`YYYY-MM-DD`, `PT<seconds>S`).
        ("clock", "formatDate") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let days = eval_date_arg(value, env, span)?;
            let bytes =
                encode_value(&SavedValue::Date(days)).map_err(|error| error.located(span))?;
            let text = String::from_utf8(bytes).expect("a canonical date encodes as UTF-8 text");
            Ok(Value::Str(text))
        }
        ("clock", "parseDate") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            match decode_value(text.as_bytes(), ScalarType::Date) {
                Some(SavedValue::Date(days)) => Ok(Value::Date(days)),
                _ => Err(type_error("parseDate: invalid date text", span)),
            }
        }
        ("clock", "formatDuration") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let nanos = eval_duration_arg(value, env, span)?;
            let bytes =
                encode_value(&SavedValue::Duration(nanos)).map_err(|error| error.located(span))?;
            let text =
                String::from_utf8(bytes).expect("a canonical duration encodes as UTF-8 text");
            Ok(Value::Str(text))
        }
        ("clock", "parseDuration") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            match decode_value(text.as_bytes(), ScalarType::Duration) {
                Some(SavedValue::Duration(nanos)) => Ok(Value::Duration(nanos)),
                _ => Err(type_error("parseDuration: invalid duration text", span)),
            }
        }
        // `add(instant, duration)`: shift an instant by a signed span of nanos.
        ("clock", "add") => {
            let [instant, span_arg] = args else {
                return Err(std_arity(module, op, span));
            };
            let nanos = eval_instant_arg(instant, env, span)?;
            let offset = eval_duration_arg(span_arg, env, span)?;
            nanos
                .checked_add(offset)
                .map(Value::Instant)
                .ok_or_else(|| overflow(span))
        }
        _ => Err(unsupported(&format!("std::{module}::{op}"), span)),
    }
}

/// Convert a string argument to bytes (`bytes(text)`): the string's UTF-8 bytes.
pub(crate) fn eval_bytes_conversion(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`bytes` takes one argument", span));
    };
    match eval_expr(&arg.value, env)? {
        Value::Str(text) => Ok(Value::Bytes(text.into_bytes())),
        _ => Err(type_error("`bytes` converts a string to bytes", span)),
    }
}

/// Evaluate a scalar conversion builtin (`int`/`decimal`/`string`/`bool`/`date`/
/// `instant`/`duration`): coerce a dynamically-typed value to the named type.
/// `bool(...)` accepts the canonical boolean values
/// `{false, true, 0, 1}` from a bool, int, or string; `int(...)`/`decimal(...)`
/// parse canonical numeric text from a string (and raise a typed numeric error on
/// malformed input). The remaining conversions validate that the value already
/// has the named type (the `unknown` → concrete bridge); temporal text parsing
/// lives in `std::clock`, not here.
pub(crate) fn eval_conversion(
    name: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(&format!("`{name}` takes one argument"), span));
    };
    let value = eval_expr(&arg.value, env)?;
    match name {
        "bool" => convert_to_bool(value, span),
        "int" => convert_to_int(value, span),
        "decimal" => convert_to_decimal(value, span),
        "string" if matches!(value, Value::Str(_)) => Ok(value),
        "date" if matches!(value, Value::Date(_)) => Ok(value),
        "instant" if matches!(value, Value::Instant(_)) => Ok(value),
        "duration" if matches!(value, Value::Duration(_)) => Ok(value),
        _ => Err(conversion_error(name, span)),
    }
}

/// Coerce to a bool: a bool is itself; an int or string is accepted only as a
/// canonical boolean value (`0`/`false` → `false`, `1`/`true` → `true`).
pub(crate) fn convert_to_bool(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let result = match &value {
        Value::Bool(_) => return Ok(value),
        Value::Int(0) => false,
        Value::Int(1) => true,
        Value::Str(text) if text == "false" || text == "0" => false,
        Value::Str(text) if text == "true" || text == "1" => true,
        _ => return Err(conversion_error("bool", span)),
    };
    Ok(Value::Bool(result))
}

/// Coerce to an int: an int is itself; a string parses as a canonical `i64`
/// (a malformed or out-of-range value is a typed numeric error).
pub(crate) fn convert_to_int(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Int(_) => Ok(value),
        Value::Str(text) => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| conversion_error("int", span)),
        _ => Err(conversion_error("int", span)),
    }
}

/// Coerce to a decimal: a decimal is itself; a string parses as canonical decimal
/// text (a malformed or out-of-envelope value is a typed numeric error).
pub(crate) fn convert_to_decimal(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Decimal(_) => Ok(value),
        Value::Str(text) => Decimal::parse(&text)
            .map(Value::Decimal)
            .ok_or_else(|| conversion_error("decimal", span)),
        _ => Err(conversion_error("decimal", span)),
    }
}

/// Evaluate `arg` and pull out one expected value shape, or a type error naming
/// the expectation. `extract` returns the typed payload when the value matches.
pub(crate) fn eval_typed_arg<T>(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
    expected: &str,
    extract: impl FnOnce(Value) -> Option<T>,
) -> Result<T, RuntimeError> {
    extract(eval_expr(&arg.value, env)?)
        .ok_or_else(|| type_error(&format!("expected {expected}"), span))
}

/// Evaluate `arg` to bytes, or a type error.
pub(crate) fn eval_bytes_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Vec<u8>, RuntimeError> {
    eval_typed_arg(arg, env, span, "bytes", |value| match value {
        Value::Bytes(bytes) => Some(bytes),
        _ => None,
    })
}

/// Evaluate `arg` to a decimal, or a type error.
pub(crate) fn eval_decimal_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Decimal, RuntimeError> {
    eval_typed_arg(arg, env, span, "a decimal", |value| match value {
        Value::Decimal(decimal) => Some(decimal),
        _ => None,
    })
}

/// Evaluate `arg` to an instant (UTC nanoseconds), or a type error.
pub(crate) fn eval_instant_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i128, RuntimeError> {
    eval_typed_arg(arg, env, span, "an instant", |value| match value {
        Value::Instant(nanos) => Some(nanos),
        _ => None,
    })
}

/// Evaluate `arg` to a date (days since the Unix epoch), or a type error.
pub(crate) fn eval_date_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i32, RuntimeError> {
    eval_typed_arg(arg, env, span, "a date", |value| match value {
        Value::Date(days) => Some(days),
        _ => None,
    })
}

/// Evaluate `arg` to a duration (signed nanoseconds), or a type error.
pub(crate) fn eval_duration_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i128, RuntimeError> {
    eval_typed_arg(arg, env, span, "a duration", |value| match value {
        Value::Duration(nanos) => Some(nanos),
        _ => None,
    })
}

/// Evaluate `arg` to a string, or a type error.
pub(crate) fn eval_text(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<String, RuntimeError> {
    eval_typed_arg(arg, env, span, "a string", |value| match value {
        Value::Str(text) => Some(text),
        _ => None,
    })
}

/// Truncated integer remainder (sign of the dividend), rejecting a zero divisor
/// and the `i64::MIN % -1` overflow.
pub(crate) fn int_remainder(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_DIVIDE_BY_ZERO,
            message: "integer remainder by zero".into(),
            span,
        });
    }
    a.checked_rem(b).ok_or_else(|| overflow(span))
}

/// Floored integer modulo (sign of the divisor).
pub(crate) fn int_modulo(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    let remainder = int_remainder(a, b, span)?;
    // Shift the truncated remainder toward the divisor's sign when they differ.
    Ok(if remainder != 0 && (remainder < 0) != (b < 0) {
        remainder + b
    } else {
        remainder
    })
}

/// Build a builtin `Error(...)` value from named arguments as a resource-shaped
/// `Value`. `code` and `message` are required; `help` and `data` are optional.
/// Positional, duplicate, or unknown fields are type errors.
pub(crate) fn eval_error_constructor(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut fields: Vec<(String, Value)> = Vec::new();
    for arg in args {
        let Some(name) = &arg.name else {
            return Err(type_error("`Error(...)` takes named arguments", span));
        };
        if !matches!(name.as_str(), "code" | "message" | "help" | "data") {
            return Err(type_error(&format!("`Error` has no field `{name}`"), span));
        }
        if fields.iter().any(|(existing, _)| existing == name) {
            return Err(type_error(
                &format!("`{name}` is supplied more than once"),
                span,
            ));
        }
        fields.push((name.clone(), eval_expr(&arg.value, env)?));
    }
    for required in ["code", "message"] {
        if !fields.iter().any(|(name, _)| name == required) {
            return Err(type_error(&format!("`Error` requires `{required}`"), span));
        }
    }
    Ok(Value::Resource(fields))
}

/// Number of nanoseconds in a UTC day, for `today()`'s instant-to-date reduction.
pub(crate) const NANOS_PER_DAY: i128 = 86_400_000_000_000;

/// Evaluate `std::clock::now()` (an instant) or `std::clock::today()` (the UTC
/// calendar date) from the host's clock capability. A run with no clock
/// capability raises a typed capability error rather than reading the wall clock
/// implicitly.
pub(crate) fn eval_clock_capability(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if !args.is_empty() {
        return Err(type_error(
            &format!("`std::clock::{op}` takes no arguments"),
            span,
        ));
    }
    let nanos = env.host.clock.ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_CAPABILITY,
        message: format!("this run provides no clock capability for `std::clock::{op}`"),
        span,
    })?;
    match op {
        "now" => Ok(Value::Instant(nanos)),
        // The UTC calendar date is the floored day count, matching the store's
        // instant-to-date reduction.
        "today" => Ok(Value::Date(nanos.div_euclid(NANOS_PER_DAY) as i32)),
        _ => Err(unsupported(&format!("std::clock::{op}"), span)),
    }
}

/// Evaluate a `std::env::*` builtin against the host's environment capability:
/// `exists(name)`, `get(name, default)`, or `require(name)`. A run with no
/// environment capability raises a typed capability error rather than reading
/// the process environment implicitly; `require` on an absent variable raises a
/// typed absence error.
pub(crate) fn eval_env(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    // Evaluate the string arguments before borrowing the environment, so their
    // mutable use of `env` does not overlap the shared read of the capability.
    let names: Vec<String> = args
        .iter()
        .map(|arg| eval_text(arg, env, span))
        .collect::<Result<_, _>>()?;
    let variables = env.host.environment.as_ref().ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_CAPABILITY,
        message: format!("this run provides no environment capability for `std::env::{op}`"),
        span,
    })?;
    match (op, names.as_slice()) {
        ("exists", [name]) => Ok(Value::Bool(variables.contains_key(name))),
        ("get", [name, default]) => Ok(Value::Str(
            variables
                .get(name)
                .cloned()
                .unwrap_or_else(|| default.clone()),
        )),
        ("require", [name]) => match variables.get(name).cloned() {
            Some(value) => Ok(Value::Str(value)),
            None => Err(raise_fault(
                RUN_ABSENT,
                format!("required environment variable `{name}` is absent"),
                span,
            )),
        },
        ("exists" | "get" | "require", _) => Err(std_arity("env", op, span)),
        _ => Err(unsupported(&format!("std::env::{op}"), span)),
    }
}

/// Evaluate a `std::log::*` builtin against the host's log capability:
/// `info(message)`, `warn(message)`, or `error(err)`. Each appends one formatted
/// line to the sink and yields nothing. A run with no log capability raises a
/// typed capability error.
pub(crate) fn eval_log(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // Evaluate the arguments before borrowing the sink, so their mutable use of
    // `env` does not overlap the shared read of the capability.
    let values: Vec<Value> = args
        .iter()
        .map(|arg| eval_expr(&arg.value, env))
        .collect::<Result<_, _>>()?;
    let sink = env.host.log.as_ref().ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_CAPABILITY,
        message: format!("this run provides no log capability for `std::log::{op}`"),
        span,
    })?;
    let line = match (op, values.as_slice()) {
        ("info", [Value::Str(message)]) => format!("INFO {message}\n"),
        ("warn", [Value::Str(message)]) => format!("WARN {message}\n"),
        ("info" | "warn", [_]) => return Err(type_error("expected a string message", span)),
        ("error", [value]) => {
            let code = error_field(value, "code")
                .ok_or_else(|| type_error("`std::log::error` expects an Error", span))?;
            let message = error_field(value, "message").unwrap_or_default();
            format!("ERROR [{code}] {message}\n")
        }
        ("info" | "warn" | "error", _) => return Err(std_arity("log", op, span)),
        _ => return Err(unsupported(&format!("std::log::{op}"), span)),
    };
    sink.borrow_mut().push_str(&line);
    Ok(None)
}

/// Evaluate a `std::io::*` file builtin against the host's filesystem capability:
/// `readText`/`writeText`/`readBytes`/`writeBytes`. A run with no filesystem
/// capability raises a typed capability fault; an IO failure (a missing file,
/// permissions) raises a catchable `Error` value the program can `catch`.
pub(crate) fn eval_io(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // Evaluate the arguments before checking the capability, as the other host
    // modules do.
    let values: Vec<Value> = args
        .iter()
        .map(|arg| eval_expr(&arg.value, env))
        .collect::<Result<_, _>>()?;
    if !env.host.filesystem {
        return Err(RuntimeError::fault(
            RUN_CAPABILITY,
            format!("this run provides no filesystem capability for `std::io::{op}`"),
            span,
        ));
    }
    // A failed `std::io` call raises in this frame, so it carries no deeper
    // origin; the `invoke` boundary stamps this frame's file as it leaves.
    match (op, values.as_slice()) {
        ("readText", [Value::Str(path)]) => match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(Value::Str(text))),
            Err(error) => Err(raise(io_error("io.read", op, path, &error), span, None)),
        },
        ("writeText", [Value::Str(path), Value::Str(text)]) => match std::fs::write(path, text) {
            Ok(()) => Ok(None),
            Err(error) => Err(raise(io_error("io.write", op, path, &error), span, None)),
        },
        ("readBytes", [Value::Str(path)]) => match std::fs::read(path) {
            Ok(bytes) => Ok(Some(Value::Bytes(bytes))),
            Err(error) => Err(raise(io_error("io.read", op, path, &error), span, None)),
        },
        ("writeBytes", [Value::Str(path), Value::Bytes(data)]) => {
            match std::fs::write(path, data) {
                Ok(()) => Ok(None),
                Err(error) => Err(raise(io_error("io.write", op, path, &error), span, None)),
            }
        }
        ("readText" | "writeText" | "readBytes" | "writeBytes", _) => Err(type_error(
            &format!("`std::io::{op}` got the wrong arguments"),
            span,
        )),
        _ => Err(unsupported(&format!("std::io::{op}"), span)),
    }
}
