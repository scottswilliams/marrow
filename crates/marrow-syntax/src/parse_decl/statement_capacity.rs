//! How many statements each brace-delimited *region* of a token slice can hold — a
//! `{ … }` block, a `match` body, and the slice itself — measured in one pass before
//! parsing so statement lists are reserved exactly rather than grown by doubling.
//!
//! A region holds at most one statement per *statement start* it opens directly: a
//! significant token at the region's own brace depth following a boundary — the region's
//! own `{`, a `NEWLINE`, or any `}`. A `}` is a boundary because a compound statement's
//! body closes mid-line and the parser may structure another statement from the same line
//! (`if a {} if b {}`); counting lines would size such a region at one.
//!
//! Counting only a region's *own* starts is what makes the total sound: a nested start
//! belongs to exactly one region, so the per-region counts sum to the slice's own start
//! count instead of over-reserving by the nesting depth. The count is an upper bound — a
//! continuing clause (`} else {`) follows a boundary without starting a statement — and
//! every counted start costs at least two source bytes (its token plus its boundary),
//! which is the floor the per-source-byte parse charge is derived from.
//!
//! This pass also owns which regions the statement parser structures: its stack is
//! bounded by [`NESTING_DEPTH_LIMIT`] rather than by the source, a `{` past the limit is
//! left unmeasured, and the parser builds exactly the regions that carry a measurement.
//! It does not own how deep the descent goes — it is keyed on `{`, so it says nothing
//! about a clause taking a single inline statement; the frame counter in `stmt` bounds
//! the native stack. [`outer_count`] answers the declaration parser's question, the outer
//! count alone, without allocating or measuring nested regions.

use crate::NESTING_DEPTH_LIMIT;
use crate::token::{Token, TokenKind};

/// What one measurement holds regardless of the body's length: the open-region stack,
/// which the nesting limit bounds rather than the source, and the smallest non-zero
/// capacity its region vector takes. Both are constants, so they are charged once in
/// [`crate::MAX_PARSE_FIXED_BYTES`] rather than per source byte.
pub(crate) const FIXED_BYTES: usize =
    NESTING_DEPTH_LIMIT * size_of::<Frame>() + MIN_REGION_CAPACITY * size_of::<(u32, u32)>();

/// The standard library's minimum non-zero capacity for an element of this width.
const MIN_REGION_CAPACITY: usize = 4;

/// The measured statement capacity of a token slice and of each region inside it.
pub(super) struct StatementCapacity {
    /// Statement starts directly in the slice, outside every nested region.
    body: usize,
    /// `(index of the `{`, statement starts directly in that region)`, by token index.
    regions: Box<[(u32, u32)]>,
}

/// One open region while measuring: where its `{` sits, the starts counted so far, and
/// whether a statement is already in progress.
struct Frame {
    open: u32,
    statements: u32,
    /// Set by the first significant token after a boundary and cleared by the next
    /// boundary, so the tokens between them are counted as one statement rather than
    /// one each.
    in_statement: bool,
}

/// Count outer starts without constructing the regions used by statement parsing.
/// Lexical depth continues past the measured limit so nested starts stay nested.
pub(super) fn outer_count(tokens: &[Token]) -> usize {
    let mut body = Frame::new(0);
    let mut depth = 0usize;
    for token in tokens {
        if token.kind == TokenKind::RightBrace {
            depth = depth.saturating_sub(1);
        }
        if depth == 0 {
            body.count_token(token.kind);
        }
        if token.kind == TokenKind::LeftBrace {
            depth += 1;
        }
    }
    body.statements as usize
}

