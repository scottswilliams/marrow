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

use lsp_types::{Position, Range};

/// Maps UTF-8 byte offsets in one source string to LSP UTF-16 positions.
///
/// The line-start table is built in one pass over the source. Each lookup then binary
/// searches its line and counts UTF-16 units over that line alone, so projecting every
/// span of a document costs one scan of the source plus one scan of each addressed line.
pub(crate) struct LineMap<'a> {
    source: &'a str,
    /// The byte offset at which each line begins. The first entry is `0`; a trailing
    /// newline opens one final empty line.
    line_starts: Vec<usize>,
}

impl<'a> LineMap<'a> {
    /// Build a map over source bytes. The bytes must be valid UTF-8 (the snapshot's
    /// input files that parsed always are; a non-UTF-8 file is never queried for a
    /// span-bearing fact).
    pub(crate) fn new(source: &'a str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(index, _)| index + 1))
            .collect();
        Self {
            source,
            line_starts,
        }
    }

    /// The LSP position of a UTF-8 byte offset. An offset past the end clamps to the
    /// end-of-source position; an offset that falls inside a multi-byte character
    /// snaps to that character's start.
    pub(crate) fn position_at(&self, byte_offset: usize) -> Position {
        let mut clamped = byte_offset.min(self.source.len());
        while !self.source.is_char_boundary(clamped) {
            clamped -= 1;
        }
        let line = self.line_starts.partition_point(|&start| start <= clamped) - 1;
        let character = self.source[self.line_starts[line]..clamped]
            .encode_utf16()
            .count();
        Position::new(line as u32, character as u32)
    }

    /// The LSP range spanning a half-open byte range.
    pub(crate) fn range_of(&self, start_byte: usize, end_byte: usize) -> Range {
        Range::new(self.position_at(start_byte), self.position_at(end_byte))
    }

    /// The UTF-8 byte offset of an LSP position. A line past the end clamps to the end
    /// of source; a character past the line end clamps to the line end (the LSP
    /// convention). Total and defensive: a stale client position never panics.
    pub(crate) fn byte_at(&self, position: Position) -> usize {
        let line = position.line as usize;
        let Some(&line_start) = self.line_starts.get(line) else {
            return self.source.len();
        };
        let line_end = self
            .line_starts
            .get(line + 1)
            .map_or(self.source.len(), |next| next - 1);
        let mut units = 0u32;
        for (index, ch) in self.source[line_start..line_end].char_indices() {
            if units >= position.character {
                return line_start + index;
            }
            units += ch.len_utf16() as u32;
        }
        line_end
    }

    /// The end-of-source position — the range end of a whole-document edit.
    pub(crate) fn end_position(&self) -> Position {
        self.position_at(self.source.len())
    }
}

/// The byte offset immediately after `needle`'s first occurrence in `source`: how a test
/// addresses a position without counting bytes by hand.
#[cfg(test)]
pub(crate) fn after(source: &str, needle: &str) -> usize {
    source.find(needle).expect("needle present") + needle.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_of_source_is_origin() {
        let map = LineMap::new("hello");
        assert_eq!(map.position_at(0), Position::new(0, 0));
    }

    #[test]
    fn counts_lines_and_ascii_columns() {
        let map = LineMap::new("ab\ncd\nef");
        assert_eq!(map.position_at(4), Position::new(1, 1));
        assert_eq!(map.position_at(7), Position::new(2, 1));
    }

    #[test]
    fn astral_character_is_two_utf16_units() {
        // "a😀b": 'a' (1 byte), '😀' U+1F600 (4 bytes, 2 UTF-16 units), 'b'.
        let map = LineMap::new("a😀b");
        // Offset at 'b' (byte 5): character = 1 (a) + 2 (astral) = 3.
        assert_eq!(map.position_at(5), Position::new(0, 3));
    }

    #[test]
    fn bmp_multibyte_is_one_utf16_unit() {
        // "é" is U+00E9 (2 UTF-8 bytes, 1 UTF-16 unit).
        let map = LineMap::new("é!");
        assert_eq!(map.position_at(2), Position::new(0, 1));
    }

    #[test]
    fn offset_past_end_clamps() {
        let map = LineMap::new("ab\ncd");
        assert_eq!(map.position_at(999), Position::new(1, 2));
    }

    #[test]
    fn offset_inside_multibyte_snaps_to_start() {
        // Offset 1 is inside the 4-byte astral char at byte 0.
        let map = LineMap::new("😀x");
        assert_eq!(map.position_at(1), Position::new(0, 0));
    }

    #[test]
    fn byte_at_round_trips_position_at() {
        let source = "let x = 1\nlet 😀 = 2\nend";
        let map = LineMap::new(source);
        for offset in [0usize, 4, 9, 10, 14, source.len()] {
            let position = map.position_at(offset);
            // Round-trip lands on a character boundary at or before the original offset.
            let back = map.byte_at(position);
            assert!(back <= offset, "byte_at({position:?})={back} > {offset}");
            assert_eq!(map.position_at(back), position);
        }
    }

    #[test]
    fn byte_at_clamps_line_and_character() {
        let map = LineMap::new("ab\ncd");
        assert_eq!(map.byte_at(Position::new(9, 0)), 5);
        assert_eq!(map.byte_at(Position::new(0, 99)), 2);
    }

    #[test]
    fn end_position_is_source_end() {
        let map = LineMap::new("ab\ncde");
        assert_eq!(map.end_position(), Position::new(1, 3));
    }

    #[test]
    fn trailing_newline_opens_an_empty_last_line() {
        let map = LineMap::new("ab\n");
        assert_eq!(map.end_position(), Position::new(1, 0));
        assert_eq!(map.byte_at(Position::new(1, 0)), 3);
        assert_eq!(map.byte_at(Position::new(0, 5)), 2);
    }

    #[test]
    fn range_of_spans_start_and_end() {
        let map = LineMap::new("abc\ndef");
        let range = map.range_of(1, 6);
        assert_eq!(range.start, Position::new(0, 1));
        assert_eq!(range.end, Position::new(1, 2));
    }
}
