use super::codec::{decode_base64_field, decode_key, encode_key, encode_query_path};
use super::{PROTOCOL_BAD_REQUEST, PROTOCOL_MALFORMED, PROTOCOL_UNKNOWN_OP, ProtocolSession};

use marrow_run::base64;
use marrow_store::key::SavedKey;
use serde_json::{Value, json};

use crate::serve::test_support::{
    ServeState, empty_state, state_with_books, write_summary, write_tag,
};
use marrow_check::tooling::DataQuerySegment;

fn request(state: &ServeState, value: Value) -> Value {
    let session = ProtocolSession::new(false);
    request_with_session(&session, state, value)
}

fn request_with_session(session: &ProtocolSession, state: &ServeState, value: Value) -> Value {
    session.handle_request(&state.program, &state.store, &value)
}

fn forged_cursor(path: &[DataQuerySegment]) -> String {
    base64::encode(
        json!({
            "v": 2,
            "scope": "^books",
            "path": encode_query_path(path),
            "sig": "0000000000000000",
        })
        .to_string()
        .as_bytes(),
    )
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
        write_tag(&state, *pos, tag);
    }
    state
}

fn state_with_details(summary: &str) -> ServeState {
    let state = empty_state();
    write_summary(&state, summary);
    state
}

#[test]
fn debug_data_roots_lists_the_roots_and_echoes_the_id() {
    let state = state_with_a_book();
    let reply = request(&state, json!({ "id": 7, "op": "debug_data_roots" }));
    assert_eq!(reply["id"], json!(7));
    assert_eq!(reply["ok"]["roots"], json!(["books"]));
}

#[test]
fn an_empty_store_lists_no_roots() {
    let state = empty_state();
    let reply = request(&state, json!({ "id": 1, "op": "debug_data_roots" }));
    assert_eq!(reply["ok"]["roots"], json!([]));
}

#[test]
fn an_unknown_op_is_a_protocol_error() {
    let state = empty_state();
    let reply = request(&state, json!({ "id": 1, "op": "frobnicate" }));
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_UNKNOWN_OP));
}

#[test]
fn raw_data_ops_are_not_production_protocol_ops() {
    let state = state_with_a_book();
    for op in ["data_roots", "data_get", "data_children", "data_walk"] {
        let reply = request(
            &state,
            json!({
                "id": 1,
                "op": op,
                "path": [{"root": "books"}],
                "limit": 1,
            }),
        );
        assert_eq!(reply["error"]["code"], json!(PROTOCOL_UNKNOWN_OP), "{op}");
    }
}

#[test]
fn a_request_without_an_op_is_malformed_and_echoes_a_null_id() {
    let state = empty_state();
    let reply = request(&state, json!({ "what": true }));
    assert_eq!(reply["id"], Value::Null);
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_MALFORMED));
}

#[test]
fn debug_data_get_returns_presence_and_the_base64_value() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "id": 1, "op": "debug_data_get",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"field": "title"}],
        }),
    );
    assert_eq!(reply["ok"]["presence"], json!("value_only"));
    assert_eq!(reply["ok"]["value"], json!("TW9ydA=="));
}

#[test]
fn debug_data_get_of_an_absent_path_has_no_value() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "op": "debug_data_get",
            "path": [{"root": "books"}, {"key": {"int": 2}}, {"field": "title"}],
        }),
    );
    assert_eq!(reply["ok"]["presence"], json!("absent"));
    assert_eq!(reply["ok"]["value"], Value::Null);
}

#[test]
fn debug_data_children_lists_record_keys_then_field_names() {
    let state = state_with_a_book();
    let under_root = request(
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}] }),
    );
    assert_eq!(under_root["ok"]["children"], json!([{"key": {"int": 1}}]));
    let under_record = request(
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}, {"key": {"int": 1}}] }),
    );
    assert_eq!(under_record["ok"]["children"], json!([{"name": "title"}]));
}

#[test]
fn debug_data_children_lists_populated_keyed_leaf_layers_under_a_record() {
    let state = state_with_tags(&[(1, "favorite")]);
    let reply = request(
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}, {"key": {"int": 1}}] }),
    );
    assert_eq!(reply["ok"]["children"], json!([{"name": "tags"}]));
}

#[test]
fn debug_data_children_lists_nested_group_members() {
    let state = state_with_details("paperback");
    let under_record = request(
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}, {"key": {"int": 1}}] }),
    );
    assert_eq!(under_record["ok"]["children"], json!([{"name": "details"}]));

    let under_group = request(
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"layer": "details"}],
        }),
    );
    assert_eq!(under_group["ok"]["children"], json!([{"name": "summary"}]));
}

