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
//!
//! This pass owns which blocks the statement parser structures. Its stack is bounded by
//! [`NESTING_DEPTH_LIMIT`] rather than by the source, so a `{` nested past the limit is
//! left unmeasured, and the parser refuses exactly the blocks that carry no measurement
//! instead of counting depth a second time. A block whose statement count is unknown is
//! therefore never built, and no second counter exists to disagree with this one about
//! which blocks the tree holds.

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
        // Open `{`s this pass left unmeasured. Measuring stops at the limit, so every
        // unmeasured `{` sits inside the innermost measured block and a plain count
        // keeps the stack aligned: their `}` closes one of them and never pops a
        // measured frame. Popping for one would credit the rest of that measured
        // block's lines to its parent and leave the block itself to grow from nothing.
        let mut unmeasured = 0usize;
        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::LeftBrace => {
                    if unmeasured > 0 {
                        unmeasured += 1;
                        continue;
                    }
                    current(&mut body, &mut open_blocks).line_has_content = true;
                    // Past the limit the parser skips the block rather than structuring
                    // it, so measuring deeper would size lists that are never built —
                    // and would make this stack grow with the source rather than with a
                    // fixed bound. The whole skipped extent is one statement of the
                    // block that holds it: the line its `{` sits on, counted above, and
                    // nothing within.
                    match u32::try_from(index) {
                        Ok(open) if open_blocks.len() < NESTING_DEPTH_LIMIT => {
                            open_blocks.push(Frame {
                                open,
                                lines: 0,
                                line_has_content: false,
                            });
                        }
                        _ => unmeasured = 1,
                    }
                }
                TokenKind::RightBrace => {
                    if unmeasured > 0 {
                        unmeasured -= 1;
                    } else if let Some(frame) = open_blocks.pop() {
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
                _ => {
                    if unmeasured == 0 {
                        current(&mut body, &mut open_blocks).line_has_content = true;
                    }
                }
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

    /// The statement capacity of the block whose `{` is at `open`, or `None` when this
    /// pass left that block unmeasured because it nests past [`NESTING_DEPTH_LIMIT`].
    /// The parser builds exactly the blocks that answer `Some` here.
    pub(super) fn block(&self, open: usize) -> Option<usize> {
        let open = u32::try_from(open).ok()?;
        match self.blocks.binary_search_by_key(&open, |(at, _)| *at) {
            Ok(index) => Some(self.blocks[index].1 as usize),
            Err(_) => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex_source;

    /// A source's tokens, and the indices of its `{`s within the one function body it
    /// holds — the slice `DeclParser` hands the statement parser, which is what
    /// [`BlockLines::measure`] runs over.
    struct Body {
        tokens: Box<[Token]>,
        opens: Vec<usize>,
    }

    impl Body {
        fn of(source: &str) -> Self {
            let file = lex_source(source).tokens;
            let open = file
                .iter()
                .position(|token| token.kind == TokenKind::LeftBrace)
                .expect("the fixture opens a function body");
            let mut depth = 0usize;
            let mut close = file.len();
            for (index, token) in file.iter().enumerate().skip(open) {
                match token.kind {
                    TokenKind::LeftBrace => depth += 1,
                    TokenKind::RightBrace => {
                        depth -= 1;
                        if depth == 0 {
                            close = index;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let tokens: Box<[Token]> = file[open + 1..close].into();
            let opens = tokens
                .iter()
                .enumerate()
                .filter(|(_, token)| token.kind == TokenKind::LeftBrace)
                .map(|(index, _)| index)
                .collect();
            Self { tokens, opens }
        }

        fn measure(&self) -> BlockLines {
            BlockLines::measure(&self.tokens)
        }
    }

    /// One body nested to exactly the limit, holding one `{}` a level past it, then
    /// `statements` further statement lines in the innermost measured block.
    fn nested_to_the_limit(statements: usize) -> String {
        let mut source = String::from("module m\n\nfn f() {\n");
        for _ in 0..NESTING_DEPTH_LIMIT {
            source.push_str("if a {\n");
        }
        source.push_str("if a {}\n");
        for _ in 0..statements {
            source.push_str("a\n");
        }
        for _ in 0..NESTING_DEPTH_LIMIT {
            source.push_str("}\n");
        }
        source.push_str("}\n");
        source
    }

    /// A `{` past the limit opens no frame, so its `}` closes none either.
    ///
    /// It closed the innermost measured block's frame instead, which credited that
    /// block's remaining lines to its parent: the parent was then sized at a line count
    /// it never fills — a phantom held for the whole parse — and the block itself was
    /// sized at its first two lines and grew by doubling to hold the rest. That is the
    /// amortized growth this pass exists to remove, reintroduced by one unbalanced pop.
    #[test]
    fn a_block_past_the_limit_closes_no_measured_frame() {
        let statements = 64;
        let body = Body::of(&nested_to_the_limit(statements));
        let lines = body.measure();

        let innermost = body.opens[NESTING_DEPTH_LIMIT - 1];
        let parent = body.opens[NESTING_DEPTH_LIMIT - 2];
        let past_the_limit = body.opens[NESTING_DEPTH_LIMIT];

        assert_eq!(
            lines.block(innermost),
            Some(statements + 1),
            "the innermost measured block holds the over-limit `if` and every statement \
             line after it"
        );
        assert_eq!(
            lines.block(parent),
            Some(1),
            "its parent holds one statement — the `if` that opens the innermost block — \
             and must not be sized at its child's line count"
        );
        assert_eq!(
            lines.block(past_the_limit),
            None,
            "a block past the limit is not measured, so the parser does not build it"
        );
    }

    /// The measurement owns the limit, so it answers for every block the parser reaches:
    /// every brace inside the limit is sized and every brace past it is not.
    #[test]
    fn the_limit_decides_exactly_which_blocks_are_measured() {
        let body = Body::of(&nested_to_the_limit(4));
        let lines = body.measure();
        for (depth, open) in body.opens.iter().enumerate() {
            assert_eq!(
                lines.block(*open).is_some(),
                depth < NESTING_DEPTH_LIMIT,
                "the block at depth {depth} disagrees with the limit"
            );
        }
    }

    /// A `match` opens a brace of its own before its arms open theirs, and both count
    /// against the one limit. Counting only the arms would let a nested `match` reach
    /// twice the limit's brace depth, and the blocks past that point would be built from
    /// a measurement that never recorded them — sized at nothing, grown by doubling.
    #[test]
    fn a_match_body_counts_toward_the_limit_like_any_other_block() {
        let levels = NESTING_DEPTH_LIMIT / 2 + 1;
        let mut source = String::from("module m\n\nfn f() {\n");
        for _ in 0..levels {
            source.push_str("match a {\nb => {\n");
        }
        source.push_str("a\n");
        for _ in 0..levels {
            source.push_str("}\n}\n");
        }
        source.push_str("}\n");

        let body = Body::of(&source);
        let lines = body.measure();
        let measured = body
            .opens
            .iter()
            .filter(|open| lines.block(**open).is_some())
            .count();
        assert_eq!(
            measured, NESTING_DEPTH_LIMIT,
            "a `match` brace and an arm brace each take one level of the limit"
        );

        // What the parser builds agrees, because the measurement is the only thing it
        // asks. A second depth counter that skipped the `match` brace would structure
        // one more level than was measured, and size that level's block at nothing.
        let parsed = crate::parse_source(&source);
        let Some(crate::Declaration::Function(function)) = parsed.file.declarations.first() else {
            panic!("the fixture declares one function");
        };
        let mut block = &function.body;
        let mut structured = 0usize;
        while let Some(crate::Statement::Match { arms, .. }) = block.statements.first() {
            let Some(arm) = arms.first() else {
                break;
            };
            block = &arm.block;
            if !block.statements.is_empty() {
                structured += 1;
            }
        }
        assert!(
            block.statements.is_empty(),
            "the deepest arm is past the limit and fails closed"
        );
        assert_eq!(
            structured,
            NESTING_DEPTH_LIMIT / 2,
            "the parser structures exactly the `match` levels the limit measures"
        );
    }

    /// Every content line belongs to exactly one block, so the measurements sum to the
    /// body's own line count and never over-reserve by the nesting depth.
    #[test]
    fn the_measurements_sum_to_the_lines_they_came_from() {
        let statements = 32;
        let body = Body::of(&nested_to_the_limit(statements));
        let lines = body.measure();
        let total: usize = body
            .opens
            .iter()
            .filter_map(|open| lines.block(*open))
            .sum::<usize>()
            + lines.body();
        let content_lines = NESTING_DEPTH_LIMIT + 1 + statements;
        assert_eq!(
            total, content_lines,
            "the measured lines are the body's own content lines, once each"
        );
    }
}
