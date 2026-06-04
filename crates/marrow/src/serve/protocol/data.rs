use marrow_check::CheckedProgram;
use marrow_check::tooling::{
    DataChild, data_children, data_children_supports_paging, data_presence_name,
    data_roots_in_store, read_data_query,
};
use marrow_run::base64;
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};

use super::codec::{encode_key, request_path, request_query};
use super::cursor::CursorState;
use super::walk::MAX_WALK;
use super::{ProtocolError, bad_request, store_error, tooling_error};

pub(super) fn op_debug_data_roots(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<Value, ProtocolError> {
    let roots = data_roots_in_store(program, store).map_err(store_error)?;
    Ok(json!({ "roots": roots }))
}

pub(super) fn op_debug_data_get(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
) -> Result<Value, ProtocolError> {
    let query = request_query(program, request)?;
    let (value, presence) = read_data_query(store, &query).map_err(store_error)?;
    Ok(json!({
        "presence": data_presence_name(presence),
        "value": value.map(|payload| base64::encode(payload.as_bytes())),
    }))
}

pub(super) fn op_debug_data_children(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let segments = request_path(request)?;
    let supports_paging =
        data_children_supports_paging(program, &segments).map_err(tooling_error)?;
    if !segments.is_empty()
        && !supports_paging
        && (request.get("limit").is_some() || request.get("cursor").is_some())
    {
        return Err(bad_request(
            "`debug_data_children` declared-member listings take no `limit` or `cursor`",
        ));
    }
    let limit = if supports_paging {
        request_children_limit(request)?
    } else {
        MAX_WALK
    };
    let resume = if supports_paging {
        let scope = marrow_check::tooling::render_query_segments(&segments);
        request
            .get("cursor")
            .map(|value| cursors.decode_children_cursor(value, &scope))
            .transpose()?
    } else {
        None
    };
    let page =
        data_children(program, store, &segments, limit, resume.as_ref()).map_err(tooling_error)?;
    let children: Vec<Value> = page
        .children
        .into_iter()
        .map(|child| match child {
            DataChild::Key(key) => json!({ "key": encode_key(&key) }),
            DataChild::Member(name) => json!({ "name": name }),
        })
        .collect();
    let cursor = match page.cursor {
        Some(anchor) => {
            let scope = marrow_check::tooling::render_query_segments(&segments);
            json!(cursors.encode_children_cursor(&scope, &anchor))
        }
        None => Value::Null,
    };
    Ok(json!({
        "children": children,
        "truncated": page.truncated,
        "cursor": cursor,
    }))
}

fn request_children_limit(request: &Value) -> Result<usize, ProtocolError> {
    let Some(value) = request.get("limit") else {
        return Ok(MAX_WALK);
    };
    let invalid = bad_request("`debug_data_children` `limit` must be a positive integer");
    let Some(number) = value.as_number() else {
        return Err(invalid);
    };
    if let Some(limit) = number.as_u64() {
        if limit == 0 {
            return Err(invalid);
        }
        return Ok(limit.min(MAX_WALK as u64) as usize);
    }
    if number.as_i64().is_some() {
        return Err(invalid);
    }
    if number
        .as_f64()
        .is_some_and(|limit| limit.is_finite() && limit.fract() == 0.0 && limit >= u64::MAX as f64)
    {
        return Ok(MAX_WALK);
    }
    let text = number.to_string();
    if text.bytes().all(|byte| byte.is_ascii_digit()) && text != "0" {
        return Ok(MAX_WALK);
    }
    Err(invalid)
}
