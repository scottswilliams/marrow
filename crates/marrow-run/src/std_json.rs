use marrow_check::CheckedArg as ExecArg;
use marrow_store::{Decimal, DecimalParseError};
use marrow_syntax::SourceSpan;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::result::Result as StdResult;

use crate::collection::absent_read;
use crate::env::Env;
use crate::error::{RuntimeError, decimal_overflow, std_arity, type_error};
use crate::stdlib::{eval_string_sequence, eval_text};
use crate::value::Value;

const MAX_BYTES: usize = 1_048_576;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 10_000;
const MAX_STRING_BYTES: usize = 65_536;

#[derive(Clone, Copy)]
enum JsonScalarOp {
    String,
    Int,
    Decimal,
    Bool,
    Count,
}

impl JsonScalarOp {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "int" => Some(Self::Int),
            "decimal" => Some(Self::Decimal),
            "bool" => Some(Self::Bool),
            "count" => Some(Self::Count),
            _ => None,
        }
    }
}

pub(crate) fn eval_json(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "valid" => {
            let [text] = args else {
                return Err(std_arity("json", op, span));
            };
            let text = eval_text(text, env, span)?;
            Ok(Value::Bool(parse_json(&text).is_ok()))
        }
        "stringLit" => {
            let [text] = args else {
                return Err(std_arity("json", op, span));
            };
            Ok(Value::Str(string_literal(&eval_text(text, env, span)?)))
        }
        "stringArray" => {
            let [items] = args else {
                return Err(std_arity("json", op, span));
            };
            let literals: Vec<String> = eval_string_sequence(items, env, span)?
                .iter()
                .map(|item| string_literal(item))
                .collect();
            Ok(Value::Str(format!("[{}]", literals.join(","))))
        }
        _ => {
            let Some(scalar_op) = JsonScalarOp::from_name(op) else {
                return Err(crate::error::unsupported(&format!("std::json::{op}"), span));
            };
            let [text, pointer] = args else {
                return Err(std_arity("json", op, span));
            };
            let text = eval_text(text, env, span)?;
            let pointer = eval_text(pointer, env, span)?;
            let root = parse_json(&text).map_err(|_| type_error("invalid JSON text", span))?;
            let Some(value) = select_pointer(root.get(), &pointer, span)? else {
                return Err(absent_read("JSON pointer selected no value".into(), span));
            };
            if is_null(&value) {
                return Err(absent_read("JSON pointer selected null".into(), span));
            }
            json_value(scalar_op, &value, span)
        }
    }
}

fn parse_json(text: &str) -> Result<Box<RawValue>, ()> {
    if text.len() > MAX_BYTES {
        return Err(());
    }
    reject_negative_zero_integer(text)?;
    validate_json(text)?;
    serde_json::from_str::<Box<RawValue>>(text).map_err(|_| ())
}

fn validate_json(text: &str) -> Result<(), ()> {
    let mut limits = JsonLimits {
        nodes: 0,
        max_depth: 0,
        max_string_bytes: 0,
    };
    let mut deserializer = serde_json::Deserializer::from_str(text);
    JsonPolicySeed {
        limits: &mut limits,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| ())?;
    deserializer.end().map_err(|_| ())
}

struct JsonPolicySeed<'a> {
    limits: &'a mut JsonLimits,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for JsonPolicySeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> StdResult<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for JsonPolicySeed<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("valid bounded JSON without duplicate object keys")
    }

    fn visit_bool<E>(mut self, _value: bool) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.record_node()
    }

    fn visit_i64<E>(mut self, _value: i64) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.record_node()
    }

    fn visit_u64<E>(mut self, _value: u64) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.record_node()
    }

    fn visit_f64<E>(mut self, _value: f64) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.record_node()
    }

    fn visit_str<E>(mut self, value: &str) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.record_node()?;
        self.record_string(value)
    }

    fn visit_string<E>(self, value: String) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_unit<E>(mut self) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.record_node()
    }

    fn visit_none<E>(self) -> StdResult<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_some<D>(self, deserializer: D) -> StdResult<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        self.deserialize(deserializer)
    }

    fn visit_seq<A>(mut self, mut seq: A) -> StdResult<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        self.record_node()?;
        let depth = self.depth + 1;
        while seq
            .next_element_seed(JsonPolicySeed {
                limits: self.limits,
                depth,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A>(mut self, mut map: A) -> StdResult<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        self.record_node()?;
        let mut keys = HashSet::new();
        let depth = self.depth + 1;
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            self.record_string(&key)?;
            map.next_value_seed(JsonPolicySeed {
                limits: self.limits,
                depth,
            })?;
        }
        Ok(())
    }
}

impl JsonPolicySeed<'_> {
    fn record_node<E>(&mut self) -> StdResult<(), E>
    where
        E: de::Error,
    {
        self.limits.nodes += 1;
        self.limits.max_depth = self.limits.max_depth.max(self.depth);
        if self.limits.nodes > MAX_NODES || self.limits.max_depth > MAX_DEPTH {
            return Err(de::Error::custom("JSON structure is too large"));
        }
        Ok(())
    }

    fn record_string<E>(&mut self, value: &str) -> StdResult<(), E>
    where
        E: de::Error,
    {
        self.limits.max_string_bytes = self.limits.max_string_bytes.max(value.len());
        if self.limits.max_string_bytes > MAX_STRING_BYTES {
            return Err(de::Error::custom("JSON string is too large"));
        }
        Ok(())
    }
}

