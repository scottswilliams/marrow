use marrow_check::CheckedProgram;
use marrow_check::tooling::{MAX_PREVIEW_ITEMS, walk_data};
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};

use super::codec::request_query;
use super::cursor::CursorState;
use super::{ProtocolError, bad_request, tooling_error};

pub(super) const MAX_WALK: usize = MAX_PREVIEW_ITEMS;

pub(super) fn op_debug_data_walk(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let query = request_query(program, request)?;
    let limit = request_walk_limit(request)?;
    let cursor = request
        .get("cursor")
        .map(|value| cursors.decode_cursor(program, value, &query))
        .transpose()?;
    let page = walk_data(program, store, &query, cursor.as_ref(), limit).map_err(tooling_error)?;
    let entries: Vec<Value> = page
        .entries
        .into_iter()
        .map(|entry| {
            json!({
                "path": entry.path,
                "value": marrow_run::base64::encode(entry.payload.as_bytes()),
            })
        })
        .collect();
    let next_cursor = page
        .next_cursor_path
        .as_ref()
        .map(|path| cursors.encode_cursor(query.path(), path.segments()));
    Ok(json!({
        "entries": entries,
        "truncated": page.truncated,
        "nextCursor": next_cursor,
    }))
}

fn request_walk_limit(request: &Value) -> Result<usize, ProtocolError> {
    let value = request
        .get("limit")
        .ok_or_else(|| bad_request("`debug_data_walk` requires an integer `limit`"))?;
    if let Some(limit) = value.as_u64() {
        if limit == 0 {
            return Err(bad_request(
                "`debug_data_walk` requires a positive integer `limit`",
            ));
        }
        return Ok(limit.min(MAX_WALK as u64) as usize);
    }
    if value.as_i64().is_some() {
        return Err(bad_request(
            "`debug_data_walk` requires a positive integer `limit`",
        ));
    }
    let Some(number) = value.as_number() else {
        return Err(bad_request("`debug_data_walk` requires an integer `limit`"));
    };
    if number
        .as_f64()
        .is_some_and(|value| value.is_finite() && value.fract() == 0.0 && value >= u64::MAX as f64)
    {
        return Ok(MAX_WALK);
    }
    let text = number.to_string();
    if text.bytes().all(|byte| byte.is_ascii_digit()) && text != "0" {
        return Ok(MAX_WALK);
    }
    Err(bad_request("`debug_data_walk` requires an integer `limit`"))
}
