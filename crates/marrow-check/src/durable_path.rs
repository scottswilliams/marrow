//! Checked classification for decoded durable store paths.

use marrow_schema::KeyDef;
use marrow_store::key::SavedKey;
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};

use crate::CheckedProgram;
use crate::facts::EnumId;
use crate::resolve::resolve_store_by_root;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Root(String),
    RecordKey(SavedKey),
    Field(String),
    IndexKey(SavedKey),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathParseError {
    pub message: String,
}

pub fn parse_path(text: &str) -> Result<Vec<PathSegment>, PathParseError> {
    let mut parser = PathTextParser {
        rest: text.trim(),
        segments: Vec::new(),
        seen_member: false,
    };
    parser.parse()?;
    Ok(parser.segments)
}

pub fn display_path(segments: &[PathSegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            PathSegment::Root(name) => {
                text.push('^');
                text.push_str(name);
            }
            PathSegment::Field(name) => {
                text.push('.');
                text.push_str(name);
            }
            PathSegment::RecordKey(key) | PathSegment::IndexKey(key) => {
                text.push('(');
                text.push_str(&display_key(key));
                text.push(')');
            }
        }
    }
    text
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLeafKind {
    Scalar(ScalarType),
    Enum { enum_id: EnumId },
    Identity { store_root: String, arity: usize },
}

pub fn identity_leaf_key_mismatch(
    program: &CheckedProgram,
    store_root: &str,
    keys: &[SavedKey],
) -> Option<(ScalarType, ScalarType)> {
    let declared = checked_identity_key_defs(program, store_root)?;
    key_type_mismatch(declared, keys.iter())
}

fn key_type_mismatch<'a>(
    declared: &[KeyDef],
    found: impl Iterator<Item = &'a SavedKey>,
) -> Option<(ScalarType, ScalarType)> {
    declared
        .iter()
        .zip(found)
        .find_map(|(def, key)| match def.ty.scalar() {
            Some(expected) if expected != key.scalar_type() => Some((expected, key.scalar_type())),
            _ => None,
        })
}

fn checked_identity_key_defs<'p>(program: &'p CheckedProgram, root: &str) -> Option<&'p [KeyDef]> {
    resolve_store_by_root(program, root).map(|store| store.store.identity_keys.as_slice())
}

struct PathTextParser<'a> {
    rest: &'a str,
    segments: Vec<PathSegment>,
    seen_member: bool,
}

impl PathTextParser<'_> {
    fn parse(&mut self) -> Result<(), PathParseError> {
        let after_root = self
            .rest
            .strip_prefix('^')
            .ok_or_else(|| self.error("a saved path starts with `^root`"))?;
        let (root, rest) = split_name(after_root);
        if root.is_empty() {
            return Err(self.error("a saved root name after `^`"));
        }
        self.segments.push(PathSegment::Root(root.to_string()));
        self.rest = rest;

        while !self.rest.is_empty() {
            match self.rest.as_bytes()[0] {
                b'.' => {
                    let (name, rest) = split_name(&self.rest[1..]);
                    if name.is_empty() {
                        return Err(self.error("a member name after `.`"));
                    }
                    self.segments.push(PathSegment::Field(name.to_string()));
                    self.rest = rest;
                    self.seen_member = true;
                }
                b'(' => {
                    let close = self
                        .rest
                        .find(')')
                        .ok_or_else(|| self.error("a closing `)` for a key"))?;
                    let key = self.parse_key(&self.rest[1..close])?;
                    let segment = if self.seen_member {
                        PathSegment::IndexKey(key)
                    } else {
                        PathSegment::RecordKey(key)
                    };
                    self.segments.push(segment);
                    self.rest = &self.rest[close + 1..];
                }
                _ => return Err(self.error("`.name` or `(key)` after a path segment")),
            }
        }
        Ok(())
    }

    fn parse_key(&self, text: &str) -> Result<SavedKey, PathParseError> {
        let text = text.trim();
        if let Some(quoted) = text.strip_prefix('"') {
            let inner = quoted
                .strip_suffix('"')
                .ok_or_else(|| self.error("a closing quote in a string key"))?;
            return Ok(SavedKey::Str(unescape_string(inner)));
        }
        if let Some(hex) = text.strip_prefix("0x") {
            let bytes = decode_hex(hex).ok_or_else(|| self.error("valid hex bytes after `0x`"))?;
            return Ok(SavedKey::Bytes(bytes));
        }
        if text == "true" {
            return Ok(SavedKey::Bool(true));
        }
        if text == "false" {
            return Ok(SavedKey::Bool(false));
        }
        if let Ok(value) = text.parse::<i64>() {
            return Ok(SavedKey::Int(value));
        }
        if let Some(SavedValue::Date(days)) = decode_value(text.as_bytes(), ScalarType::Date) {
            return Ok(SavedKey::Date(days));
        }
        if let Some(SavedValue::Instant(nanos)) = decode_value(text.as_bytes(), ScalarType::Instant)
        {
            return Ok(SavedKey::Instant(nanos));
        }
        if let Some(SavedValue::Duration(nanos)) =
            decode_value(text.as_bytes(), ScalarType::Duration)
        {
            return Ok(SavedKey::Duration(nanos));
        }
        Err(self.error(
            "a key literal: an int, true/false, \"text\", 0x<hex>, or an ISO date/instant/duration",
        ))
    }

    fn error(&self, expected: &str) -> PathParseError {
        PathParseError {
            message: format!("malformed saved path: expected {expected}"),
        }
    }
}

fn split_name(text: &str) -> (&str, &str) {
    let end = text.find(['.', '(']).unwrap_or(text.len());
    (&text[..end], &text[end..])
}

fn display_key(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(value) => value.to_string(),
        SavedKey::Bool(value) => value.to_string(),
        SavedKey::Str(value) => format!("{value:?}"),
        SavedKey::Bytes(value) => {
            let mut text = String::from("0x");
            push_hex(&mut text, value);
            text
        }
        SavedKey::Date(days) => render_temporal(SavedValue::Date(*days)),
        SavedKey::Instant(nanos) => render_temporal(SavedValue::Instant(*nanos)),
        SavedKey::Duration(nanos) => render_temporal(SavedValue::Duration(*nanos)),
    }
}

fn render_temporal(value: SavedValue) -> String {
    match encode_value(&value) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| format!("{value:?}")),
        Err(_) => format!("{value:?}"),
    }
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    use std::fmt::Write;
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
}

fn unescape_string(inner: &str) -> String {
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        bytes.push((hi * 16 + lo) as u8);
    }
    Some(bytes)
}
