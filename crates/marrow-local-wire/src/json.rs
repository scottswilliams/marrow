//! The canonical JSON value model and codec — the single owner of local-wire
//! canonical JSON.
//!
//! A [`Json`] is the closed value tree that crosses the wire: `null`, a boolean, a
//! 64-bit integer (Marrow has no floating-point value, so the wire has no
//! fractional number), a string, an array, and an object. [`encode`] emits the one
//! canonical byte spelling — object keys sorted ascending by byte with no
//! whitespace, minimal integer spellings, and the fixed string escapes below.
//! [`parse_strict`] is the inverse and the gatekeeper: it accepts a value only in
//! that exact canonical form, so every wire message has exactly one legal encoding.
//!
//! Canonicality is enforced by construction: the parser accepts a tolerant JSON
//! grammar (bounded in depth and string length so a hostile payload cannot drive
//! unbounded work), then requires the re-encoding of the parsed value to be
//! byte-identical to the input. Whitespace, unsorted keys, a non-minimal number, or
//! a non-canonical escape all survive parsing but fail that equality and are
//! rejected as [`WireError::Noncanonical`]; a structurally invalid body, a
//! non-integer number, or trailing bytes are [`WireError::Malformed`]; a duplicate
//! object key is rejected during parsing.
//!
//! The string escaping is the same discipline the CLI's JSONL surface uses
//! (`marrow`'s `outcome` owner): `\"`, `\\`, `\b`, `\t`, `\n`, `\f`, `\r`, other C0
//! as lowercase `\u00xx`, and every other character — including `/` and all
//! non-ASCII — passed through literally. The two encoders are independent because
//! this crate must not depend on the VM value model the CLI encoder renders, but
//! they implement one documented rule, each pinned by its own known-answer test.

use crate::error::WireError;
use crate::{MAX_DEPTH, MAX_STRING_BYTES};

/// A canonical JSON value: the closed set that may appear in a wire message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// A 64-bit signed integer. The wire has no fractional or exponent number.
    Int(i64),
    Str(String),
    Array(Vec<Json>),
    /// An object. Keys are unique; [`encode`] sorts them ascending by byte, and
    /// [`parse_strict`] accepts them only already so sorted.
    Object(Vec<(String, Json)>),
}

/// Encode a value in its one canonical byte spelling.
pub fn encode(value: &Json) -> String {
    let mut out = String::new();
    encode_into(value, &mut out);
    out
}

fn encode_into(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(n) => out.push_str(&n.to_string()),
        Json::Str(s) => encode_string(s, out),
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_into(item, out);
            }
            out.push(']');
        }
        Json::Object(pairs) => {
            // Canonical output sorts keys ascending by byte regardless of the order
            // the pairs were built in, so a value the runner assembles field-by-field
            // still encodes canonically.
            let mut ordered: Vec<&(String, Json)> = pairs.iter().collect();
            ordered.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push('{');
            for (i, (key, val)) in ordered.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_string(key, out);
                out.push(':');
                encode_into(val, out);
            }
            out.push('}');
        }
    }
}

/// Append the canonical JSON string encoding of `text`.
fn encode_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                let byte = c as u32;
                out.push(hex_digit((byte >> 4) as u8));
                out.push(hex_digit((byte & 0xf) as u8));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn hex_digit(nibble: u8) -> char {
    char::from_digit(u32::from(nibble), 16).expect("nibble is one hex digit")
}