#[test]
fn debug_data_children_rejects_limit_and_cursor_for_declared_member_listings() {
    let state = state_with_a_book();
    for payload in [
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 1}}],
            "limit": 1,
        }),
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 1}}],
            "cursor": "not-a-member-listing-cursor",
        }),
    ] {
        let reply = request(&state, payload);
        assert_eq!(
            reply["error"]["code"],
            json!(PROTOCOL_BAD_REQUEST),
            "{reply}"
        );
        assert_eq!(
            reply["error"]["message"],
            json!("`debug_data_children` declared-member listings take no `limit` or `cursor`"),
            "{reply}"
        );
    }
}

#[test]
fn debug_data_children_of_the_empty_path_lists_roots() {
    let state = state_with_a_book();
    let reply = request(&state, json!({ "op": "debug_data_children", "path": [] }));
    assert_eq!(reply["ok"]["children"], json!([{"name": "books"}]));
}

#[test]
fn debug_data_children_pages_record_keys_and_resumes_with_the_cursor() {
    let state = state_with_books(&[(1, "Mort"), (2, "Sourcery"), (3, "Reaper")]);
    let session = ProtocolSession::new(false);
    let first = request_with_session(
        &session,
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}], "limit": 2 }),
    );
    assert_eq!(
        first["ok"]["children"],
        json!([{"key": {"int": 1}}, {"key": {"int": 2}}]),
        "{first}"
    );
    assert_eq!(first["ok"]["truncated"], json!(true), "{first}");
    let cursor = first["ok"]["cursor"]
        .as_str()
        .expect("a truncated children page returns a cursor");

    let second = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}],
            "limit": 2,
            "cursor": cursor,
        }),
    );
    assert_eq!(
        second["ok"]["children"],
        json!([{"key": {"int": 3}}]),
        "the cursor resumes after the prior page: {second}"
    );
    assert_eq!(second["ok"]["truncated"], json!(false), "{second}");
    assert_eq!(second["ok"]["cursor"], Value::Null, "{second}");
}

#[test]
fn debug_data_children_pages_keyed_layer_keys() {
    let state = state_with_tags(&[(1, "a"), (2, "b"), (3, "c")]);
    let session = ProtocolSession::new(false);
    let first = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"layer": "tags"}],
            "limit": 1,
        }),
    );
    assert_eq!(
        first["ok"]["children"],
        json!([{"key": {"int": 1}}]),
        "{first}"
    );
    assert_eq!(first["ok"]["truncated"], json!(true), "{first}");
    let cursor = first["ok"]["cursor"].as_str().expect("a cursor");

    let rest = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"layer": "tags"}],
            "limit": 100,
            "cursor": cursor,
        }),
    );
    assert_eq!(
        rest["ok"]["children"],
        json!([{"key": {"int": 2}}, {"key": {"int": 3}}]),
        "{rest}"
    );
    assert_eq!(rest["ok"]["truncated"], json!(false), "{rest}");
}

#[test]
fn debug_data_children_clamps_an_oversized_limit_rather_than_rejecting() {
    let state = state_with_books(&[(1, "Mort"), (2, "Sourcery")]);
    // A limit far above the server max returns every child rather than an error: a
    // small store fits in one page once the limit is clamped to the max.
    let reply = request(
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}],
            "limit": 1_000_000_000_000u64,
        }),
    );
    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(
        reply["ok"]["children"],
        json!([{"key": {"int": 1}}, {"key": {"int": 2}}]),
        "{reply}"
    );
    assert_eq!(reply["ok"]["truncated"], json!(false), "{reply}");
}

#[test]
fn debug_data_children_without_a_limit_uses_the_server_maximum() {
    let state = state_with_two_books();
    let reply = request(
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}] }),
    );
    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(
        reply["ok"]["children"],
        json!([{"key": {"int": 1}}, {"key": {"int": 2}}]),
        "{reply}"
    );
    assert_eq!(reply["ok"]["truncated"], json!(false), "{reply}");
}

#[test]
fn debug_data_children_rejects_a_zero_limit() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({ "op": "debug_data_children", "path": [{"root": "books"}], "limit": 0 }),
    );
    assert_eq!(
        reply["error"]["code"],
        json!(PROTOCOL_BAD_REQUEST),
        "{reply}"
    );
}

