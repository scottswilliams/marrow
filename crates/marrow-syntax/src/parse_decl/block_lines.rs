//! How many statements each `{ … }` block of a function body can hold, measured
//! before the body is parsed so every statement list is allocated once at its final
//! size.
//!
//! A block holds at most one statement per *statement start* it opens directly: a
//! significant token at the block's own brace depth that follows a boundary — the
//! block's own `{`, a `NEWLINE`, or the `}` closing a nested block. A newline is not
//! the only boundary because a compound statement's body closes on a `}`, which leaves
//! the cursor mid-line and the parser free to structure another statement from the same
//! line (`if a {} if b {}`); counting lines would size such a block at one and leave the
//! rest of it to be grown by doubling, which is what this pass exists to prevent.
//!
//! Counting only a block's own starts is what makes the total sound: a nested start
//! belongs to exactly one block, so summed over every block in a body the count is the
//! body's own start count, where counting each block's whole extent would count a nested
//! start once per enclosing block and over-reserve by the nesting depth.
//!
//! A start costs at least two source bytes — its own token and the boundary separating
//! it from the previous one — which is the bound the per-source-byte parse charge is
//! derived from.
//!
//! The count is an upper bound, not an exact one: a clause continuing its statement past
//! a nested block (`} else {`) begins no new statement but does follow a boundary, so it
//! is counted. Over-reserving a slot cannot make the list grow, and the two-byte floor
//! holds for a counted start whether or not the parser spends it.
//!
//! One pass with a brace stack measures every block in a body, so the body costs a
//! single walk of its tokens rather than one walk per block.
//!
//! This pass owns which brace-delimited regions the statement parser structures. Its
//! stack is bounded by [`NESTING_DEPTH_LIMIT`] rather than by the source, so a `{` nested
//! past the limit is left unmeasured, and the parser structures exactly the regions that
//! carry a measurement — a block, and a `match` body. A region whose statement count is
//! unknown is therefore never built, and no second counter disagrees with this one about
//! which regions the tree holds.
//!
//! It does not own how deep the descent goes, and cannot: this pass is keyed on a `{`, so
//! it has nothing to say about a trailing clause that takes a single inline statement in
//! place of a block. Bounding the native stack is a separate question with a separate
//! owner, the frame counter in `stmt`.

use crate::NESTING_DEPTH_LIMIT;
use crate::token::{Token, TokenKind};

/// What one measurement holds regardless of the body's length: the open-block stack,
/// which the nesting limit bounds rather than the source, and the smallest non-zero
/// capacity its block vector takes. Both are constants, so they are charged once in
/// [`crate::MAX_PARSE_FIXED_BYTES`] rather than per source byte.
pub(crate) const FIXED_BYTES: usize =
    NESTING_DEPTH_LIMIT * size_of::<Frame>() + MIN_BLOCK_CAPACITY * size_of::<(u32, u32)>();

/// The standard library's minimum non-zero capacity for an element of this width.
const MIN_BLOCK_CAPACITY: usize = 4;

/// The measured statement capacity of a body and of each block inside it.
pub(super) struct BlockLines {
    /// Statement starts directly in the body, outside every nested block.
    body: usize,
    /// `(index of the `{`, statement starts directly in that block)`, by token index.
    blocks: Box<[(u32, u32)]>,
}

/// One open block while measuring: where its `{` sits, the starts counted so far, and
/// whether a statement is already in progress.
struct Frame {
    open: u32,
    statements: u32,
    /// Set by the first significant token after a boundary and cleared by the next
    /// boundary, so the tokens between them are counted as one statement rather than
    /// one each.
    in_statement: bool,
}

