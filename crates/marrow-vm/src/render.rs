//! The one runtime value walker and its canonical text grammar. VM conversion renders
//! through the text grammar with its own byte limit; the CLI's JSON output supplies its
//! own [`ValueSink`] and shares the traversal. Every variable-size contribution is checked
//! before it is appended, nested aggregates share one destination, and temporal scalars
//! retain bounded scratch text from their canonical formatters.

use std::fmt::{self, Write};

use marrow_kernel::codec::key::KeyScalar;
use marrow_verify::{SealedEnumType, SealedRecordType};

use crate::Value;

/// Canonical text would exceed the caller's UTF-8 byte limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLimit;

/// One output grammar for a walked value. [`walk`] owns the traversal — nesting, record
/// field order, and the type-metadata lookups — and reports every scalar and punctuation
/// event to the sink, which owns its byte budget and spelling, so every contribution is
/// checked before it lands and no rendering retains a separately rendered child.
pub trait ValueSink {
    /// Whether record fields are visited in ascending byte order of their names rather
    /// than declaration order.
    const SORTED_FIELDS: bool;

    fn int(&mut self, value: i64) -> Result<(), TextLimit>;
    fn bool(&mut self, value: bool) -> Result<(), TextLimit>;
    fn text(&mut self, value: &str) -> Result<(), TextLimit>;
    fn bytes(&mut self, value: &[u8]) -> Result<(), TextLimit>;
    /// A date, instant, or duration in its canonical spelling.
    fn temporal(&mut self, text: &str) -> Result<(), TextLimit>;
    fn id(&mut self, keys: &[KeyScalar]) -> Result<(), TextLimit>;
    fn absent(&mut self) -> Result<(), TextLimit>;
    /// Between two payload items, fields, list items, or map entries.
    fn separator(&mut self) -> Result<(), TextLimit>;
    /// An enum's name and member, `None` where the image carries no metadata, before its
    /// `payload` items.
    fn enum_open(
        &mut self,
        name: Option<&str>,
        member: Option<&str>,
        payload: usize,
    ) -> Result<(), TextLimit>;
    fn enum_close(&mut self, payload: usize) -> Result<(), TextLimit>;
    fn record_open(&mut self) -> Result<(), TextLimit>;
    /// The label of the field whose value follows; `None` when the image names no field
    /// at that slot.
    fn field(&mut self, name: Option<&str>) -> Result<(), TextLimit>;
    fn record_close(&mut self) -> Result<(), TextLimit>;
    fn list_open(&mut self) -> Result<(), TextLimit>;
    fn list_close(&mut self) -> Result<(), TextLimit>;
    fn map_open(&mut self) -> Result<(), TextLimit>;
    /// A map key, before its value.
    fn map_key(&mut self, key: &KeyScalar) -> Result<(), TextLimit>;
    fn map_close(&mut self, len: usize) -> Result<(), TextLimit>;
}

