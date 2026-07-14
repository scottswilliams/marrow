# marrow-syntax Contributor Notes

This leaf crate owns source text, AST construction, formatting, and parse
diagnostics. Its only dependency is `marrow-codes`; no edge points upward into
checking, runtime, catalog, or storage.

Parsing is total. A failure remains an error node with a diagnostic rather than
dropping syntax. Decode literal and type grammar once, return `Option` for
absent children, and prefer typed parser state over booleans.

Syntax owns source spelling only. It never defines stable schema path identity,
URI text, authority scope, graph-version evolution relations, or physical
encoding.

## Grammar-extension checklist

A later grammar feature lands as one vertical inside this crate. Each item below
moves together in the feature's lane; a feature is not done while any is missing:

1. **Grammar spelling** is documented in `docs/language/` (the authority), not in a
   plan or a comment. Read the relevant page before changing any `.mw` shape.
2. **Lexer/parser**: extend the token model and the recursive-descent parser.
   Parsing stays total — a failure is an error node plus a typed diagnostic, never
   dropped syntax. New nesting recurses under the shared `NESTING_DEPTH_LIMIT`
   guard; new decoders declare a bound before allocating.
3. **Recovery**: a new construct reports one diagnostic at its failure token with a
   typed `DiagnosticReason`; it does not resurrect a second cascading diagnostic
   (`total_parser_architecture` guards this). Spans stay in bounds and 1-based.
4. **Formatter**: render the construct canonically and idempotently. A body-bearing
   header joins its body through `append_body_block` (empty bodies render as the
   header alone); own-line comments render at their block's canonical indent; a
   statement's span covers only its own content, never a following sibling.
5. **Examples/tests**: add source-driven cases; documented `docs/language/` module
   blocks must parse, reconstruct, and format cleanly (the shared corpus tests).
6. **Fuzz oracle**: extend `tests/common/oracle.rs` (the reusable bounded oracle)
   and its `tests/cases/fuzz.rs` driver with the new construct — a deterministic
   corpus entry plus, for any minimized counterexample, a permanent regression
   fixture. The oracle asserts, over arbitrary bytes: no panic, deterministic
   parse, lossless token tiling, a bounded diagnostic count, and — for a
   comment-free clean parse — formatter idempotence. The faithful lens
   (comment- and structure-preserving formatting) runs over the curated valid
   corpus. Keep both bounded: a fixed deterministic corpus plus a seeded,
   fixed-iteration random pass, no external fuzz dependency.

**Known limitation (tracked for a follow-up lane).** An own-line comment at an
irregular indentation *inside* a `match` or compound-statement body has no owner in
the AST — `Match` and the compound bodies do not model inter-arm/inter-statement
own-line comments, so such a comment is attributed by byte span and can shift or be
dropped when formatting normalizes indentation. Formatter idempotence is therefore
asserted unconditionally only for comment-free clean parses; giving these nodes a
comment-owning structure is the fix.

Map: [docs/implementation/syntax.md](../../docs/implementation/syntax.md).
