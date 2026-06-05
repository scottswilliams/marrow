use marrow_check::CheckedProgram;
use marrow_check::tooling::{DataQuery, DataQuerySegment, resolve_data_query};
use marrow_run::base64;
use marrow_store::key::SavedKey;
use serde_json::{Value, json};

use super::{ProtocolError, bad_request, query_error};

pub(super) fn request_path(request: &Value) -> Result<Vec<DataQuerySegment>, ProtocolError> {
    let path = request
        .get("path")
        .ok_or_else(|| bad_request("request is missing `path`"))?;
    decode_query_path(path)
}

pub(super) fn request_query(
    program: &CheckedProgram,
    request: &Value,
) -> Result<DataQuery, ProtocolError> {
    let segments = request_path(request)?;
    resolve_data_query(program, &segments).map_err(query_error)
}

pub(super) fn decode_query_path(value: &Value) -> Result<Vec<DataQuerySegment>, ProtocolError> {
    value
        .as_array()
        .ok_or_else(|| bad_request("`path` must be an array of segments"))?
        .iter()
        .map(decode_segment)
        .collect()
}

fn decode_segment(value: &Value) -> Result<DataQuerySegment, ProtocolError> {
    let (kind, inner) = one_field(value, "a path segment")?;
    let segment = match kind.as_str() {
        "root" => DataQuerySegment::Root(segment_name(inner, "root")?),
        "key" => DataQuerySegment::Key(decode_key(inner)?),
        "field" => DataQuerySegment::Field(segment_name(inner, kind)?),
        "layer" => DataQuerySegment::Layer(segment_name(inner, kind)?),
        other => return Err(bad_request(&format!("unknown path segment `{other}`"))),
    };
    Ok(segment)
}

fn segment_name(value: &Value, kind: &str) -> Result<String, ProtocolError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| bad_request(&format!("`{kind}` must name a string")))
}

pub(super) fn decode_key(value: &Value) -> Result<SavedKey, ProtocolError> {
    let (tag, inner) = one_field(value, "a key")?;
    let key = match tag.as_str() {
        "int" => SavedKey::Int(
            inner
                .as_i64()
                .ok_or_else(|| bad_request("`int` key must be an integer"))?,
        ),
        "bool" => SavedKey::Bool(
            inner
                .as_bool()
                .ok_or_else(|| bad_request("`bool` key must be a boolean"))?,
        ),
        "str" => SavedKey::Str(segment_name(inner, "str")?),
        "date" => {
            let days = inner
                .as_i64()
                .ok_or_else(|| bad_request("`date` key must be an integer"))?;
            SavedKey::Date(
                i32::try_from(days).map_err(|_| bad_request("`date` key is out of range"))?,
            )
        }
        "duration" => SavedKey::Duration(parse_i128(inner, "duration")?),
        "instant" => SavedKey::Instant(parse_i128(inner, "instant")?),
        "bytes" => SavedKey::Bytes(decode_base64_field(inner, "bytes")?),
        other => return Err(bad_request(&format!("unknown key type `{other}`"))),
    };
    Ok(key)
}

pub(super) fn encode_key(key: &SavedKey) -> Value {
    let tag = key_json_tag(key);
    let payload = match key {
        SavedKey::Int(value) => json!(value),
        SavedKey::Bool(value) => json!(value),
        SavedKey::Str(value) => json!(value),
        SavedKey::Date(value) => json!(value),
        SavedKey::Duration(value) => json!(value.to_string()),
        SavedKey::Instant(value) => json!(value.to_string()),
        SavedKey::Bytes(value) => json!(base64::encode(value)),
    };
    json!({ tag: payload })
}

fn key_json_tag(key: &SavedKey) -> &'static str {
    match key {
        SavedKey::Int(_) => "int",
        SavedKey::Bool(_) => "bool",
        SavedKey::Str(_) => "str",
        SavedKey::Date(_) => "date",
        SavedKey::Duration(_) => "duration",
        SavedKey::Instant(_) => "instant",
        SavedKey::Bytes(_) => "bytes",
    }
}

