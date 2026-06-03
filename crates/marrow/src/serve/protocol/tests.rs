use super::*;

use crate::serve::test_support::{ServeState, empty_state, state_with_books};

fn request(state: &ServeState, value: Value) -> Value {
    handle_request(&state.program, &state.store, &value)
}

fn state_with_a_book() -> ServeState {
    state_with_books(&[(1, "Mort")])
}

fn state_with_two_books() -> ServeState {
    state_with_books(&[(1, "Mort"), (2, "Sourcery")])
}

fn state_with_tags(tags: &[(i64, &str)]) -> ServeState {
    let state = empty_state();
    for (pos, tag) in tags {
        write_checked_value(
            &state,
            &[
                DataQuerySegment::Root("books".into()),
                DataQuerySegment::Key(SavedKey::Int(1)),
                DataQuerySegment::Layer("tags".into()),
                DataQuerySegment::Key(SavedKey::Int(*pos)),
            ],
            tag.as_bytes(),
        );
    }
    state
}

fn write_checked_value(state: &ServeState, segments: &[DataQuerySegment], value: &[u8]) {
    let query = resolve_data_query(&state.program, segments).expect("checked fixture path");
    state
        .store
        .write_data_value(
            &query.store,
            &query.identity,
            &query.data_path,
            value.to_vec(),
        )
        .expect("write checked fixture value");
}

#[test]
fn data_roots_lists_the_roots_and_echoes_the_id() {
    let state = state_with_a_book();
    let reply = request(&state, json!({ "id": 7, "op": "data_roots" }));
    assert_eq!(reply["id"], json!(7));
    assert_eq!(reply["ok"]["roots"], json!(["books"]));
}

#[test]
fn an_empty_store_lists_no_roots() {
    let state = empty_state();
    let reply = request(&state, json!({ "id": 1, "op": "data_roots" }));
    assert_eq!(reply["ok"]["roots"], json!([]));
}

#[test]
fn an_unknown_op_is_a_protocol_error() {
    let state = empty_state();
    let reply = request(&state, json!({ "id": 1, "op": "frobnicate" }));
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_UNKNOWN_OP));
}

#[test]
fn a_request_without_an_op_is_malformed_and_echoes_a_null_id() {
    let state = empty_state();
    let reply = request(&state, json!({ "what": true }));
    assert_eq!(reply["id"], Value::Null);
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_MALFORMED));
}

#[test]
fn data_get_returns_presence_and_the_base64_value() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "id": 1, "op": "data_get",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"field": "title"}],
        }),
    );
    assert_eq!(reply["ok"]["presence"], json!("value_only"));
    assert_eq!(reply["ok"]["value"], json!("TW9ydA=="));
}

#[test]
fn data_get_of_an_absent_path_has_no_value() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "op": "data_get",
            "path": [{"root": "books"}, {"key": {"int": 2}}, {"field": "title"}],
        }),
    );
    assert_eq!(reply["ok"]["presence"], json!("absent"));
    assert_eq!(reply["ok"]["value"], Value::Null);
}

#[test]
fn data_children_lists_record_keys_then_field_names() {
    let state = state_with_a_book();
    let under_root = request(
        &state,
        json!({ "op": "data_children", "path": [{"root": "books"}] }),
    );
    assert_eq!(under_root["ok"]["children"], json!([{"key": {"int": 1}}]));
    let under_record = request(
        &state,
        json!({ "op": "data_children", "path": [{"root": "books"}, {"key": {"int": 1}}] }),
    );
    assert_eq!(under_record["ok"]["children"], json!([{"name": "title"}]));
}

#[test]
fn data_children_of_the_empty_path_lists_roots() {
    let state = state_with_a_book();
    let reply = request(&state, json!({ "op": "data_children", "path": [] }));
    assert_eq!(reply["ok"]["children"], json!([{"name": "books"}]));
}