#[test]
fn debug_data_children_rejects_negative_float_and_malformed_limits() {
    let state = state_with_a_book();
    for limit in [json!(-1), json!(1.0), json!(1.5), json!("10"), Value::Null] {
        let reply = request(
            &state,
            json!({ "op": "debug_data_children", "path": [{"root": "books"}], "limit": limit }),
        );
        assert_eq!(
            reply["error"]["code"],
            json!(PROTOCOL_BAD_REQUEST),
            "{reply}"
        );
        assert_eq!(
            reply["error"]["message"],
            json!("`debug_data_children` `limit` must be a positive integer"),
            "{reply}"
        );
    }
}

#[test]
fn debug_data_children_caps_an_over_u64_integer_limit() {
    let state = state_with_two_books();
    let value: Value = serde_json::from_str(
        r#"{"op":"debug_data_children","path":[{"root":"books"}],"limit":18446744073709551616}"#,
    )
    .expect("json integer beyond u64");
    let reply = request(&state, value);
    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(
        reply["ok"]["children"],
        json!([{"key": {"int": 1}}, {"key": {"int": 2}}]),
        "{reply}"
    );
    assert_eq!(reply["ok"]["truncated"], json!(false), "{reply}");
}

#[test]
fn debug_data_children_rejects_a_cursor_replayed_under_a_different_path() {
    let state = state_with_tags(&[(1, "a"), (2, "b")]);
    let session = ProtocolSession::new(false);
    let first = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"layer": "tags"}],
            "limit": 1,
        }),
    );
    let cursor = first["ok"]["cursor"].as_str().expect("a cursor");

    // Replaying that cursor under a different request path is rejected: the cursor
    // is bound to the scope that issued it.
    let replayed = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_children",
            "path": [{"root": "books"}, {"key": {"int": 2}}, {"layer": "tags"}],
            "limit": 1,
            "cursor": cursor,
        }),
    );
    assert_eq!(
        replayed["error"]["code"],
        json!(PROTOCOL_BAD_REQUEST),
        "{replayed}"
    );
}

#[test]
fn a_bad_path_segment_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "debug_data_get", "path": [{"frob": "x"}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn a_debug_data_get_without_a_path_is_a_bad_request() {
    let state = empty_state();
    let reply = request(&state, json!({ "op": "debug_data_get" }));
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
fn debug_data_walk_truncates_at_the_limit() {
    let state = state_with_two_books();
    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1 }),
    );
    assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 1);
    assert_eq!(reply["ok"]["truncated"], json!(true));
}

#[test]
fn debug_data_walk_cursor_resumes_after_the_previous_page() {
    let state = state_with_two_books();
    let session = ProtocolSession::new(false);
    let first = request_with_session(
        &session,
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1 }),
    );
    let cursor = first["ok"]["nextCursor"]
        .as_str()
        .expect("a truncated page returns a cursor");

    let second = request_with_session(
        &session,
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
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
fn debug_data_walk_returns_the_whole_subtree_under_a_generous_limit() {
    let state = state_with_two_books();
    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 100 }),
    );
    assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 2);
    assert_eq!(reply["ok"]["truncated"], json!(false));
}

