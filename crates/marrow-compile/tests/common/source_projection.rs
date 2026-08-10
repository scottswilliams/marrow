//! One projection of Rust source to the code a production absence gate is about: every
//! comment, string, char literal, and `#[cfg(test)]` item blanked to spaces, with byte
//! offsets and line breaks preserved so a match's line number is the source's own.
//!
//! A text gate that matches raw lines silently disables itself the moment the shape it
//! forbids appears inside a comment, a string, or a test module: the scan then reports a
//! hit it must ignore, someone narrows the needle, and the gate stops seeing real code.
//! Every suite that scans source shares this one projection, and `absence_gates` carries
//! its plant probes — a second, weaker hand-rolled scanner would be a second answer to
//! what counts as code.

#![allow(dead_code)]

use std::path::Path;

/// `source` with every comment and string/char literal replaced by spaces. Byte offsets
/// and newlines are preserved, so a match's line number is the source's own.
pub fn without_literals(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    let mut index = 0;
    // Copy `count` bytes through unchanged.
    macro_rules! keep {
        ($count:expr) => {{
            let count = $count;
            out[index..index + count].copy_from_slice(&bytes[index..index + count]);
            index += count;
        }};
    }
    // Blank `count` bytes, keeping their newlines so line numbers survive.
    macro_rules! blank {
        ($count:expr) => {{
            for offset in index..(index + $count).min(bytes.len()) {
                if bytes[offset] == b'\n' {
                    out[offset] = b'\n';
                }
            }
            index = (index + $count).min(bytes.len());
        }};
    }
    while index < bytes.len() {
        let rest = &bytes[index..];
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(rest.len());
            blank!(end);
        } else if rest.starts_with(b"/*") {
            let mut depth = 0usize;
            let mut cursor = 0usize;
            while cursor < rest.len() {
                if rest[cursor..].starts_with(b"/*") {
                    depth += 1;
                    cursor += 2;
                } else if rest[cursor..].starts_with(b"*/") {
                    depth -= 1;
                    cursor += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    cursor += 1;
                }
            }
            blank!(cursor);
        } else if let Some((prefix, hashes)) = raw_string_open(bytes, index) {
            // The opener's width: the `b`/`c` marker, the `r`, the hashes, the quote.
            let open = prefix + 1 + hashes + 1;
            let mut closer = Vec::with_capacity(hashes + 1);
            closer.push(b'"');
            closer.extend(std::iter::repeat_n(b'#', hashes));
            let body = &rest[open..];
            let end = body
                .windows(closer.len())
                .position(|window| window == closer.as_slice())
                .map_or(rest.len(), |offset| open + offset + closer.len());
            blank!(end);
        } else if rest[0] == b'"' {
            let mut cursor = 1usize;
            while cursor < rest.len() {
                match rest[cursor] {
                    b'\\' => cursor += 2,
                    b'"' => {
                        cursor += 1;
                        break;
                    }
                    _ => cursor += 1,
                }
            }
            blank!(cursor);
        } else if rest[0] == b'\'' && char_literal_len(rest).is_some() {
            let len = char_literal_len(rest).unwrap_or(1);
            blank!(len);
        } else {
            keep!(1);
        }
    }
    String::from_utf8(out).expect("blanking preserves UTF-8 boundaries")
}

pub fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The `(prefix, hashes)` of a raw string literal opening at `index`, where `prefix`
/// counts the optional `b`/`c` byte- or C-string marker before the `r`.
///
/// A raw string honours no escapes. A scanner that fails to recognise one of its
/// spellings falls through to the ordinary string branch, reads the literal's backslash
/// as an escape, walks past the closing quote, and blanks the rest of the file — every
/// gate downstream then passes because its shape was erased rather than absent. So every
/// spelling is recognised here, and the probe test below plants one of each.
pub fn raw_string_open(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let rest = &bytes[index..];
    let prefix = usize::from(matches!(rest.first(), Some(b'b' | b'c')));
    if rest.get(prefix) != Some(&b'r') {
        return None;
    }
    // The whole prefix must open a token: `abr"x"` is an identifier, not a literal.
    if index
        .checked_sub(1)
        .is_some_and(|previous| is_ident_byte(bytes[previous]))
    {
        return None;
    }
    let hashes = rest[prefix + 1..].iter().position(|byte| *byte != b'#')?;
    (rest.get(prefix + 1 + hashes) == Some(&b'"')).then_some((prefix, hashes))
}