impl StatementCapacity {
    pub(super) fn measure(tokens: &[Token]) -> Self {
        let mut body = Frame::new(0);
        let mut open_regions: Vec<Frame> = Vec::with_capacity(NESTING_DEPTH_LIMIT);
        let mut regions: Vec<(u32, u32)> = Vec::new();
        // Open `{`s this pass left unmeasured. Every one sits inside the innermost
        // measured region, so counting them keeps the stack aligned: their `}` closes one
        // of them and must never pop a measured frame, which would credit the rest of
        // that region's starts to its parent.
        let mut unmeasured = 0usize;
        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::LeftBrace => {
                    if unmeasured > 0 {
                        unmeasured += 1;
                        continue;
                    }
                    current(&mut body, &mut open_regions).count_token(token.kind);
                    // Past the limit the parser skips the block, so measuring deeper would
                    // size lists that are never built and would let this stack grow with
                    // the source. The whole skipped extent counts as the one statement
                    // begun above.
                    match u32::try_from(index) {
                        Ok(open) if open_regions.len() < NESTING_DEPTH_LIMIT => {
                            open_regions.push(Frame::new(open));
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
                    } else if let Some(frame) = open_regions.pop() {
                        regions.push((frame.open, frame.statements));
                    }
                    // A closed nested block ends the statement that held it, so the next
                    // significant token on the same line begins another one. A `}` that
                    // closes no frame is a boundary on the same terms, since the parser
                    // skips it and structures what follows on the same line.
                    current(&mut body, &mut open_regions).count_token(token.kind);
                }
                TokenKind::Newline => {
                    // An unmeasured region still leaves this boundary on the current
                    // measured frame, matching the body parser's regional sizing.
                    current(&mut body, &mut open_regions).count_token(token.kind);
                }
                _ => {
                    if unmeasured == 0 {
                        current(&mut body, &mut open_regions).count_token(token.kind);
                    }
                }
            }
        }
        // A region left open at the end of the slice still gets its measurement, so an
        // unclosed `{` is not the one shape whose statement list grows by doubling.
        while let Some(frame) = open_regions.pop() {
            regions.push((frame.open, frame.statements));
        }
        regions.sort_unstable_by_key(|(open, _)| *open);
        Self {
            body: body.statements as usize,
            regions: regions.into_boxed_slice(),
        }
    }

    /// The statement capacity of the slice itself.
    pub(super) fn body(&self) -> usize {
        self.body
    }

    /// The statement capacity of the region whose `{` is at `open`, or `None` when this
    /// pass left that region unmeasured because it nests past [`NESTING_DEPTH_LIMIT`].
    /// The parser builds exactly the regions that answer `Some` here.
    pub(super) fn region(&self, open: usize) -> Option<usize> {
        let open = u32::try_from(open).ok()?;
        match self.regions.binary_search_by_key(&open, |(at, _)| *at) {
            Ok(index) => Some(self.regions[index].1 as usize),
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

    /// Boundaries finish an item; the first significant token starts the next one.
    fn count_token(&mut self, kind: TokenKind) {
        match kind {
            TokenKind::RightBrace | TokenKind::Newline => self.in_statement = false,
            TokenKind::Eof => {}
            _ if !self.in_statement => {
                self.statements += 1;
                self.in_statement = true;
            }
            _ => {}
        }
    }
}

fn current<'f>(body: &'f mut Frame, open_regions: &'f mut [Frame]) -> &'f mut Frame {
    open_regions.last_mut().unwrap_or(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex_source;

    /// A nested block inside a declaration body parses to the same spans and statements
    /// as a flat one, and its region is never measured a second time.
    #[test]
    fn a_nested_region_parses_to_the_same_spans_and_statements() {
        use crate::{Declaration, Expression, LiteralKind, Statement};

        const SOURCE: &str = "module m\n\nfn first() {\n    if true {\n        return\n    }\n}\n\nfn second() {\n    return\n}\n";
        let parsed = crate::parse_source(SOURCE);

        assert!(
            parsed
                .diagnostics
                .as_complete()
                .unwrap()
                .as_slice()
                .is_empty()
        );
        let [Declaration::Function(first), Declaration::Function(second)] =
            parsed.file.declarations.as_ref()
        else {
            panic!("both nonempty function bodies are parsed");
        };
        assert_eq!((&*first.name, &*second.name), ("first", "second"));
        for (span, expected) in [
            (first.span, (10, 22, 3, 1)),
            (first.body.span, (21, 59, 3, 12)),
            (second.span, (61, 74, 9, 1)),
            (second.body.span, (73, 87, 9, 13)),
        ] {
            assert_eq!(
                (span.start_byte, span.end_byte, span.line, span.column),
                expected
            );
        }
        let [
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            },
        ] = first.body.statements.as_ref()
        else {
            panic!("the first function retains its nested block");
        };
        assert!(
            matches!(condition, Expression::Literal { kind: LiteralKind::Bool, text, .. }
            if text.as_ref() == "true")
        );
        assert!(else_ifs.is_empty() && else_block.is_none());
        assert!(matches!(
            then_block.statements.as_ref(),
            [Statement::Return { value: None, .. }]
        ));
        assert!(matches!(
            second.body.statements.as_ref(),
            [Statement::Return { value: None, .. }]
        ));
    }

    /// A source's tokens, and the indices of its `{`s, within the one function body it
    /// holds — the same slice `DeclParser` hands the statement parser.
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

        fn measure(&self) -> StatementCapacity {
            StatementCapacity::measure(&self.tokens)
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

    /// A `{` past the limit opens no frame, so its `}` closes none either. An unbalanced
    /// pop would credit the innermost measured block's remaining lines to its parent,
    /// leaving the parent oversized and the block itself to grow by doubling.
    #[test]
    fn a_block_past_the_limit_closes_no_measured_frame() {
        let statements = 64;
        let body = Body::of(&nested_to_the_limit(statements));
        let capacity = body.measure();

        let innermost = body.opens[NESTING_DEPTH_LIMIT - 1];
        let parent = body.opens[NESTING_DEPTH_LIMIT - 2];
        let past_the_limit = body.opens[NESTING_DEPTH_LIMIT];

        assert_eq!(
            capacity.region(innermost),
            Some(statements + 1),
            "the innermost measured block holds the over-limit `if` and every statement \
             line after it"
        );
        assert_eq!(
            capacity.region(parent),
            Some(1),
            "its parent holds one statement — the `if` that opens the innermost block — \
             and must not be sized at its child's line count"
        );
        assert_eq!(
            capacity.region(past_the_limit),
            None,
            "a block past the limit is not measured, so the parser does not build it"
        );
    }

    /// The measurement owns the limit, so it answers for every block the parser reaches:
    /// every brace inside the limit is sized and every brace past it is not.
    #[test]
    fn the_limit_decides_exactly_which_blocks_are_measured() {
        let body = Body::of(&nested_to_the_limit(4));
        let capacity = body.measure();
        for (depth, open) in body.opens.iter().enumerate() {
            assert_eq!(
                capacity.region(*open).is_some(),
                depth < NESTING_DEPTH_LIMIT,
                "the block at depth {depth} disagrees with the limit"
            );
        }
    }

    /// A `match` opens a brace of its own before its arms open theirs, and both count
    /// against the one limit. Counting only the arms would let a nested `match` reach
    /// twice the limit's brace depth and build blocks the measurement never recorded.
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
        let capacity = body.measure();
        let measured = body
            .opens
            .iter()
            .filter(|open| capacity.region(**open).is_some())
            .count();
        assert_eq!(
            measured, NESTING_DEPTH_LIMIT,
            "a `match` brace and an arm brace each take one level of the limit"
        );

        // What the parser builds must agree: a `match` that structured its body without
        // asking the measurement about its own brace would build one level too many.
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

    /// A compound statement's body closes mid-line, leaving the parser free to structure
    /// another statement from the same line. A block therefore holds as many statements
    /// as it has *starts*, not as many as it has lines.
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

    /// A declaration body can close on a `}` mid-line, so the outer count must begin
    /// the following declaration even without a newline.
    #[test]
    fn declarations_that_share_a_line_are_each_measured() {
        let units = 64;
        let source = format!("module m\n\n{}\n", "fn f(){} ".repeat(units));
        let tokens = crate::lex_source(&source).tokens;
        let measured = outer_count(&tokens);
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

    /// A `}` that closes nothing is still a boundary: the declaration parser skips it and
    /// structures whatever follows on the same line, so letting the statement in progress
    /// run past it would count one declaration for a whole file of them.
    #[test]
    fn declarations_after_an_unmatched_brace_are_each_measured() {
        let units = 64;
        let source = format!("module m\n{}\n", "const x = 1 }".repeat(units));
        let tokens = crate::lex_source(&source).tokens;
        let measured = outer_count(&tokens);
        let structured = crate::parse_source(&source).file.declarations.len();

        assert_eq!(
            structured, units,
            "the parser structures one declaration per `const x = 1`, each separated \
             from the next only by a `}}` that closes nothing"
        );
        assert!(
            measured >= structured,
            "the file was measured at {measured} declarations and the parser built \
             {structured} of them, so the list it was handed grew by doubling"
        );
    }

    #[test]
    fn outer_count_matches_regional_body() {
        let mut deep = nested_to_the_limit(4);
        deep.push_str("const tail = 1\n");
        for (case, source) in [
            (
                "closed",
                "module m\nfn f() { if true { return } }\nconst x = 1\n",
            ),
            (
                "members",
                "module m\nstruct Pair { left: int\nright: int }\nfn f(){}\n",
            ),
            ("same line", "module m\nfn f(){} fn g(){}\n"),
            ("unmatched close", "module m\n} const x = 1 } const y = 2"),
            ("unclosed", "module m\nfn f() {\nif true {\nreturn\n"),
            ("beyond limit", deep.as_str()),
        ] {
            let tokens = lex_source(source).tokens;
            assert_eq!(
                outer_count(&tokens),
                StatementCapacity::measure(&tokens).body(),
                "{case}"
            );
        }
    }

    /// Every counted start belongs to exactly one block, so the measurements sum to the
    /// body's own start count and never over-reserve by the nesting depth.
    #[test]
    fn the_measurements_sum_to_the_starts_they_came_from() {
        let statements = 32;
        let body = Body::of(&nested_to_the_limit(statements));
        let capacity = body.measure();
        let total: usize = body
            .opens
            .iter()
            .filter_map(|open| capacity.region(*open))
            .sum::<usize>()
            + capacity.body();
        // One `if a {` per level, the over-limit `if a {}` inside the innermost, and the
        // trailing statement lines — each one statement, in exactly one block.
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
            let capacity = body.measure();
            let total: usize = body
                .opens
                .iter()
                .filter_map(|open| capacity.region(*open))
                .sum::<usize>()
                + capacity.body();
            assert!(
                total * 2 <= source.len(),
                "{unit:?} measured {total} starts from {} source bytes, under the two \
                 bytes per start the parse charge is derived from",
                source.len()
            );
        }
    }
}
