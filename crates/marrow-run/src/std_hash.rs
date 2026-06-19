use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;
use sha2::{Digest, Sha256, Sha512};

use crate::env::Env;
use crate::error::{RuntimeError, std_arity, unsupported};
use crate::stdlib::eval_bytes_arg;
use crate::value::Value;

/// HMAC and SHA-256 share the same 64-byte block.
const HMAC_BLOCK: usize = 64;

pub(crate) fn eval_hash(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "sha256" => {
            let data = unary_bytes("sha256", args, span, env)?;
            Ok(Value::Bytes(Sha256::digest(&data).to_vec()))
        }
        "sha512" => {
            let data = unary_bytes("sha512", args, span, env)?;
            Ok(Value::Bytes(Sha512::digest(&data).to_vec()))
        }
        "hmacSha256" => {
            let [key, message] = args else {
                return Err(std_arity("hash", "hmacSha256", span));
            };
            let key = eval_bytes_arg(key, env, span)?;
            let message = eval_bytes_arg(message, env, span)?;
            Ok(Value::Bytes(hmac_sha256(&key, &message)))
        }
        other => Err(unsupported(&format!("std::hash::{other}"), span)),
    }
}

fn unary_bytes(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<u8>, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("hash", op, span));
    };
    eval_bytes_arg(value, env, span)
}

/// HMAC-SHA256 per RFC 2104: keys longer than the block are first hashed, then
/// the (zero-padded) key is XORed with the inner and outer pads around two
/// SHA-256 passes.
fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut block = [0u8; HMAC_BLOCK];
    if key.len() > HMAC_BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(message);

    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().to_vec()
}