/// Walk `value` once, in its canonical order — records in declaration order (or name
/// order, as the sink asks), lists in insertion order, maps in ascending key order — and
/// report it to `sink`. An optional reports its inner value or `absent`.
pub fn walk<S: ValueSink>(
    value: &Value,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    sink: &mut S,
) -> Result<(), TextLimit> {
    match value {
        Value::Int(v) => sink.int(*v),
        Value::Bool(v) => sink.bool(*v),
        Value::Text(v) => sink.text(v),
        Value::Bytes(v) => sink.bytes(v),
        Value::Date(v) => sink.temporal(&date_text(*v)),
        Value::Instant(v) => sink.temporal(&instant_text(*v)),
        Value::Duration(v) => sink.temporal(&marrow_temporal::format_duration(*v)),
        Value::Id(_, keys) => sink.id(keys),
        Value::Optional(None) => sink.absent(),
        Value::Optional(Some(inner)) => walk(inner, types, enums, sink),
        Value::Enum(idx, variant, payload) => {
            let enum_def = enums.get(*idx as usize);
            let variant_def = enum_def.and_then(|e| e.variants().get(*variant as usize));
            sink.enum_open(
                enum_def.map(SealedEnumType::name),
                variant_def.map(|v| v.name().as_ref()),
                payload.len(),
            )?;
            for (position, item) in payload.iter().enumerate() {
                if position > 0 {
                    sink.separator()?;
                }
                walk(item, types, enums, sink)?;
            }
            sink.enum_close(payload.len())
        }
        Value::Record(idx, slots) => {
            let fields = types
                .get(idx.index() as usize)
                .map(SealedRecordType::fields);
            let name = |position: usize| -> Option<&str> {
                fields
                    .and_then(|fields| fields.get(position))
                    .map(|field| &**field.name())
            };
            sink.record_open()?;
            let emit = |sink: &mut S, count: usize, position: usize| {
                if count > 0 {
                    sink.separator()?;
                }
                sink.field(name(position))?;
                match &slots[position] {
                    Some(inner) => walk(inner, types, enums, sink),
                    None => sink.absent(),
                }
            };
            if S::SORTED_FIELDS {
                let mut order: Vec<usize> = (0..slots.len()).collect();
                order.sort_by(|a, b| {
                    name(*a)
                        .unwrap_or("")
                        .as_bytes()
                        .cmp(name(*b).unwrap_or("").as_bytes())
                });
                for (count, position) in order.into_iter().enumerate() {
                    emit(sink, count, position)?;
                }
            } else {
                for position in 0..slots.len() {
                    emit(sink, position, position)?;
                }
            }
            sink.record_close()
        }
        Value::List(_, _, items) => {
            sink.list_open()?;
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    sink.separator()?;
                }
                walk(item, types, enums, sink)?;
            }
            sink.list_close()
        }
        Value::Map(_, _, entries) => {
            sink.map_open()?;
            for (position, (key, value)) in entries.iter().enumerate() {
                if position > 0 {
                    sink.separator()?;
                }
                sink.map_key(key)?;
                walk(value, types, enums, sink)?;
            }
            sink.map_close(entries.len())
        }
    }
}

/// The canonical text grammar over one byte budget.
struct Text {
    output: String,
    max_bytes: usize,
}

impl Text {
    fn new(max_bytes: usize) -> Self {
        Self {
            output: String::new(),
            max_bytes,
        }
    }

    fn append(&mut self, text: &str) -> Result<(), TextLimit> {
        if text.len() > self.max_bytes - self.output.len() {
            return Err(TextLimit);
        }
        self.output.push_str(text);
        Ok(())
    }

    fn write_hex(&mut self, bytes: &[u8]) -> Result<(), TextLimit> {
        let remaining = self.max_bytes - self.output.len();
        // Check the complete contribution without overflowing its doubled length.
        if remaining < 2 || bytes.len() > (remaining - 2) / 2 {
            return Err(TextLimit);
        }
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        self.output.push_str("0x");
        for &byte in bytes {
            self.output.push(char::from(DIGITS[usize::from(byte >> 4)]));
            self.output.push(char::from(DIGITS[usize::from(byte & 15)]));
        }
        Ok(())
    }

    fn write_key(&mut self, key: &KeyScalar) -> Result<(), TextLimit> {
        match key {
            KeyScalar::Int(v) => write!(self, "{v}").map_err(|_| TextLimit),
            KeyScalar::Bool(v) => self.append(if *v { "true" } else { "false" }),
            KeyScalar::Str(v) => self.append(v),
            KeyScalar::Bytes(v) => self.write_hex(v),
            KeyScalar::Date(v) => self.append(&date_text(*v)),
            KeyScalar::Instant(v) => self.append(&instant_text(*v)),
            KeyScalar::Duration(v) => self.append(&marrow_temporal::format_duration(*v)),
        }
    }

    fn write_id(&mut self, keys: &[KeyScalar]) -> Result<(), TextLimit> {
        self.append("Id(")?;
        for (position, key) in keys.iter().enumerate() {
            if position > 0 {
                self.append(", ")?;
            }
            self.write_key(key)?;
        }
        self.append(")")
    }
}

impl ValueSink for Text {
    const SORTED_FIELDS: bool = false;