/// The length of a char literal starting at `rest`, or `None` when the quote opens a
/// lifetime (`'a`) instead — which is ordinary code and must not be blanked.
pub fn char_literal_len(rest: &[u8]) -> Option<usize> {
    if rest.get(1) == Some(&b'\\') {
        // The byte after the backslash belongs to the escape — in `'\''` it is a quote —
        // so the closing quote is the first one beyond it. Searching from the backslash
        // instead measures `'\''` as three bytes and leaves its fourth, a bare quote, in
        // the projection, where it pairs with the next quote and blanks the code between.
        let end = rest.iter().skip(3).position(|byte| *byte == b'\'')? + 4;
        return Some(end);
    }
    // A one-character literal closes immediately; anything else after the quote is a
    // lifetime name. The character may be multi-byte, so its width is read from the
    // leading byte rather than assumed to be one.
    let width = match rest.get(1)? {
        byte if *byte < 0x80 => 1,
        byte if byte >> 5 == 0b110 => 2,
        byte if byte >> 4 == 0b1110 => 3,
        _ => 4,
    };
    (rest.get(1 + width) == Some(&b'\'')).then_some(2 + width)
}

/// `source` with every `#[cfg(test)]` item blanked as well: what the crate compiles when
/// it is built as a dependency, which is the only code a production absence gate is about.
pub fn production_code(source: &str) -> String {
    without_cfg_test_items(&without_literals(source))
}

/// `code` with every `#[cfg(test)]` item — block or single-statement — blanked to spaces,
/// byte offsets and line breaks preserved.
///
/// Applied to the literal-blanked projection it completes it, and applied to raw source it
/// answers, independently of that projection, which lines are production code. A gate that
/// asks whether the projection kept a line must take the line from somewhere the projection
/// did not choose, or it proves only that the projection agrees with itself.
pub fn without_cfg_test_items(code: &str) -> String {
    let mut code = code.to_string();
    const MARKER: &str = "#[cfg(test)]";
    while let Some(start) = code.find(MARKER) {
        let bytes = code.as_bytes();
        let mut cursor = start + MARKER.len();
        while cursor < bytes.len() && bytes[cursor] != b'{' && bytes[cursor] != b';' {
            cursor += 1;
        }
        let end = if bytes.get(cursor) == Some(&b'{') {
            let mut depth = 0usize;
            let mut scan = cursor;
            while scan < bytes.len() {
                match bytes[scan] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            scan += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                scan += 1;
            }
            scan
        } else {
            (cursor + 1).min(bytes.len())
        };
        // A char is replaced by as many spaces as it occupied bytes. Applied to the
        // literal-blanked projection the region is already ASCII either way, but applied
        // to raw source a multi-byte char in a test comment would otherwise shorten the
        // text and shift every offset after it off the source's own.
        let blanked: String = code[start..end]
            .chars()
            .map(|c| {
                if c == '\n' {
                    "\n".to_string()
                } else {
                    " ".repeat(c.len_utf8())
                }
            })
            .collect();
        code.replace_range(start..end, &blanked);
    }
    code
}

/// Files whose whole contents are test code: they are included behind `#[cfg(test)]` at
/// their module declaration, so their own text carries no production shape.
pub fn is_test_only_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_tests.rs"))
}

/// The last item header in `source`'s production code: a column-0 line opening a block,
/// outside every `#[cfg(test)]` item. Lines carrying a string, a comment, or a char
/// literal are excluded so the sentinel is text the projection must reproduce byte for
/// byte; a lifetime tick is such text, and excluding it too would leave a file whose only
/// column-0 header is an `impl<'a>` with no sentinel at all.
///
/// The test region is removed by blanking rather than by truncating at the first
/// `#[cfg(test)]`: that marker also spells a test-only `use`, after which thousands of
/// lines of production code follow, and truncating there leaves the whole tail unexamined.
pub fn last_production_item(source: &str) -> Option<String> {
    without_cfg_test_items(source)
        .lines()
        .filter(|line| {
            line.starts_with(|first: char| first.is_ascii_alphabetic())
                && line.ends_with('{')
                && !line.contains('"')
                && !line.contains("//")
                && !contains_char_literal(line)
        })
        .next_back()
        .map(str::to_string)
}

/// Whether any quote in `line` opens a char literal rather than a lifetime.
fn contains_char_literal(line: &str) -> bool {
    let bytes = line.as_bytes();
    (0..bytes.len())
        .filter(|index| bytes[*index] == b'\'')
        .any(|index| char_literal_len(&bytes[index..]).is_some())
}