#[test]
fn a_bad_path_segment_is_a_bad_request() {
    let state = empty_state();
    let reply = request(&state, json!({ "op": "data_get", "path": [{"frob": "x"}] }));
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn a_data_get_without_a_path_is_a_bad_request() {
    let state = empty_state();
    let reply = request(&state, json!({ "op": "data_get" }));
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn keys_of_every_type_round_trip_through_the_codec() {
    for key in [
        SavedKey::Int(7),
        SavedKey::Bool(true),
        SavedKey::Str("x".into()),
        SavedKey::Date(19_000),
        SavedKey::Duration(123_000_000_000),
        SavedKey::Instant(-5),
        SavedKey::Bytes(vec![0, 1, 2, 255]),
    ] {
        assert_eq!(decode_key(&encode_key(&key)).expect("decode"), key);
    }
}

#[test]
fn base64_round_trips_arbitrary_bytes() {
    for bytes in [
        Vec::new(),
        vec![0u8],
        vec![1, 2],
        vec![1, 2, 3],
        b"Mort".to_vec(),
        vec![0, 255, 128, 64, 32],
    ] {
        assert_eq!(base64::decode(&base64::encode(&bytes)), Some(bytes));
    }
}

#[test]
fn serve_base64_decode_rejects_non_canonical_padding() {
    for text in ["Zm8", "Zg", "Zm9vYg", "Zg===="] {
        assert!(
            decode_base64_field(&json!(text), "key").is_err(),
            "non-canonical base64 {text:?} must be rejected"
        );
        assert_eq!(base64::decode(text), None, "{text:?}");
    }
    assert_eq!(
        decode_base64_field(&json!("Zm8="), "key").expect("padded"),
        b"fo".to_vec()
    );
    assert_eq!(
        decode_base64_field(&json!("Zm9vYg=="), "key").expect("padded"),
        b"foob".to_vec()
    );
}

#[test]
fn data_walk_truncates_at_the_limit() {
    let state = state_with_two_books();
    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 1 }),
    );
    assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 1);
    assert_eq!(reply["ok"]["truncated"], json!(true));
}

#[test]
fn data_walk_cursor_resumes_after_the_previous_page() {
    let state = state_with_two_books();
    let first = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 1 }),
    );
    let cursor = first["ok"]["nextCursor"]
        .as_str()
        .expect("a truncated page returns a cursor");

    let second = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    let first_entry = &first["ok"]["entries"][0];
    let second_entry = &second["ok"]["entries"][0];
    assert_ne!(
        first_entry["path"], second_entry["path"],
        "the cursor should advance past the prior page"
    );
    assert_eq!(second["ok"]["truncated"], json!(false), "{second}");
    assert_eq!(second["ok"]["nextCursor"], Value::Null, "{second}");
}

#[test]
fn data_walk_returns_the_whole_subtree_under_a_generous_limit() {
    let state = state_with_two_books();
    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 100 }),
    );
    assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 2);
    assert_eq!(reply["ok"]["truncated"], json!(false));
}

#[test]
fn data_walk_keyed_path_filter_returns_the_requested_key() {
    let state = state_with_tags(&[
        (1, "older"),
        (2, "older"),
        (3, "older"),
        (4, "older"),
        (10, "target"),
    ]);

    let reply = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [
                {"root": "books"},
                {"key": {"int": 1}},
                {"layer": "tags"},
                {"key": {"int": 10}}
            ],
            "limit": 100,
        }),
    );

    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(
        reply["ok"]["entries"],
        json!([{"path": "^books(1).tags(10)", "value": "dGFyZ2V0"}])
    );
}

