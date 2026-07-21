//! Byte-offset ⇄ LSP UTF-16 position mapping.
//!
//! The Language Server Protocol addresses source by zero-based line and UTF-16 code
//! unit within the line. The compiler's spans are UTF-8 byte offsets. This module is
//! the one owner of the projection between them, over the exact source bytes the
//! snapshot analyzed. Astral characters (outside the Basic Multilingual Plane) occupy
//! two UTF-16 code units, so the mapping is not a byte count.
//!
//! The mapping is total and defensive: an offset past the source end clamps to the end
//! position, so a stale or out-of-range span can never panic the server.

/// A zero-based LSP position: a line and a UTF-16 code-unit offset within that line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based UTF-16 code-unit offset within the line.
    pub character: u32,
}

/// A zero-based half-open LSP range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Range {
    /// Inclusive start.
    pub start: Position,
    /// Exclusive end.
    pub end: Position,
}

/// Maps UTF-8 byte offsets in one source string to LSP UTF-16 positions.
///
/// Built once per source; `position_at` is a linear scan bounded by the byte offset.
/// For the small spans the analysis facts carry, this is well within the instant-
/// response requirement; a source is re-scanned only per query, never retained beyond
/// the snapshot it maps.
pub struct LineMap<'a> {
    source: &'a str,
}

impl<'a> LineMap<'a> {
    /// Build a map over source bytes. The bytes must be valid UTF-8 (the snapshot's
    /// input files that parsed always are; a non-UTF-8 file is never queried for a
    /// span-bearing fact).
    pub fn new(source: &'a str) -> Self {
        Self { source }
    }

    /// The LSP position of a UTF-8 byte offset. An offset past the end clamps to the
    /// end-of-source position; an offset that falls inside a multi-byte character
    /// snaps to that character's start.
    pub fn position_at(&self, byte_offset: usize) -> Position {
        let clamped = byte_offset.min(self.source.len());
        let mut line = 0u32;
        let mut line_start_byte = 0usize;
        // Advance line count up to the byte offset.
        for (index, byte) in self.source.as_bytes()[..clamped].iter().enumerate() {
            if *byte == b'\n' {
                line += 1;
                line_start_byte = index + 1;
            }
        }
        // UTF-16 code units from the line start to the clamped offset, snapping to a
        // character boundary at or before the offset.
        let line_text = &self.source[line_start_byte..];
        let mut character = 0u32;
        for (index, ch) in line_text.char_indices() {
            // Count a character only when it lies wholly before the offset. A character
            // that starts at or straddles the offset ends the count, so an offset that
            // falls inside a multi-byte character snaps to that character's start.
            if line_start_byte + index + ch.len_utf8() > clamped {
                break;
            }
            character += ch.len_utf16() as u32;
        }
        Position { line, character }
    }

    /// The LSP range spanning a half-open byte range.
    pub fn range_of(&self, start_byte: usize, end_byte: usize) -> Range {
        Range {
            start: self.position_at(start_byte),
            end: self.position_at(end_byte),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_of_source_is_origin() {
        let map = LineMap::new("hello");
        assert_eq!(map.position_at(0), Position { line: 0, character: 0 });
    }

    #[test]
    fn counts_lines_and_ascii_columns() {
        let map = LineMap::new("ab\ncd\nef");
        assert_eq!(map.position_at(4), Position { line: 1, character: 1 });
        assert_eq!(map.position_at(7), Position { line: 2, character: 1 });
    }

    #[test]
    fn astral_character_is_two_utf16_units() {
        // "a😀b": 'a' (1 byte), '😀' U+1F600 (4 bytes, 2 UTF-16 units), 'b'.
        let source = "a😀b";
        let map = LineMap::new(source);
        // Offset at 'b' (byte 5): character = 1 (a) + 2 (astral) = 3.
        assert_eq!(map.position_at(5), Position { line: 0, character: 3 });
    }

    #[test]
    fn bmp_multibyte_is_one_utf16_unit() {
        // "é" is U+00E9 (2 UTF-8 bytes, 1 UTF-16 unit).
        let source = "é!";
        let map = LineMap::new(source);
        assert_eq!(map.position_at(2), Position { line: 0, character: 1 });
    }

    #[test]
    fn offset_past_end_clamps() {
        let map = LineMap::new("ab\ncd");
        assert_eq!(map.position_at(999), Position { line: 1, character: 2 });
    }

    #[test]
    fn offset_inside_multibyte_snaps_to_start() {
        // Offset 1 is inside the 4-byte astral char at byte 0.
        let map = LineMap::new("😀x");
        assert_eq!(map.position_at(1), Position { line: 0, character: 0 });
    }

    #[test]
    fn range_of_spans_start_and_end() {
        let map = LineMap::new("abc\ndef");
        let range = map.range_of(1, 6);
        assert_eq!(range.start, Position { line: 0, character: 1 });
        assert_eq!(range.end, Position { line: 1, character: 2 });
    }
}