/// Parse a value, accepting only its canonical form. See the module docs for how
/// malformed, non-canonical, and over-limit inputs are distinguished.
pub fn parse_strict(input: &[u8]) -> Result<Json, WireError> {
    let text = std::str::from_utf8(input).map_err(|_| WireError::Malformed)?;
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = parser.parse_value(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(WireError::Malformed);
    }
    if encode(&value).as_bytes() != input {
        return Err(WireError::Noncanonical);
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self, depth: usize) -> Result<Json, WireError> {
        self.skip_ws();
        match self.peek().ok_or(WireError::Malformed)? {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth),
            b'"' => Ok(Json::Str(self.parse_string()?)),
            b't' => self.parse_literal(b"true", Json::Bool(true)),
            b'f' => self.parse_literal(b"false", Json::Bool(false)),
            b'n' => self.parse_literal(b"null", Json::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(WireError::Malformed),
        }
    }

    fn parse_literal(&mut self, word: &[u8], value: Json) -> Result<Json, WireError> {
        if self.bytes[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(WireError::Malformed)
        }
    }

    fn parse_number(&mut self) -> Result<Json, WireError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == digits_start {
            return Err(WireError::Malformed);
        }
        // A fraction or exponent is not an integer value Marrow can carry.
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err(WireError::Malformed);
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).expect("ascii digits");
        // A well-formed but out-of-i64 integer (or a lone `-`) is not representable.
        let n = text.parse::<i64>().map_err(|_| WireError::Malformed)?;
        Ok(Json::Int(n))
    }

    fn parse_string(&mut self) -> Result<String, WireError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        let mut out = String::new();
        loop {
            let byte = self.peek().ok_or(WireError::Malformed)?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    self.parse_escape(&mut out)?;
                }
                // A raw control character is not legal inside a JSON string.
                0x00..=0x1f => return Err(WireError::Malformed),
                lead => {
                    let len = utf8_len(lead).ok_or(WireError::Malformed)?;
                    let end = self.pos + len;
                    let slice = self.bytes.get(self.pos..end).ok_or(WireError::Malformed)?;
                    // The input was validated UTF-8, so this slice is a whole char.
                    out.push_str(std::str::from_utf8(slice).map_err(|_| WireError::Malformed)?);
                    self.pos = end;
                }
            }
            if out.len() > MAX_STRING_BYTES {
                return Err(WireError::StringLimit);
            }
        }
    }

    fn parse_escape(&mut self, out: &mut String) -> Result<(), WireError> {
        let esc = self.peek().ok_or(WireError::Malformed)?;
        self.pos += 1;
        match esc {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{08}'),
            b'f' => out.push('\u{0C}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let code = self.parse_hex4()?;
                let ch = if (0xd800..=0xdbff).contains(&code) {
                    // A high surrogate must be followed by `\uXXXX` low surrogate.
                    if self.peek() != Some(b'\\') {
                        return Err(WireError::Malformed);
                    }
                    self.pos += 1;
                    if self.peek() != Some(b'u') {
                        return Err(WireError::Malformed);
                    }
                    self.pos += 1;
                    let low = self.parse_hex4()?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return Err(WireError::Malformed);
                    }
                    let combined = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                    char::from_u32(combined).ok_or(WireError::Malformed)?
                } else {
                    char::from_u32(code).ok_or(WireError::Malformed)?
                };
                out.push(ch);
            }
            _ => return Err(WireError::Malformed),
        }
        Ok(())
    }

    fn parse_hex4(&mut self) -> Result<u32, WireError> {
        let slice = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or(WireError::Malformed)?;
        let mut value = 0u32;
        for &b in slice {
            let digit = (b as char).to_digit(16).ok_or(WireError::Malformed)?;
            value = value * 16 + digit;
        }
        self.pos += 4;
        Ok(value)
    }

    fn parse_array(&mut self, depth: usize) -> Result<Json, WireError> {
        if depth + 1 > MAX_DEPTH {
            return Err(WireError::DepthLimit);
        }
        self.pos += 1; // consume '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.parse_value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(WireError::Malformed),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Json, WireError> {
        if depth + 1 > MAX_DEPTH {
            return Err(WireError::DepthLimit);
        }
        self.pos += 1; // consume '{'
        let mut pairs: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(WireError::Malformed);
            }
            let key = self.parse_string()?;
            // A canonical object has unique keys.
            if pairs.iter().any(|(existing, _)| existing == &key) {
                return Err(WireError::Noncanonical);
            }
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(WireError::Malformed);
            }
            self.pos += 1;
            let value = self.parse_value(depth + 1)?;
            pairs.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(pairs));
                }
                _ => return Err(WireError::Malformed),
            }
        }
    }
}