#[test]
fn debug_data_walk_keyed_path_filter_returns_the_requested_key() {
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
            "op": "debug_data_walk",
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
fn debug_data_walk_cursor_into_keyed_layer_resumes_at_the_cursor_key() {
    let state = state_with_tags(&[
        (1, "older"),
        (2, "older"),
        (3, "older"),
        (4, "older"),
        (10, "target"),
    ]);
    let session = ProtocolSession::new(false);
    let first = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_walk",
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

    let reply = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_walk",
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
fn debug_data_walk_rejects_a_forged_keyed_cursor_for_an_absent_entry() {
    let state = state_with_tags(&[(1, "older"), (2, "older"), (3, "older"), (4, "older")]);
    let cursor = forged_cursor(&[
        DataQuerySegment::Root("books".into()),
        DataQuerySegment::Key(SavedKey::Int(1)),
        DataQuerySegment::Layer("tags".into()),
        DataQuerySegment::Key(SavedKey::Int(10)),
    ]);

    let reply = request(
        &state,
        json!({
            "op": "debug_data_walk",
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
fn debug_data_walk_rejects_a_cursor_replayed_under_a_different_path() {
    let state = state_with_tags(&[(1, "older"), (2, "target")]);
    let session = ProtocolSession::new(false);
    let first = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_walk",
            "path": [
                {"root": "books"},
                {"key": {"int": 1}},
                {"layer": "tags"}
            ],
            "limit": 1,
        }),
    );
    let cursor = first["ok"]["nextCursor"]
        .as_str()
        .expect("a truncated keyed-layer page returns a cursor");

    let replayed = request_with_session(
        &session,
        &state,
        json!({
            "op": "debug_data_walk",
            "path": [{"root": "books"}, {"key": {"int": 1}}],
            "limit": 100,
            "cursor": cursor,
        }),
    );

    assert_eq!(
        replayed["error"]["code"],
        json!(PROTOCOL_BAD_REQUEST),
        "{replayed}"
    );
    assert_eq!(
        replayed["error"]["message"],
        json!("`cursor` is outside the requested path"),
        "{replayed}"
    );
}

#[test]
fn debug_data_walk_rejects_a_prefix_cursor_as_not_a_position() {
    let state = state_with_tags(&[(1, "older"), (2, "older"), (3, "older"), (4, "older")]);
    let cursor = forged_cursor(&[
        DataQuerySegment::Root("books".into()),
        DataQuerySegment::Key(SavedKey::Int(1)),
        DataQuerySegment::Layer("tags".into()),
    ]);

    let reply = request(
        &state,
        json!({
            "op": "debug_data_walk",
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
    assert_eq!(
        reply["error"]["message"],
        json!("`cursor` is not a debug_data_walk cursor"),
        "{reply}"
    );
}

#[test]
fn debug_data_walk_without_a_limit_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn debug_data_walk_rejects_a_keyed_layer_addressed_as_a_field() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "op": "debug_data_walk",
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
fn debug_data_walk_rejects_an_unknown_checked_path() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({
            "op": "debug_data_walk",
            "path": [{"root": "books"}, {"key": {"int": 1}}, {"field": "missing"}],
            "limit": 10,
        }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn debug_data_walk_rejects_a_zero_limit() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 0 }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn debug_data_walk_rejects_a_negative_limit_with_a_positive_integer_message() {
    let state = state_with_a_book();
    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": -1 }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    assert_eq!(
        reply["error"]["message"],
        json!("`debug_data_walk` requires a positive integer `limit`")
    );
}

#[test]
fn debug_data_walk_caps_an_over_u64_integer_limit() {
    let state = state_with_two_books();
    let value: Value = serde_json::from_str(
        r#"{"op":"debug_data_walk","path":[{"root":"books"}],"limit":18446744073709551616}"#,
    )
    .expect("json integer beyond u64");
    let reply = request(&state, value);
    assert_eq!(reply["error"], Value::Null, "{reply}");
    assert_eq!(reply["ok"]["entries"].as_array().expect("entries").len(), 2);
    assert_eq!(reply["ok"]["truncated"], json!(false));
}

#[test]
fn debug_data_walk_rejects_a_malformed_cursor_inside_the_path_prefix() {
    let state = state_with_a_book();
    let cursor = base64::encode(b"^books\xff");

    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn debug_data_walk_rejects_a_forged_in_prefix_path_cursor() {
    let state = state_with_a_book();
    let cursor = base64::encode(b"^books(999999).title");

    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn debug_data_walk_rejects_a_forged_token_for_an_absent_entry() {
    let state = state_with_a_book();
    let cursor = forged_cursor(&[
        DataQuerySegment::Root("books".into()),
        DataQuerySegment::Key(SavedKey::Int(99)),
        DataQuerySegment::Field("title".into()),
    ]);

    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn debug_data_walk_rejects_a_forged_token_for_an_existing_entry() {
    let state = state_with_a_book();
    let cursor = forged_cursor(&[
        DataQuerySegment::Root("books".into()),
        DataQuerySegment::Key(SavedKey::Int(1)),
        DataQuerySegment::Field("title".into()),
    ]);

    let reply = request(
        &state,
        json!({ "op": "debug_data_walk", "path": [{"root": "books"}], "limit": 1, "cursor": cursor }),
    );

    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
    assert_eq!(
        reply["error"]["message"],
        json!("`cursor` is not a debug_data_walk cursor"),
        "{reply}"
    );
}

#[test]
fn debug_data_walk_rejects_a_cursor_outside_the_checked_path_boundary() {
    let state = state_with_two_books();
    let cursor = base64::encode(b"^books(10).title");

    let reply = request(
        &state,
        json!({
            "op": "debug_data_walk",
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
        json!({ "op": "debug_data_get", "path": [{"root": "books"}, {"key": {"frob": 1}}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn a_bytes_key_with_invalid_base64_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "debug_data_get", "path": [{"root": "books"}, {"key": {"bytes": "!!!"}}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}

#[test]
fn a_wide_integer_key_that_is_not_an_integer_is_a_bad_request() {
    let state = empty_state();
    let reply = request(
        &state,
        json!({ "op": "debug_data_get", "path": [{"root": "books"}, {"key": {"duration": "notanint"}}] }),
    );
    assert_eq!(reply["error"]["code"], json!(PROTOCOL_BAD_REQUEST));
}