impl BlockLines {
    pub(super) fn measure(tokens: &[Token]) -> Self {
        let mut body = Frame::new(0);
        let mut open_blocks: Vec<Frame> = Vec::with_capacity(NESTING_DEPTH_LIMIT);
        let mut blocks: Vec<(u32, u32)> = Vec::new();
        // Open `{`s this pass left unmeasured. Measuring stops at the limit, so every
        // unmeasured `{` sits inside the innermost measured block and a plain count
        // keeps the stack aligned: their `}` closes one of them and never pops a
        // measured frame. Popping for one would credit the rest of that measured
        // block's starts to its parent and leave the block itself to grow from nothing.
        let mut unmeasured = 0usize;
        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::LeftBrace => {
                    if unmeasured > 0 {
                        unmeasured += 1;
                        continue;
                    }
                    current(&mut body, &mut open_blocks).begin_statement();
                    // Past the limit the parser skips the block rather than structuring
                    // it, so measuring deeper would size lists that are never built —
                    // and would make this stack grow with the source rather than with a
                    // fixed bound. The whole skipped extent is one statement of the
                    // block that holds it: the one begun above, and nothing within.
                    match u32::try_from(index) {
                        Ok(open) if open_blocks.len() < NESTING_DEPTH_LIMIT => {
                            open_blocks.push(Frame::new(open));
                        }
                        _ => unmeasured = 1,
                    }
                }
                TokenKind::RightBrace => {
                    if unmeasured > 0 {
                        unmeasured -= 1;
                        if unmeasured > 0 {
                            continue;
                        }
                    } else if let Some(frame) = open_blocks.pop() {
                        blocks.push((frame.open, frame.statements));
                    } else {
                        continue;
                    }
                    // A closed nested block ends the statement that held it, so the next
                    // significant token on the same line begins another one.
                    current(&mut body, &mut open_blocks).in_statement = false;
                }
                TokenKind::Newline => {
                    current(&mut body, &mut open_blocks).in_statement = false;
                }
                TokenKind::Eof => {}
                _ => {
                    if unmeasured == 0 {
                        current(&mut body, &mut open_blocks).begin_statement();
                    }
                }
            }
        }
        // A block left open at the end of the slice still gets its measurement: an
        // unclosed `{` holding a body's worth of statements would otherwise be the one
        // shape whose statement list is allocated by growing.
        while let Some(frame) = open_blocks.pop() {
            blocks.push((frame.open, frame.statements));
        }
        blocks.sort_unstable_by_key(|(open, _)| *open);
        Self {
            body: body.statements as usize,
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
    fn new(open: u32) -> Self {
        Self {
            open,
            statements: 0,
            in_statement: false,
        }
    }

    /// Count a statement start, unless one is already in progress in this block.
    fn begin_statement(&mut self) {
        if !self.in_statement {
            self.statements += 1;
            self.in_statement = true;
        }
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

        // What the parser builds agrees, because the measurement is what it asks about
        // its own brace as well as about its arms' braces. A `match` that structured its
        // body without asking would build one more level than was measured, and grow an
        // arm list sized at nothing.
        let parsed = crate::parse_source(&source);
        let Some(crate::Declaration::Function(function)) = parsed.file.declarations.first() else {
            panic!("the fixture declares one function");
        };
        let mut block = &function.body;
        let mut structured = 0usize;
        let arms_of_the_deepest_match = loop {
            let Some(crate::Statement::Match { arms, .. }) = block.statements.first() else {
                panic!("every level of the fixture is a `match`");
            };
            let Some(arm) = arms.first() else {
                break arms.len();
            };
            structured += 1;
            block = &arm.block;
        };
        assert_eq!(
            arms_of_the_deepest_match, 0,
            "the deepest `match` sits at the limit, so its own body is past it and fails \
             closed with no arms rather than with arms whose blocks are skipped"
        );
        assert_eq!(
            structured,
            NESTING_DEPTH_LIMIT / 2,
            "the parser structures exactly the `match` levels the limit measures"
        );
    }

    /// A compound statement's body closes on a `}`, which leaves the cursor mid-line and
    /// the parser's loop free to structure another statement from the same line. A block
    /// therefore holds as many statements as it has *starts*, not as many as it has
    /// lines, and sizing it by lines is what let the one list this pass exists to size
    /// exactly be grown by doubling instead.
    #[test]
    fn statements_that_share_a_line_are_each_measured() {
        let units = 64;
        let source = format!("module m\n\nfn f() {{\n{}\n}}\n", "if a {} ".repeat(units));
        let body = Body::of(&source);
        let measured = body.measure().body();

        let parsed = crate::parse_source(&source);
        let Some(crate::Declaration::Function(function)) = parsed.file.declarations.first() else {
            panic!("the fixture declares one function");
        };
        let structured = function.body.statements.len();

        assert_eq!(
            structured, units,
            "the parser structures one statement per `if a {{}}`, all on one line"
        );
        assert!(
            measured >= structured,
            "the body was measured at {measured} statements and the parser built \
             {structured} of them, so the list it was handed grew by doubling"
        );
    }

    /// The same defect at declaration level: the file's declaration list is sized by this
    /// pass too, and a declaration body also closes on a `}` mid-line.
    #[test]
    fn declarations_that_share_a_line_are_each_measured() {
        let units = 64;
        let source = format!("module m\n\n{}\n", "fn f(){} ".repeat(units));
        let tokens = crate::lex_source(&source).tokens;
        let measured = BlockLines::measure(&tokens).body();
        let structured = crate::parse_source(&source).file.declarations.len();

        assert_eq!(
            structured, units,
            "the parser structures one declaration per `fn f(){{}}`, all on one line \
             (the `module` header is its own field, not a declaration)"
        );
        assert!(
            measured >= structured,
            "the file was measured at {measured} declarations and the parser built \
             {structured} of them, so the list it was handed grew by doubling"
        );
    }

    /// Every counted start belongs to exactly one block, so the measurements sum to the
    /// body's own start count and never over-reserve by the nesting depth.
    #[test]
    fn the_measurements_sum_to_the_starts_they_came_from() {
        let statements = 32;
        let body = Body::of(&nested_to_the_limit(statements));
        let lines = body.measure();
        let total: usize = body
            .opens
            .iter()
            .filter_map(|open| lines.block(*open))
            .sum::<usize>()
            + lines.body();
        // One `if a {` per level, the over-limit `if a {}` inside the innermost, and the
        // trailing statement lines. Each opens exactly one statement, in one block.
        let starts = NESTING_DEPTH_LIMIT + 1 + statements;
        assert_eq!(
            total, starts,
            "the measured starts are the body's own statement starts, once each"
        );
    }

    /// A start costs at least two source bytes — its own token and the boundary before
    /// it — which is the floor the per-source-byte parse charge is derived from. Both
    /// boundary kinds are exercised: the newline, and the `}` of a nested block.
    #[test]
    fn a_measured_start_costs_at_least_two_source_bytes() {
        for unit in ["a\n", "{}", "if{}", "a\nb\n"] {
            let filler = unit.repeat(512);
            let source = format!("module m\n\nfn f() {{\n{filler}\n}}\n");
            let body = Body::of(&source);
            let lines = body.measure();
            let total: usize = body
                .opens
                .iter()
                .filter_map(|open| lines.block(*open))
                .sum::<usize>()
                + lines.body();
            assert!(
                total * 2 <= source.len(),
                "{unit:?} measured {total} starts from {} source bytes, under the two \
                 bytes per start the parse charge is derived from",
                source.len()
            );
        }
    }
}
