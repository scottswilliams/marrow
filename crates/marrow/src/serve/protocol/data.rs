use marrow_check::CheckedProgram;
use marrow_check::tooling::{
    DataChild, data_children, data_children_supports_paging, data_roots_in_store, read_data_query,
};
use marrow_run::base64;
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};

use super::codec::{
    LimitBounds, LimitDefault, encode_key, request_limit, request_path, request_query,
};
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
        "presence": presence.as_label(),
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
    // A paged listing decodes the resume cursor and encodes the next cursor against
    // the same query scope, so it is computed once here and reused for both.
    let scope = supports_paging.then(|| marrow_check::tooling::render_query_segments(&segments));
    let limit = if supports_paging {
        request_limit(
            request,
            &LimitBounds {
                default: LimitDefault::ServerMaximum,
                max: MAX_WALK,
                op: "debug_data_children",
            },
        )?
    } else {
        MAX_WALK
    };
    let resume = match &scope {
        Some(scope) => request
            .get("cursor")
            .map(|value| cursors.decode_children_cursor(value, scope))
            .transpose()?,
        None => None,
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
        // A non-null page cursor only arises from a paging listing, so the scope is
        // present here.
        Some(anchor) => {
            let scope = scope.as_ref().expect("a paged listing computed its scope");
            json!(cursors.encode_children_cursor(scope, &anchor))
        }
        None => Value::Null,
    };
    Ok(json!({
        "children": children,
        "truncated": page.truncated,
        "cursor": cursor,
    }))
}