fn reject_negative_zero_integer(text: &str) -> Result<(), ()> {
    let mut in_string = false;
    let mut escaped = false;
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'-' if bytes.get(index + 1) == Some(&b'0')
                && !matches!(bytes.get(index + 2), Some(b'.' | b'e' | b'E' | b'0'..=b'9')) =>
            {
                return Err(());
            }
            _ => {}
        }
        index += 1;
    }
    Ok(())
}

struct JsonLimits {
    nodes: usize,
    max_depth: usize,
    max_string_bytes: usize,
}

fn select_pointer(
    root: &str,
    pointer: &str,
    span: SourceSpan,
) -> Result<Option<String>, RuntimeError> {
    let mut current = root.trim().to_string();
    if pointer.is_empty() {
        return Ok(Some(current));
    }
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(type_error(
            "JSON pointer must be empty or start with `/`",
            span,
        ));
    };
    for raw in rest.split('/') {
        let token = decode_pointer_token(raw, span)?;
        match json_kind(&current) {
            Some(b'{') => {
                let fields = parse_raw_object(&current, span)?;
                let Some(next) = fields.get(&token) else {
                    return Ok(None);
                };
                current = next.get().trim().to_string();
            }
            Some(b'[') => {
                if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
                    return Ok(None);
                }
                if token.len() > 1 && token.starts_with('0') {
                    return Err(type_error(
                        "JSON pointer array index has a leading zero",
                        span,
                    ));
                }
                let Ok(index) = token.parse::<usize>() else {
                    return Ok(None);
                };
                let items = parse_raw_array(&current, span)?;
                let Some(next) = items.get(index) else {
                    return Ok(None);
                };
                current = next.get().trim().to_string();
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(current))
}

fn decode_pointer_token(raw: &str, span: SourceSpan) -> Result<String, RuntimeError> {
    let mut decoded = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            _ => return Err(type_error("JSON pointer has an invalid escape", span)),
        }
    }
    Ok(decoded)
}

fn json_value(op: JsonScalarOp, value: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    match (op, json_kind(value)) {
        (JsonScalarOp::String, Some(b'"')) => serde_json::from_str::<String>(value)
            .map(Value::Str)
            .map_err(|_| type_error("JSON value is not a string", span)),
        (JsonScalarOp::Int, Some(b'-' | b'0'..=b'9')) => parse_json_number(value, span)?
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| type_error("JSON number is not an int", span)),
        (JsonScalarOp::Decimal, Some(b'-' | b'0'..=b'9')) => parse_json_decimal(value, span),
        (JsonScalarOp::Bool, Some(b't' | b'f')) => serde_json::from_str::<bool>(value)
            .map(Value::Bool)
            .map_err(|_| type_error("JSON value is not a bool", span)),
        (JsonScalarOp::Count, Some(b'[')) => {
            Ok(Value::Int(parse_raw_array(value, span)?.len() as i64))
        }
        (JsonScalarOp::Count, Some(b'{')) => {
            Ok(Value::Int(parse_raw_object(value, span)?.len() as i64))
        }
        (JsonScalarOp::String, _)
        | (JsonScalarOp::Int, _)
        | (JsonScalarOp::Decimal, _)
        | (JsonScalarOp::Bool, _)
        | (JsonScalarOp::Count, _) => Err(type_error("JSON value has the wrong kind", span)),
    }
}

fn parse_raw_object(
    value: &str,
    span: SourceSpan,
) -> Result<BTreeMap<String, Box<RawValue>>, RuntimeError> {
    serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(value)
        .map_err(|_| type_error("JSON value is not an object", span))
}

fn parse_raw_array(value: &str, span: SourceSpan) -> Result<Vec<Box<RawValue>>, RuntimeError> {
    serde_json::from_str::<Vec<Box<RawValue>>>(value)
        .map_err(|_| type_error("JSON value is not an array", span))
}

fn parse_json_number(value: &str, span: SourceSpan) -> Result<serde_json::Number, RuntimeError> {
    serde_json::from_str::<serde_json::Number>(value)
        .map_err(|_| type_error("JSON value is not a number", span))
}

fn parse_json_decimal(value: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    // A JSON number is external data, so a non-canonical spelling such as `9.50`
    // or `9.0` is a valid number that canonicalizes to its one stored value, not a
    // malformed literal.
    match Decimal::parse_relaxed(value.trim()) {
        Ok(decimal) => Ok(Value::Decimal(decimal)),
        Err(DecimalParseError::Overflow) => Err(decimal_overflow(span)),
        Err(DecimalParseError::Malformed) => Err(type_error("JSON number is not a decimal", span)),
    }
}

fn is_null(value: &str) -> bool {
    value.trim() == "null"
}

fn json_kind(value: &str) -> Option<u8> {
    value.bytes().find(|byte| !byte.is_ascii_whitespace())
}

/// Render `text` as a correctly escaped JSON string literal, including the
/// surrounding quotes. The one owner of JSON string escaping across the runtime.
pub(crate) fn string_literal(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}
