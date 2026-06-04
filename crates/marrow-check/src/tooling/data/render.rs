use marrow_store::key::SavedKey;
use marrow_store::value::{SavedValue, encode_value};

use super::DataQuerySegment;

pub fn render_query_segments(segments: &[DataQuerySegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            DataQuerySegment::Root(name) => {
                text.push('^');
                text.push_str(name);
            }
            DataQuerySegment::Field(name) | DataQuerySegment::Layer(name) => {
                text.push('.');
                text.push_str(name);
            }
            DataQuerySegment::Key(key) => {
                push_key(&mut text, key);
            }
        }
    }
    text
}

pub(crate) fn push_member(path: &mut String, name: &str) -> usize {
    let prior_len = path.len();
    path.push('.');
    path.push_str(name);
    prior_len
}

pub(crate) fn push_key(path: &mut String, key: &SavedKey) -> usize {
    let prior_len = path.len();
    path.push('(');
    path.push_str(&render_key(key));
    path.push(')');
    prior_len
}

fn render_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Bytes(value) => {
            let mut text = String::from("0x");
            push_hex(&mut text, value);
            text
        }
        SavedKey::Date(value) => render_key_temporal(SavedValue::Date(*value)),
        SavedKey::Instant(value) => render_key_temporal(SavedValue::Instant(*value)),
        SavedKey::Duration(value) => render_key_temporal(SavedValue::Duration(*value)),
    }
}

fn render_key_temporal(value: SavedValue) -> String {
    String::from_utf8(encode_value(&value).expect("temporal key values encode"))
        .expect("temporal key encodings are ascii")
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
}
