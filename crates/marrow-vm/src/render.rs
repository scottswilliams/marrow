//! Canonical runtime value text. VM conversion and CLI output use this owner with
//! their own byte limits. Every variable-size contribution is checked before it is
//! appended. Nested aggregates share one destination; temporal scalars retain
//! bounded scratch text from their canonical formatters.

use std::fmt::{self, Write};

use marrow_kernel::codec::key::KeyScalar;
use marrow_verify::{SealedEnumType, SealedRecordType};

use crate::Value;

/// Canonical text would exceed the caller's UTF-8 byte limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLimit;

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

    fn hex(&mut self, bytes: &[u8]) -> Result<(), TextLimit> {
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

    fn key(&mut self, key: &KeyScalar) -> Result<(), TextLimit> {
        match key {
            KeyScalar::Int(v) => write!(self, "{v}").map_err(|_| TextLimit),
            KeyScalar::Bool(v) => self.append(if *v { "true" } else { "false" }),
            KeyScalar::Str(v) => self.append(v),
            KeyScalar::Bytes(v) => self.hex(v),
            KeyScalar::Date(v) => self.append(&date_text(*v)),
            KeyScalar::Instant(v) => self.append(&instant_text(*v)),
            KeyScalar::Duration(v) => self.append(&marrow_temporal::format_duration(*v)),
        }
    }

    fn id(&mut self, keys: &[KeyScalar]) -> Result<(), TextLimit> {
        self.append("Id(")?;
        for (position, key) in keys.iter().enumerate() {
            if position > 0 {
                self.append(", ")?;
            }
            self.key(key)?;
        }
        self.append(")")
    }

    fn enum_value(
        &mut self,
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
        enum_idx: u16,
        variant: u16,
        payload: &[Value],
    ) -> Result<(), TextLimit> {
        let enum_def = enums.get(enum_idx as usize);
        let variant_def = enum_def.and_then(|e| e.variants().get(variant as usize));
        let enum_name = enum_def.map(SealedEnumType::name).unwrap_or("enum");
        let member = variant_def.map(|v| v.name.as_ref()).unwrap_or("?");
        self.append(enum_name)?;
        self.append("::")?;
        self.append(member)?;
        if !payload.is_empty() {
            self.append("(")?;
            for (position, value) in payload.iter().enumerate() {
                if position > 0 {
                    self.append(", ")?;
                }
                self.value(value, types, enums)?;
            }
            self.append(")")?;
        }
        Ok(())
    }

    fn value(
        &mut self,
        value: &Value,
        types: &[SealedRecordType],
        enums: &[SealedEnumType],
    ) -> Result<(), TextLimit> {
        match value {
            Value::Int(v) => write!(self, "{v}").map_err(|_| TextLimit),
            Value::Bool(v) => self.append(if *v { "true" } else { "false" }),
            Value::Text(v) => self.append(v),
            Value::Bytes(v) => self.hex(v),
            Value::Date(v) => self.append(&date_text(*v)),
            Value::Instant(v) => self.append(&instant_text(*v)),
            Value::Duration(v) => self.append(&marrow_temporal::format_duration(*v)),
            Value::Enum(idx, variant, payload) => {
                self.enum_value(types, enums, *idx, *variant, payload)
            }
            Value::Id(_, keys) => self.id(keys),
            Value::Optional(None) => self.append("absent"),
            Value::Optional(Some(inner)) => self.value(inner, types, enums),
            Value::Record(idx, slots) => {
                let fields = types.get(*idx as usize).map(SealedRecordType::fields);
                self.append("{")?;
                for (position, slot) in slots.iter().enumerate() {
                    if position > 0 {
                        self.append(", ")?;
                    }
                    if let Some(field) = fields.and_then(|fields| fields.get(position)) {
                        self.append(&field.name)?;
                        self.append(": ")?;
                    }
                    match slot {
                        Some(inner) => self.value(inner, types, enums)?,
                        None => self.append("absent")?,
                    }
                }
                self.append("}")
            }
            Value::List(_, _, items) => {
                self.append("[")?;
                for (position, item) in items.iter().enumerate() {
                    if position > 0 {
                        self.append(", ")?;
                    }
                    self.value(item, types, enums)?;
                }
                self.append("]")
            }
            Value::Map(_, _, entries) => {
                self.append("[")?;
                for (position, (key, value)) in entries.iter().enumerate() {
                    if position > 0 {
                        self.append(", ")?;
                    }
                    self.key(key)?;
                    self.append(": ")?;
                    self.value(value, types, enums)?;
                }
                if entries.is_empty() {
                    self.append(":")?;
                }
                self.append("]")
            }
        }
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
    text.hex(bytes)?;
    Ok(text.output)
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
    text.key(key)?;
    Ok(text.output)
}

/// `Id(k0, k1)`, within `max_bytes` including punctuation and every key.
pub fn id_text(keys: &[KeyScalar], max_bytes: usize) -> Result<String, TextLimit> {
    let mut text = Text::new(max_bytes);
    text.id(keys)?;
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
    text.value(value, types, enums)?;
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
                Value::Record(0, vec![Some(Value::Int(7)), None].into_boxed_slice()),
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
