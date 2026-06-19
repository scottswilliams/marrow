use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;
use sha2::{Digest, Sha256};

use crate::env::Env;
use crate::error::{RuntimeError, std_arity};
use crate::stdlib::eval_text;
use crate::value::Value;

pub(crate) fn eval_id(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text] = args else {
        return Err(std_arity("id", op, span));
    };
    let text = eval_text(text, env, span)?;
    match op {
        "slug" => Ok(Value::Str(slug(&text))),
        "stableUuid" => Ok(Value::Str(stable_uuid(&text))),
        _ => Err(crate::error::unsupported(&format!("std::id::{op}"), span)),
    }
}

fn slug(text: &str) -> String {
    let mut output = String::new();
    let mut separated = false;
    for byte in text.bytes() {
        let ch = match byte {
            b'A'..=b'Z' => (byte + 32) as char,
            b'a'..=b'z' | b'0'..=b'9' => byte as char,
            _ => {
                if !output.is_empty() {
                    separated = true;
                }
                continue;
            }
        };
        if separated {
            output.push('-');
            separated = false;
        }
        output.push(ch);
    }
    output
}

fn stable_uuid(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    [
        crate::hex::encode(&bytes[0..4]),
        crate::hex::encode(&bytes[4..6]),
        crate::hex::encode(&bytes[6..8]),
        crate::hex::encode(&bytes[8..10]),
        crate::hex::encode(&bytes[10..16]),
    ]
    .join("-")
}