fn one_field<'a>(value: &'a Value, what: &str) -> Result<(&'a String, &'a Value), ProtocolError> {
    let object = value
        .as_object()
        .ok_or_else(|| bad_request(&format!("{what} must be a one-field object")))?;
    if object.len() != 1 {
        return Err(bad_request(&format!("{what} must have exactly one tag")));
    }
    Ok(object.iter().next().expect("exactly one field"))
}

fn parse_i128(value: &Value, kind: &str) -> Result<i128, ProtocolError> {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| bad_request(&format!("`{kind}` key must be an integer in a string")))
}

pub(super) fn decode_base64_field(value: &Value, kind: &str) -> Result<Vec<u8>, ProtocolError> {
    let text = value
        .as_str()
        .ok_or_else(|| bad_request(&format!("`{kind}` must be a base64 string")))?;
    base64::decode(text).ok_or_else(|| bad_request(&format!("`{kind}` is not valid base64")))
}

pub(super) fn encode_query_path(path: &[DataQuerySegment]) -> Value {
    Value::Array(path.iter().map(encode_query_segment).collect())
}

fn encode_query_segment(segment: &DataQuerySegment) -> Value {
    match segment {
        DataQuerySegment::Root(name) => json!({ "root": name }),
        DataQuerySegment::Field(name) => json!({ "field": name }),
        DataQuerySegment::Layer(name) => json!({ "layer": name }),
        DataQuerySegment::Key(key) => json!({ "key": encode_key(key) }),
    }
}

/// How an operation treats an absent `limit` argument.
pub(super) enum LimitDefault {
    /// `limit` is required; its absence is a bad request.
    Required,
    /// `limit` is optional; its absence falls back to the server maximum.
    ServerMaximum,
}

/// The bounds and naming an operation gives the shared `limit` parser, so the
/// non-obvious saturation rules live in one place while each op keeps its own
/// absent policy and client-facing message.
pub(super) struct LimitBounds {
    pub(super) default: LimitDefault,
    pub(super) max: usize,
    /// The operation name woven into the bad-request message text.
    pub(super) op: &'static str,
}

/// Parse a request `limit` into a positive count no larger than `bounds.max`.
///
/// A `limit` larger than the server maximum saturates to it rather than
/// failing, including JSON integers too large to fit in `u64`, so an
/// over-eager client gets a full page instead of an error. Zero, negative, and
/// non-integer limits are rejected.
pub(super) fn request_limit(request: &Value, bounds: &LimitBounds) -> Result<usize, ProtocolError> {
    let Some(value) = request.get("limit") else {
        return match bounds.default {
            LimitDefault::Required => Err(bad_request(&format!(
                "`{}` requires a positive integer `limit`",
                bounds.op
            ))),
            LimitDefault::ServerMaximum => Ok(bounds.max),
        };
    };
    let Some(number) = value.as_number() else {
        return Err(positive_limit_error(bounds));
    };
    if let Some(limit) = number.as_u64() {
        if limit == 0 {
            return Err(positive_limit_error(bounds));
        }
        return Ok(limit.min(bounds.max as u64) as usize);
    }
    if number.as_i64().is_some() {
        return Err(positive_limit_error(bounds));
    }
    if is_over_u64_integer(number) {
        return Ok(bounds.max);
    }
    Err(positive_limit_error(bounds))
}

/// A finite positive JSON integer too large to fit in `u64`, whether serde kept
/// it as a float or an arbitrary-precision integer literal.
fn is_over_u64_integer(number: &serde_json::Number) -> bool {
    if number
        .as_f64()
        .is_some_and(|value| value.is_finite() && value.fract() == 0.0 && value >= u64::MAX as f64)
    {
        return true;
    }
    let text = number.to_string();
    text.bytes().all(|byte| byte.is_ascii_digit()) && text != "0"
}

fn positive_limit_error(bounds: &LimitBounds) -> ProtocolError {
    bad_request(&format!(
        "`{}` `limit` must be a positive integer",
        bounds.op
    ))
}
