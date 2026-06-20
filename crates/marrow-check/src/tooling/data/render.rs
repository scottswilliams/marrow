use std::collections::HashMap;

use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::{DataPathSegment as StoreDataPathSegment, decode_tree_enum_member};
use marrow_store::value::{SavedValue, decode_value, encode_value};

use super::{DataPathSegment, DataValuePreview, ResolvedDataPath};
use crate::hex::push_lower_hex;
use crate::{CheckedProgram, EnumId, StoreLeafKind, identity_leaf_key_mismatch};

const UNDECLARED_MEMBER: &str = "<undeclared member>";
const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const TRUNCATION_MARKER: &str = "...";

pub fn render_data_path_segments(segments: &[DataPathSegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            DataPathSegment::Root(name) => {
                text.push('^');
                text.push_str(name);
            }
            DataPathSegment::Field(name) | DataPathSegment::Layer(name) => {
                text.push('.');
                text.push_str(name);
            }
            DataPathSegment::Key(key) => {
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

/// Append a stored data path under an already-rendered root and identity,
/// resolving each member's catalog id to its declared source name.
pub(crate) fn render_data_path(
    text: &mut String,
    path: &[StoreDataPathSegment],
    names: &HashMap<String, String>,
) {
    for segment in path {
        match segment {
            StoreDataPathSegment::Member(member) => {
                let name = names
                    .get(member.as_str())
                    .map(String::as_str)
                    .unwrap_or(UNDECLARED_MEMBER);
                push_member(text, name);
            }
            StoreDataPathSegment::Key(key) => {
                push_key(text, key);
            }
        }
    }
}

/// Where a pushed key began, so a streaming walk can roll it back. A key that
/// extended an open composite group restores the group's closing `)`.
pub(crate) struct KeyMark {
    restore_len: usize,
    reclose_group: bool,
}

/// Append one key, opening a fresh `(...)` group unless the path already ends in
/// a key: a run of consecutive keys is one composite identity or member key and
/// renders as a single comma group that re-parses.
pub(crate) fn push_key(path: &mut String, key: &SavedKey) -> KeyMark {
    if path.ends_with(')') {
        path.pop();
        let restore_len = path.len();
        path.push(',');
        path.push_str(&render_key(key));
        path.push(')');
        KeyMark {
            restore_len,
            reclose_group: true,
        }
    } else {
        let restore_len = path.len();
        path.push('(');
        path.push_str(&render_key(key));
        path.push(')');
        KeyMark {
            restore_len,
            reclose_group: false,
        }
    }
}

pub(crate) fn pop_key(path: &mut String, mark: KeyMark) {
    path.truncate(mark.restore_len);
    if mark.reclose_group {
        path.push(')');
    }
}

pub fn render_data_value(program: &CheckedProgram, leaf: &StoreLeafKind, bytes: &[u8]) -> String {
    match leaf {
        StoreLeafKind::Scalar(ty) => {
            render_scalar_value(*ty, bytes).unwrap_or_else(|| render_hex_value(bytes))
        }
        StoreLeafKind::Identity { store_root, arity } => {
            render_identity_value(program, store_root, *arity, bytes)
                .unwrap_or_else(|| render_hex_value(bytes))
        }
        StoreLeafKind::Enum { enum_id } => {
            render_enum_value(program, *enum_id, bytes).unwrap_or_else(|| render_hex_value(bytes))
        }
    }
}

pub fn render_data_path_value(
    program: &CheckedProgram,
    path: &ResolvedDataPath,
    bytes: &[u8],
) -> String {
    match path.leaf() {
        Some(leaf) => render_data_value(program, leaf, bytes),
        None => render_hex_value(bytes),
    }
}

pub(super) fn render_data_path_value_prefix_preview(
    program: &CheckedProgram,
    path: &ResolvedDataPath,
    bytes: &[u8],
    bytes_truncated: bool,
    limit: usize,
) -> DataValuePreview {
    match path.leaf() {
        Some(leaf) => {
            render_data_value_prefix_preview(program, leaf, bytes, bytes_truncated, limit)
        }
        None => mark_source_truncated(render_hex_value_preview(bytes, limit), bytes_truncated),
    }
}

#[cfg(test)]
fn render_data_value_preview(
    program: &CheckedProgram,
    leaf: &StoreLeafKind,
    bytes: &[u8],
    limit: usize,
) -> DataValuePreview {
    render_data_value_prefix_preview(program, leaf, bytes, false, limit)
}

fn render_data_value_prefix_preview(
    program: &CheckedProgram,
    leaf: &StoreLeafKind,
    bytes: &[u8],
    bytes_truncated: bool,
    limit: usize,
) -> DataValuePreview {
    let preview =
        render_data_value_prefix_preview_inner(program, leaf, bytes, bytes_truncated, limit);
    mark_source_truncated(preview, bytes_truncated)
}

fn render_data_value_prefix_preview_inner(
    program: &CheckedProgram,
    leaf: &StoreLeafKind,
    bytes: &[u8],
    bytes_truncated: bool,
    limit: usize,
) -> DataValuePreview {
    match leaf {
        StoreLeafKind::Scalar(ty) => {
            render_scalar_value_preview(*ty, bytes, bytes_truncated, limit)
        }
        StoreLeafKind::Identity { store_root, arity } => {
            render_identity_value_preview(program, store_root, *arity, bytes, limit)
                .unwrap_or_else(|| render_hex_value_preview(bytes, limit))
        }
        StoreLeafKind::Enum { enum_id } => match render_enum_value(program, *enum_id, bytes) {
            Some(text) => bounded_rendered_text(text, limit),
            None => render_hex_value_preview(bytes, limit),
        },
    }
}

fn render_scalar_value(ty: marrow_store::value::ScalarType, bytes: &[u8]) -> Option<String> {
    match decode_value(bytes, ty)? {
        SavedValue::Str(value) => Some(format!("{value:?}")),
        SavedValue::Bytes(value) => Some(render_hex_value(&value)),
        SavedValue::Bool(value) => Some(value.to_string()),
        value => render_encoded_scalar(value),
    }
}

fn render_scalar_value_preview(
    ty: marrow_store::value::ScalarType,
    bytes: &[u8],
    bytes_truncated: bool,
    limit: usize,
) -> DataValuePreview {
    match ty {
        marrow_store::value::ScalarType::Str => {
            match render_string_value_preview(bytes, bytes_truncated, limit) {
                Some(preview) => preview,
                None => render_hex_value_preview(bytes, limit),
            }
        }
        marrow_store::value::ScalarType::Bytes => render_hex_value_preview(bytes, limit),
        _ => match render_scalar_value(ty, bytes) {
            Some(text) => bounded_rendered_text(text, limit),
            None => render_hex_value_preview(bytes, limit),
        },
    }
}

fn render_encoded_scalar(value: SavedValue) -> Option<String> {
    String::from_utf8(encode_value(&value).ok()?).ok()
}

fn render_identity_value(
    program: &CheckedProgram,
    store_root: &str,
    arity: usize,
    bytes: &[u8],
) -> Option<String> {
    let keys = decode_identity_payload_arity(bytes, arity)?;
    if identity_leaf_key_mismatch(program, store_root, &keys).is_some() {
        return None;
    }
    let mut segments = Vec::with_capacity(1 + keys.len());
    segments.push(DataPathSegment::Root(store_root.to_string()));
    segments.extend(keys.into_iter().map(DataPathSegment::Key));
    Some(render_data_path_segments(&segments))
}

fn render_identity_value_preview(
    program: &CheckedProgram,
    store_root: &str,
    arity: usize,
    bytes: &[u8],
    limit: usize,
) -> Option<DataValuePreview> {
    let keys = decode_identity_payload_arity(bytes, arity)?;
    if identity_leaf_key_mismatch(program, store_root, &keys).is_some() {
        return None;
    }
    let mut text = String::new();
    if !push_char_with_limit(&mut text, '^', limit) {
        return Some(truncated_preview(text));
    }
    if !push_str_with_limit(&mut text, store_root, limit) {
        return Some(truncated_preview(text));
    }
    for key in &keys {
        if !push_key_preview(&mut text, key, limit) {
            return Some(truncated_preview(text));
        }
    }
    Some(DataValuePreview {
        text,
        truncated: false,
    })
}

fn render_enum_value(program: &CheckedProgram, enum_id: EnumId, bytes: &[u8]) -> Option<String> {
    let stored = decode_tree_enum_member(bytes).ok()?;
    let enum_fact = program.facts.enum_(enum_id)?;
    if enum_fact.catalog_id.as_deref() != Some(stored.enum_id().as_str()) {
        return None;
    }
    let member = program.facts.enum_members().iter().find(|member| {
        member.enum_id == enum_id
            && member.catalog_id.as_deref() == Some(stored.member_id().as_str())
    })?;
    if !program.facts.enum_member_is_selectable(member.id) {
        return None;
    }
    program.facts.enum_member_catalog_path(member.id)
}

fn render_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Bytes(value) => render_hex_value(value),
        SavedKey::Date(value) => render_key_temporal(SavedValue::Date(*value)),
        SavedKey::Instant(value) => render_key_temporal(SavedValue::Instant(*value)),
        SavedKey::Duration(value) => render_key_temporal(SavedValue::Duration(*value)),
    }
}

fn render_hex_value(bytes: &[u8]) -> String {
    let mut text = String::from("0x");
    push_lower_hex(&mut text, bytes);
    text
}

fn render_string_value_preview(
    bytes: &[u8],
    bytes_truncated: bool,
    limit: usize,
) -> Option<DataValuePreview> {
    let value = string_preview_prefix(bytes, bytes_truncated)?;
    let mut text = String::new();
    if push_quoted_string_preview(&mut text, value, limit) {
        Some(DataValuePreview {
            text,
            truncated: false,
        })
    } else {
        Some(truncated_preview(text))
    }
}

fn string_preview_prefix(bytes: &[u8], bytes_truncated: bool) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(value) => Some(value),
        Err(error) if bytes_truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

fn render_hex_value_preview(bytes: &[u8], limit: usize) -> DataValuePreview {
    let mut text = String::new();
    if push_hex_preview(&mut text, bytes, limit) {
        DataValuePreview {
            text,
            truncated: false,
        }
    } else {
        truncated_preview(text)
    }
}

fn bounded_rendered_text(mut text: String, limit: usize) -> DataValuePreview {
    if text.len() <= limit {
        return DataValuePreview {
            text,
            truncated: false,
        };
    }
    truncate_to_limit(&mut text, limit);
    truncated_preview(text)
}

fn push_str_with_limit(text: &mut String, value: &str, limit: usize) -> bool {
    for ch in value.chars() {
        if !push_char_with_limit(text, ch, limit) {
            return false;
        }
    }
    true
}

fn push_str_atomic_with_limit(text: &mut String, value: &str, limit: usize) -> bool {
    if text.len() + value.len() > limit {
        return false;
    }
    text.push_str(value);
    true
}

fn push_char_with_limit(text: &mut String, ch: char, limit: usize) -> bool {
    if text.len() + ch.len_utf8() > limit {
        return false;
    }
    text.push(ch);
    true
}

/// Append one preview key, joining the trailing composite group with a comma in
/// place of its closing `)` rather than opening a fresh `(...)` when the text
/// already ends in a key, so a run of consecutive keys renders as the same
/// single comma group as the non-preview path and re-parses.
fn push_key_preview(text: &mut String, key: &SavedKey, limit: usize) -> bool {
    if text.ends_with(')') {
        text.pop();
        text.push(',');
    } else if !push_char_with_limit(text, '(', limit) {
        return false;
    }
    if !push_key_body_preview(text, key, limit) {
        return false;
    }
    push_char_with_limit(text, ')', limit)
}

fn push_key_body_preview(text: &mut String, key: &SavedKey, limit: usize) -> bool {
    match key {
        SavedKey::Str(value) => push_quoted_string_preview(text, value, limit),
        SavedKey::Bytes(value) => push_hex_preview(text, value, limit),
        _ => push_str_with_limit(text, &render_key(key), limit),
    }
}

fn push_quoted_string_preview(text: &mut String, value: &str, limit: usize) -> bool {
    let rendered = format!("{value:?}");
    if push_str_atomic_with_limit(text, &rendered, limit) {
        return true;
    }
    push_truncated_quoted_string_preview(text, value, limit)
}

fn push_truncated_quoted_string_preview(text: &mut String, value: &str, limit: usize) -> bool {
    if !push_char_with_limit(text, '"', limit) {
        return false;
    }
    for ch in value.chars() {
        let escaped = truncated_debug_char_fragment(ch);
        if !push_str_atomic_with_limit(text, &escaped, limit) {
            return false;
        }
    }
    push_char_with_limit(text, '"', limit)
}

fn truncated_debug_char_fragment(ch: char) -> String {
    let mut bytes = [0; 4];
    let value = ch.encode_utf8(&mut bytes);
    let rendered = format!("{value:?}");
    rendered[1..rendered.len() - 1].to_string()
}

fn push_hex_preview(text: &mut String, bytes: &[u8], limit: usize) -> bool {
    if !push_str_atomic_with_limit(text, "0x", limit) {
        return false;
    }
    for byte in bytes {
        if text.len() + 2 > limit {
            return false;
        }
        text.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    true
}

fn truncated_preview(mut text: String) -> DataValuePreview {
    text.push_str(TRUNCATION_MARKER);
    DataValuePreview {
        text,
        truncated: true,
    }
}

fn mark_source_truncated(preview: DataValuePreview, bytes_truncated: bool) -> DataValuePreview {
    if !bytes_truncated || preview.truncated {
        return preview;
    }
    truncated_preview(preview.text)
}

fn truncate_to_limit(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let end = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    text.truncate(end);
}

fn render_key_temporal(value: SavedValue) -> String {
    match encode_value(&value) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| format!("{value:?}")),
        Err(_) => format!("{value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use marrow_store::key::{SavedKey, encode_identity_payload};

    use super::{
        DataPathSegment, render_data_path_segments, render_data_value,
        render_data_value_prefix_preview, render_data_value_preview, render_hex_value_preview,
        render_string_value_preview,
    };
    use crate::{CheckedProgram, StoreLeafKind};

    #[test]
    fn out_of_range_temporal_keys_render_without_panicking() {
        assert_eq!(
            render_data_path_segments(&[DataPathSegment::Key(SavedKey::Date(i32::MIN))]),
            "(Date(-2147483648))"
        );
        assert_eq!(
            render_data_path_segments(&[DataPathSegment::Key(SavedKey::Instant(i128::MAX))]),
            "(Instant(170141183460469231731687303715884105727))"
        );
    }

    #[test]
    fn truncated_string_preview_appends_marker() {
        let preview = render_string_value_preview(b"aaaaaaaa", false, 4).expect("string preview");

        assert!(preview.truncated, "{preview:?}");
        assert_eq!(preview.text, "\"aaa...");
    }

    #[test]
    fn hex_preview_never_splits_byte_pairs() {
        let tight = render_hex_value_preview(&[0xab], 3);
        assert!(tight.truncated, "{tight:?}");
        assert_eq!(tight.text, "0x...");

        let one_pair = render_hex_value_preview(&[0xab, 0xcd], 5);
        assert!(one_pair.truncated, "{one_pair:?}");
        assert_eq!(one_pair.text, "0xab...");

        let complete = render_hex_value_preview(&[0xab], 4);
        assert!(!complete.truncated, "{complete:?}");
        assert_eq!(complete.text, "0xab");
    }

    #[test]
    fn identity_preview_with_large_string_and_bytes_keys_is_bounded() {
        let program = CheckedProgram::default();
        let leaf = StoreLeafKind::Identity {
            store_root: "books".into(),
            arity: 1,
        };

        let string_payload = encode_identity_payload(&[SavedKey::Str("a".repeat(256))]);
        let string_preview = render_data_value_preview(&program, &leaf, &string_payload, 16);
        assert!(string_preview.truncated, "{string_preview:?}");
        assert!(string_preview.text.len() <= 16 + "...".len());
        assert!(string_preview.text.starts_with("^books("));
        assert!(string_preview.text.ends_with("..."));

        let bytes_payload = encode_identity_payload(&[SavedKey::Bytes(vec![0xab; 256])]);
        let bytes_preview = render_data_value_preview(&program, &leaf, &bytes_payload, 16);
        assert!(bytes_preview.truncated, "{bytes_preview:?}");
        assert!(bytes_preview.text.len() <= 16 + "...".len());
        assert!(bytes_preview.text.starts_with("^books(0x"));
        assert!(bytes_preview.text.ends_with("..."));
    }

    #[test]
    fn string_preview_matches_full_rendering_for_single_quote() {
        let leaf = StoreLeafKind::Scalar(marrow_store::value::ScalarType::Str);
        let bytes = b"Bob's";

        let full = render_data_value(&CheckedProgram::default(), &leaf, bytes);
        let preview = render_data_value_preview(&CheckedProgram::default(), &leaf, bytes, 128);

        assert_eq!(preview.text, full);
        assert!(!preview.truncated, "{preview:?}");
    }

    #[test]
    fn string_preview_matches_full_rendering_for_combining_mark() {
        let leaf = StoreLeafKind::Scalar(marrow_store::value::ScalarType::Str);
        let bytes = "cafe\u{0301}".as_bytes();

        let full = render_data_value(&CheckedProgram::default(), &leaf, bytes);
        let preview = render_data_value_preview(&CheckedProgram::default(), &leaf, bytes, 128);

        assert_eq!(preview.text, full);
        assert!(!preview.truncated, "{preview:?}");
    }

    #[test]
    fn identity_string_key_preview_matches_full_rendering_for_single_quote() {
        let program = CheckedProgram::default();
        let leaf = StoreLeafKind::Identity {
            store_root: "books".into(),
            arity: 1,
        };
        let payload = encode_identity_payload(&[SavedKey::Str("Bob's".into())]);
        let full = render_data_value(&program, &leaf, &payload);
        let preview = render_data_value_preview(&program, &leaf, &payload, 128);

        assert_eq!(preview.text, full);
        assert!(!preview.truncated, "{preview:?}");
    }

    #[test]
    fn identity_string_key_preview_matches_full_rendering_for_combining_mark() {
        let program = CheckedProgram::default();
        let leaf = StoreLeafKind::Identity {
            store_root: "books".into(),
            arity: 1,
        };
        let key = SavedKey::Str("cafe\u{0301}".into());
        let payload = encode_identity_payload(std::slice::from_ref(&key));
        let full = render_data_value(&program, &leaf, &payload);
        let rendered_segments = render_data_path_segments(&[
            DataPathSegment::Root("books".into()),
            DataPathSegment::Key(key),
        ]);
        let preview = render_data_value_preview(&program, &leaf, &payload, 128);

        assert_eq!(full, rendered_segments);
        assert_eq!(preview.text, full);
        assert!(!preview.truncated, "{preview:?}");
    }

    #[test]
    fn composite_identity_preview_matches_full_comma_rendering() {
        let program = CheckedProgram::default();
        let leaf = StoreLeafKind::Identity {
            store_root: "enrolls".into(),
            arity: 2,
        };
        let payload =
            encode_identity_payload(&[SavedKey::Str("s1".into()), SavedKey::Str("c9".into())]);
        let full = render_data_value(&program, &leaf, &payload);
        let preview = render_data_value_preview(&program, &leaf, &payload, 128);

        assert_eq!(full, r#"^enrolls("s1","c9")"#);
        assert_eq!(preview.text, full);
        assert!(!preview.truncated, "{preview:?}");
    }

    #[test]
    fn source_byte_truncation_marks_preview_even_with_spare_text_budget() {
        let preview = render_data_value_prefix_preview(
            &CheckedProgram::default(),
            &StoreLeafKind::Scalar(marrow_store::value::ScalarType::Str),
            b"short",
            true,
            128,
        );

        assert_eq!(preview.text, "\"short\"...");
        assert!(preview.truncated, "{preview:?}");
    }

    #[test]
    fn utf8_prefix_preview_renders_valid_string_prefix() {
        let bytes = "abé".as_bytes();
        let preview = render_data_value_prefix_preview(
            &CheckedProgram::default(),
            &StoreLeafKind::Scalar(marrow_store::value::ScalarType::Str),
            &bytes[..3],
            true,
            128,
        );

        assert_eq!(preview.text, "\"ab\"...");
        assert!(preview.truncated, "{preview:?}");
        assert!(preview.text.starts_with('"'), "{preview:?}");
        assert!(!preview.text.starts_with("0x"), "{preview:?}");
        assert!(preview.text.ends_with("..."), "{preview:?}");
    }
}