#[test]
fn data_walk_cursor_into_keyed_layer_resumes_at_the_cursor_key() {
    let state = state_with_tags(&[
        (1, "older"),
        (2, "older"),
        (3, "older"),
        (4, "older"),
        (10, "target"),
    ]);
    let first = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [
                {"root": "books"},
                {"key": {"int": 1}},
                {"layer": "tags"}
            ],
            "limit": 4,
        }),
    );
    let cursor = first["ok"]["nextCursor"]
        .as_str()
        .expect("a truncated keyed-layer page returns a cursor");

    let reply = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [
                {"root": "books"},
                {"key": {"int": 1}},
                {"layer": "tags"}
            ],
            "limit": 100,
            "cursor": cursor,
        }),
    );

    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(
        reply["ok"]["entries"],
        json!([{"path": "^books(1).tags(10)", "value": "dGFyZ2V0"}])
    );
}

#[test]
fn data_walk_rejects_a_forged_keyed_cursor_for_an_absent_entry() {
    let state = state_with_tags(&[(1, "older"), (2, "older"), (3, "older"), (4, "older")]);
    let cursor = encode_cursor(&[
        DataQuerySegment::Root("books".into()),
        DataQuerySegment::Key(SavedKey::Int(1)),
        DataQuerySegment::Layer("tags".into()),
        DataQuerySegment::Key(SavedKey::Int(10)),
    ]);

    let reply = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [
                {"root": "books"},
                {"key": {"int": 1}},
                {"layer": "tags"}
            ],
            "limit": 100,
            "cursor": cursor,
        }),
    );

    assert_eq!(
        reply["error"]["code"],
        json!(PROTOCOL_BAD_REQUEST),
        "{reply}"
    );
}

#[test]
fn data_walk_without_a_limit_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_a_keyed_layer_addressed_as_a_field() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [
                {"root": "books"},
                {"key": {"int": 1}},
                {"field": "tags"}
            ],
            "limit": 10,
        }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_an_unknown_checked_path() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"field": "missing"}],
            "limit": 10,
        }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_a_zero_limit() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 0 }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_a_negative_limit_with_a_positive_integer_message() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": -1 }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    assert_eq!(
        reply["error"]["message"],
        json!("`data_walk` requires a positive integer `limit`")
    );
}

#[test]
fn data_walk_caps_an_over_u64_integer_limit() {
    let state = state_with_two_books();
    let value: Value = serde_json::from_str(
        r#"{"op":"data_walk","path":[{"root":"books"}],"limit":18446744073709551616}"#,
    )
    .expect("json integer beyond u64");
    let reply = request(&state, value);
    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 2);
    assert_eq!(reply["ok"]["truncated"], json!(false));
}

#[test]
fn data_walk_rejects_a_malformed_cursor_inside_the_path_prefix() {
    let state = state_with_a_book();
    let cursor = base64::encode(b"^books\xff");

    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_a_forged_in_prefix_path_cursor() {
    let state = state_with_a_book();
    let cursor = base64::encode(b"^books(999999).title");

    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_a_forged_token_for_an_absent_entry() {
    let state = state_with_a_book();
    let cursor = encode_cursor(&[
        DataQuerySegment::Root("books".into()),
        DataQuerySegment::Key(SavedKey::Int(99)),
        DataQuerySegment::Field("title".into()),
    ]);

    let reply = request(
        &state,
        json!({ "op": "data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn data_walk_rejects_a_cursor_outside_the_checked_path_boundary() {
    let state = state_with_two_books();
    let cursor = base64::encode(b"^books(10).title");

    let reply = request(
        &state,
        json!({
            "op": "data_walk",
            "path": [{"root": "books"}, {"key": {"int": 1}}],
            "limit": 1,
            "cursor": cursor,
        }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn an_unknown_key_type_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "data_get", "path": [{"root": "books"}, {"key": {"frob": 1}}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn a_bytes_key_with_invalid_base64_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "data_get", "path": [{"root": "books"}, {"key": {"bytes": "!!!"}}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn a_wide_integer_key_that_is_not_an_integer_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "data_get", "path": [{"root": "books"}, {"key": {"duration": "notanint"}}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}
