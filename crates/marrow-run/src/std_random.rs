use marrow_check::CheckedArg as ExecArg;
use marrow_store::Decimal;
use marrow_syntax::SourceSpan;
use sha2::{Digest, Sha256};

use crate::env::Env;
use crate::error::{RuntimeError, std_arity, type_error};
use crate::expr::eval_int;
use crate::stdlib::eval_text;
use crate::value::Value;

pub(crate) fn eval_random(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "int" => {
            let [seed, step, min, max] = args else {
                return Err(std_arity("random", op, span));
            };
            let seed = eval_text(seed, env, span)?;
            let step = step_value(step, env, span)?;
            let min = eval_int(&min.value, env)?;
            let max = eval_int(&max.value, env)?;
            if min > max {
                return Err(type_error("random int min must be <= max", span));
            }
            Ok(Value::Int(random_int(&seed, step, min, max)))
        }
        "bool" => {
            let [seed, step] = args else {
                return Err(std_arity("random", op, span));
            };
            let seed = eval_text(seed, env, span)?;
            let step = step_value(step, env, span)?;
            Ok(Value::Bool(
                (prf_u128("random.bool", &seed, step, 0) & 1) == 1,
            ))
        }
        "decimal" => {
            let [seed, step] = args else {
                return Err(std_arity("random", op, span));
            };
            let seed = eval_text(seed, env, span)?;
            let step = step_value(step, env, span)?;
            let units =
                (prf_u128("random.decimal", &seed, step, 0) % 1_000_000_000_000_000_000) as i128;
            Ok(Value::Decimal(Decimal::from_parts(units, 18).expect(
                "18 fractional digits are inside the decimal envelope",
            )))
        }
        _ => Err(crate::error::unsupported(
            &format!("std::random::{op}"),
            span,
        )),
    }
}

fn step_value(arg: &ExecArg, env: &mut Env<'_>, span: SourceSpan) -> Result<i64, RuntimeError> {
    let step = eval_int(&arg.value, env)?;
    if step < 0 {
        return Err(type_error("random step must be non-negative", span));
    }
    Ok(step)
}

fn random_int(seed: &str, step: i64, min: i64, max: i64) -> i64 {
    let width = (i128::from(max) - i128::from(min) + 1) as u128;
    let threshold = u128::MAX - (u128::MAX % width);
    for attempt in 0.. {
        let candidate = prf_u128("random.int", seed, step, attempt);
        if candidate < threshold {
            let offset =
                i128::try_from(candidate % width).expect("int range width always fits inside i128");
            let value = i128::from(min) + offset;
            return i64::try_from(value).expect("sampled int stays inside requested range");
        }
    }
    unreachable!("rejection sampling eventually accepts")
}

fn prf_u128(domain: &str, seed: &str, step: i64, attempt: u64) -> u128 {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    hash.update((seed.len() as u64).to_be_bytes());
    hash.update(seed.as_bytes());
    hash.update(step.to_be_bytes());
    hash.update(attempt.to_be_bytes());
    let digest = hash.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(bytes)
}
