//! How many statements each `{ … }` block of a function body can hold, measured
//! before the body is parsed so every statement list is allocated once at its final
//! size.
//!
//! A block holds at most one statement per *content line* it opens directly — a line
//! carrying at least one significant token at the block's own brace depth. Counting
//! only a block's own lines is what makes the total sound: a nested line belongs to
//! exactly one block, so summed over every block in a body the count is the body's
//! line count, where counting each block's whole extent would count a nested line once
//! per enclosing block and over-reserve by the nesting depth.
//!
//! One pass with a brace stack measures every block in a body, so the body costs a
//! single walk of its tokens rather than one walk per block.

use crate::NESTING_DEPTH_LIMIT;
use crate::token::{Token, TokenKind};

/// The measured statement capacity of a body and of each block inside it.
pub(super) struct BlockLines {
    /// Content lines directly in the body, outside every nested block.
    body: usize,
    /// `(index of the `{`, content lines directly in that block)`, by token index.
    blocks: Box<[(u32, u32)]>,
}

/// One open block while measuring: where its `{` sits, the content lines counted so
/// far, and whether the line in progress has content yet.
struct Frame {
    open: u32,
    lines: u32,
    line_has_content: bool,
}

impl BlockLines {
    pub(super) fn measure(tokens: &[Token]) -> Self {
        let mut body = Frame {
            open: 0,
            lines: 0,
            line_has_content: false,
        };
        let mut open_blocks: Vec<Frame> = Vec::with_capacity(NESTING_DEPTH_LIMIT);
        let mut blocks: Vec<(u32, u32)> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::LeftBrace => {
                    current(&mut body, &mut open_blocks).line_has_content = true;
                    // The statement parser refuses to descend past the nesting limit and
                    // yields an empty block there, so measuring deeper would size lists
                    // that are never built — and would make this stack grow with the
                    // source rather than with a fixed bound.
                    if open_blocks.len() < NESTING_DEPTH_LIMIT {
                        open_blocks.push(Frame {
                            open: index as u32,
                            lines: 0,
                            line_has_content: false,
                        });
                    }
                }
                TokenKind::RightBrace => {
                    if let Some(frame) = open_blocks.pop() {
                        blocks.push((frame.open, frame.capacity()));
                    }
                }
                TokenKind::Newline => {
                    let frame = current(&mut body, &mut open_blocks);
                    if frame.line_has_content {
                        frame.lines += 1;
                        frame.line_has_content = false;
                    }
                }
                TokenKind::Eof => {}
                _ => current(&mut body, &mut open_blocks).line_has_content = true,
            }
        }
        // A block left open at the end of the slice still gets its measurement: an
        // unclosed `{` holding a body's worth of statements would otherwise be the one
        // shape whose statement list is allocated by growing.
        while let Some(frame) = open_blocks.pop() {
            blocks.push((frame.open, frame.capacity()));
        }
        blocks.sort_unstable_by_key(|(open, _)| *open);
        Self {
            body: body.capacity() as usize,
            blocks: blocks.into_boxed_slice(),
        }
    }

    /// The statement capacity of the body itself.
    pub(super) fn body(&self) -> usize {
        self.body
    }

    /// The statement capacity of the block whose `{` is at `open`.
    pub(super) fn block(&self, open: usize) -> usize {
        let open = open as u32;
        match self.blocks.binary_search_by_key(&open, |(at, _)| *at) {
            Ok(index) => self.blocks[index].1 as usize,
            Err(_) => 0,
        }
    }
}

impl Frame {
    /// The content lines this frame opened, counting a final line that ran to the
    /// block's `}` without a newline of its own.
    fn capacity(&self) -> u32 {
        self.lines + u32::from(self.line_has_content)
    }
}

fn current<'f>(body: &'f mut Frame, open_blocks: &'f mut [Frame]) -> &'f mut Frame {
    open_blocks.last_mut().unwrap_or(body)
}