    fn int(&mut self, value: i64) -> Result<(), TextLimit> {
        write!(self, "{value}").map_err(|_| TextLimit)
    }
    fn bool(&mut self, value: bool) -> Result<(), TextLimit> {
        self.append(if value { "true" } else { "false" })
    }
    fn text(&mut self, value: &str) -> Result<(), TextLimit> {
        self.append(value)
    }
    fn bytes(&mut self, value: &[u8]) -> Result<(), TextLimit> {
        self.write_hex(value)
    }
    fn temporal(&mut self, text: &str) -> Result<(), TextLimit> {
        self.append(text)
    }
    fn id(&mut self, keys: &[KeyScalar]) -> Result<(), TextLimit> {
        self.write_id(keys)
    }
    fn absent(&mut self) -> Result<(), TextLimit> {
        self.append("absent")
    }
    fn separator(&mut self) -> Result<(), TextLimit> {
        self.append(", ")
    }
    fn enum_open(
        &mut self,
        name: Option<&str>,
        member: Option<&str>,
        payload: usize,
    ) -> Result<(), TextLimit> {
        self.append(name.unwrap_or("enum"))?;
        self.append("::")?;
        self.append(member.unwrap_or("?"))?;
        if payload > 0 {
            self.append("(")?;
        }
        Ok(())
    }
    fn enum_close(&mut self, payload: usize) -> Result<(), TextLimit> {
        if payload > 0 {
            self.append(")")?;
        }
        Ok(())
    }
    fn record_open(&mut self) -> Result<(), TextLimit> {
        self.append("{")
    }
    fn field(&mut self, name: Option<&str>) -> Result<(), TextLimit> {
        if let Some(name) = name {
            self.append(name)?;
            self.append(": ")?;
        }
        Ok(())
    }
    fn record_close(&mut self) -> Result<(), TextLimit> {
        self.append("}")
    }
    fn list_open(&mut self) -> Result<(), TextLimit> {
        self.append("[")
    }
    fn list_close(&mut self) -> Result<(), TextLimit> {
        self.append("]")
    }
    fn map_open(&mut self) -> Result<(), TextLimit> {
        self.append("[")
    }
    fn map_key(&mut self, key: &KeyScalar) -> Result<(), TextLimit> {
        self.write_key(key)?;
        self.append(": ")
    }
    fn map_close(&mut self, len: usize) -> Result<(), TextLimit> {
        if len == 0 {
            self.append(":")?;
        }
        self.append("]")
    }
}

impl fmt::Write for Text {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.append(text).map_err(|_| fmt::Error)
    }
}

/// `0x`-prefixed lowercase hex, within `max_bytes` including the prefix.
pub fn hex_bytes(bytes: &[u8], max_bytes: usize) -> Result<String, TextLimit> {
    let mut text = Text::new(max_bytes);
    text.write_hex(bytes)?;
    Ok(text.output)
}