/// The UTF-8 byte length of a character from its lead byte, or `None` for a
/// continuation or invalid lead (unreachable on validated UTF-8 at a boundary).
fn utf8_len(lead: u8) -> Option<usize> {
    match lead {
        0x00..=0x7f => Some(1),
        0xc0..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf7 => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Json, encode, parse_strict};
    use crate::error::WireError;

    fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Known-answer encodings pin the canonical spelling of each value shape.
    #[test]
    fn canonical_encodings_are_frozen() {
        assert_eq!(encode(&Json::Null), "null");
        assert_eq!(encode(&Json::Bool(true)), "true");
        assert_eq!(encode(&Json::Int(0)), "0");
        assert_eq!(encode(&Json::Int(-42)), "-42");
        assert_eq!(encode(&Json::Int(i64::MAX)), "9223372036854775807");
        assert_eq!(encode(&Json::Str("a/b".to_string())), r#""a/b""#);
        assert_eq!(
            encode(&Json::Str("\u{01}\t\n\"\\".to_string())),
            "\"\\u0001\\t\\n\\\"\\\\\""
        );
        assert_eq!(encode(&Json::Str("café ☕".to_string())), "\"café ☕\"");
        assert_eq!(
            encode(&Json::Array(vec![Json::Int(1), Json::Int(2)])),
            "[1,2]"
        );
        // Keys sort ascending by byte regardless of build order.
        assert_eq!(
            encode(&obj(
                vec![("line", Json::Int(7)), ("column", Json::Int(2)),]
            )),
            r#"{"column":2,"line":7}"#
        );
    }

    #[test]
    fn canonical_inputs_round_trip() {
        for canonical in [
            "null",
            "true",
            "false",
            "0",
            "-1",
            "123",
            r#""hi""#,
            r#""a/b""#,
            "[]",
            "[1,2,3]",
            "{}",
            r#"{"a":1,"b":[true,null]}"#,
            r#"{"column":2,"line":7}"#,
        ] {
            let value = parse_strict(canonical.as_bytes())
                .unwrap_or_else(|e| panic!("{canonical} should parse: {e:?}"));
            assert_eq!(encode(&value), canonical, "round trip for {canonical}");
        }
    }

    #[test]
    fn noncanonical_forms_are_rejected() {
        for input in [
            " 1",               // leading whitespace
            "1 ",               // trailing whitespace
            "[1, 2]",           // whitespace after comma
            r#"{"b":1,"a":2}"#, // unsorted keys
            r#"{ "a":1 }"#,     // interior whitespace
            "01",               // leading zero
            "-0",               // non-minimal zero
            "\"a\\/b\"",        // escaped slash (canonical is a literal '/')
            "\"\\u0041\"",      // escape of a printable ('A')
            "\"\\u00FF\"",      // uppercase-hex escape of a literal char
        ] {
            assert_eq!(
                parse_strict(input.as_bytes()),
                Err(WireError::Noncanonical),
                "{input} must be noncanonical"
            );
        }
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        assert_eq!(
            parse_strict(br#"{"a":1,"a":2}"#),
            Err(WireError::Noncanonical)
        );
    }

    #[test]
    fn malformed_forms_are_rejected() {
        for input in [
            "",                     // empty
            "{",                    // unterminated object
            "[1",                   // unterminated array
            "nul",                  // truncated literal
            "1.5",                  // fractional number
            "1e3",                  // exponent
            "+1",                   // leading plus
            r#""abc"#,              // unterminated string
            "truefalse",            // trailing bytes
            "99999999999999999999", // out of i64 range
            "\"\\x\"",              // bad escape
        ] {
            assert_eq!(
                parse_strict(input.as_bytes()),
                Err(WireError::Malformed),
                "{input} must be malformed"
            );
        }
        // Invalid UTF-8 is malformed.
        assert_eq!(parse_strict(&[0xff, 0xfe]), Err(WireError::Malformed));
    }

    #[test]
    fn depth_and_string_limits_hold() {
        // A value nested past the depth bound is rejected before it is materialized.
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert_eq!(parse_strict(deep.as_bytes()), Err(WireError::DepthLimit));

        // A string past the byte bound is rejected.
        let long = format!("\"{}\"", "a".repeat(crate::MAX_STRING_BYTES + 1));
        assert_eq!(parse_strict(long.as_bytes()), Err(WireError::StringLimit));
    }

    /// A deterministic xorshift PRNG: no external crate on the fuzz path.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    fn random_json(rng: &mut Rng, depth: usize) -> Json {
        let leaf = depth >= 4;
        match rng.below(if leaf { 4 } else { 6 }) {
            0 => Json::Null,
            1 => Json::Bool(rng.next() & 1 == 0),
            2 => Json::Int(rng.next() as i64),
            3 => Json::Str(format!("s{}", rng.below(1000))),
            4 => {
                let n = rng.below(4);
                Json::Array((0..n).map(|_| random_json(rng, depth + 1)).collect())
            }
            _ => {
                let n = rng.below(4);
                let mut pairs = Vec::new();
                for _ in 0..n {
                    let key = format!("k{}", rng.below(1000));
                    if pairs.iter().any(|(k, _): &(String, Json)| k == &key) {
                        continue;
                    }
                    pairs.push((key, random_json(rng, depth + 1)));
                }
                Json::Object(pairs)
            }
        }
    }

    /// Structured fuzz: any value the model can build encodes to a canonical form
    /// that parses back to the same value.
    #[test]
    fn build_encode_parse_is_identity() {
        let mut rng = Rng(0x1234_5678_9abc_def0);
        for _ in 0..2000 {
            let value = random_json(&mut rng, 0);
            let bytes = encode(&value);
            let parsed = parse_strict(bytes.as_bytes())
                .unwrap_or_else(|e| panic!("canonical {bytes} should parse: {e:?}"));
            assert_eq!(encode(&parsed), bytes);
        }
    }

    /// Byte fuzz: the parser never panics on arbitrary input and always terminates
    /// with a typed result.
    #[test]
    fn parser_never_panics_on_arbitrary_bytes() {
        let mut rng = Rng(0x0fed_cba9_8765_4321);
        for _ in 0..20_000 {
            let len = rng.below(48);
            let bytes: Vec<u8> = (0..len).map(|_| (rng.next() & 0xff) as u8).collect();
            // Only assertion: it returns (Ok or Err), never panics or hangs.
            let _ = parse_strict(&bytes);
        }
    }
}
