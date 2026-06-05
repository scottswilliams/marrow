use marrow_check::CheckedProgram;
use marrow_check::tooling::{MAX_PREVIEW_ITEMS, walk_data};
use marrow_store::tree::TreeStore;
use serde_json::{Value, json};

use super::codec::{LimitBounds, LimitDefault, request_limit, request_query};
use super::cursor::CursorState;
use super::{ProtocolError, tooling_error};

pub(super) const MAX_WALK: usize = MAX_PREVIEW_ITEMS;

pub(super) fn op_debug_data_walk(
    program: &CheckedProgram,
    store: &TreeStore,
    request: &Value,
    cursors: &CursorState,
) -> Result<Value, ProtocolError> {
    let query = request_query(program, request)?;
    let limit = request_limit(
        request,
        &LimitBounds {
            default: LimitDefault::Required,
            max: MAX_WALK,
            op: "debug_data_walk",
        },
    )?;
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