/// Decode the `0x`-prefixed even-length lowercase-hex spelling [`hex_bytes`] produces.
/// Any other spelling — a missing prefix, an odd length, an uppercase or non-hex digit —
/// is refused rather than repaired.
pub fn decode_hex_bytes(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("0x")?;
    if !hex.len().is_multiple_of(2)
        || hex
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// The canonical `YYYY-MM-DD` text of a date. A validated date always formats; a raw
/// day outside the supported range (only reachable from a hand-built value) falls back
/// to its integer so rendering never fails.
pub fn date_text(days: i32) -> String {
    marrow_temporal::format_date(days).unwrap_or_else(|| days.to_string())
}

/// The canonical UTC text of an instant, with the same out-of-range fallback.
pub fn instant_text(nanos: i128) -> String {
    marrow_temporal::format_instant(nanos).unwrap_or_else(|| nanos.to_string())
}

/// The canonical scalar text of a key, within the caller's UTF-8 byte limit.
pub fn key_text(key: &KeyScalar, max_bytes: usize) -> Result<String, TextLimit> {
    let mut text = Text::new(max_bytes);
    text.write_key(key)?;
    Ok(text.output)
}

/// `Id(k0, k1)`, within `max_bytes` including punctuation and every key.
pub fn id_text(keys: &[KeyScalar], max_bytes: usize) -> Result<String, TextLimit> {
    let mut text = Text::new(max_bytes);
    text.write_id(keys)?;
    Ok(text.output)
}

/// Canonical text within `max_bytes`. Records retain field declaration order,
/// lists insertion order, and maps ascending key order; an optional renders its
/// inner value or `absent`. Conversion admits only scalars, enums and identities,
/// but enum payloads and returned values can contain every value shape.
pub fn value_text(
    value: &Value,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    max_bytes: usize,
) -> Result<String, TextLimit> {
    let mut text = Text::new(max_bytes);
    walk(value, types, enums, &mut text)?;
    Ok(text.output)
}

#[cfg(test)]
mod tests {
    use super::{TextLimit, hex_bytes, id_text, key_text, value_text};
    use crate::Value;
    use marrow_kernel::codec::key::KeyScalar;
    use std::rc::Rc;

    #[test]
    fn limits_count_utf8_bytes_and_hex_prefixes() {
        assert_eq!(
            value_text(&Value::Text("".into()), &[], &[], 0),
            Ok(String::new())
        );
        let value = Value::Text("é".into());
        assert_eq!(value_text(&value, &[], &[], 2), Ok("é".to_string()));
        assert_eq!(value_text(&value, &[], &[], 1), Err(TextLimit));
        assert_eq!(hex_bytes(&[], 2), Ok("0x".to_string()));
        assert_eq!(hex_bytes(&[], 1), Err(TextLimit));
        assert_eq!(hex_bytes(&[0, 171, 255], 8), Ok("0x00abff".to_string()));
        assert_eq!(hex_bytes(&[0, 171, 255], 7), Err(TextLimit));
    }

    #[test]
    fn identities_share_the_key_and_punctuation_budget() {
        let keys = [
            KeyScalar::Str("é".to_string()),
            KeyScalar::Int(-1),
            KeyScalar::Bytes(vec![171]),
        ];
        assert_eq!(key_text(&keys[0], 2), Ok("é".to_string()));
        assert_eq!(key_text(&keys[0], 1), Err(TextLimit));
        let expected = "Id(é, -1, 0xab)";
        assert_eq!(id_text(&keys, expected.len()), Ok(expected.to_string()));
        assert_eq!(id_text(&keys, expected.len() - 1), Err(TextLimit));
    }

    #[test]
    fn nested_payloads_share_one_text_budget() {
        // Missing type metadata retains the existing canonical name fallbacks.
        let value = Value::Enum(
            0,
            0,
            vec![Value::list(
                0,
                Rc::new(vec![
                    Value::Text("é".into()),
                    Value::Optional(None),
                    Value::Int(-12),
                ]),
            )]
            .into_boxed_slice(),
        );
        let expected = "enum::?([é, absent, -12])";
        assert_eq!(
            value_text(&value, &[], &[], expected.len()),
            Ok(expected.to_string())
        );
        assert_eq!(
            value_text(&value, &[], &[], expected.len() - 1),
            Err(TextLimit)
        );
    }

    #[test]
    fn scalar_and_aggregate_forms_preserve_exact_limits() {
        let cases = [
            (Value::Int(i64::MIN), "-9223372036854775808"),
            (Value::Bool(false), "false"),
            (Value::list(0, Rc::new(Vec::new())), "[]"),
            (Value::map(0, Rc::new(Vec::new())), "[:]"),
            (
                Value::map(
                    0,
                    Rc::new(vec![(KeyScalar::Int(1), Value::Text("a".into()))]),
                ),
                "[1: a]",
            ),
            (
                Value::Record(
                    marrow_verify::TypeId::from_index(0),
                    vec![Some(Value::Int(7)), None].into_boxed_slice(),
                ),
                "{7, absent}",
            ),
            (Value::Optional(Some(Box::new(Value::Bool(true)))), "true"),
        ];
        for (value, expected) in cases {
            assert_eq!(
                value_text(&value, &[], &[], expected.len()),
                Ok(expected.to_string())
            );
            assert_eq!(
                value_text(&value, &[], &[], expected.len() - 1),
                Err(TextLimit)
            );
        }
    }
}
