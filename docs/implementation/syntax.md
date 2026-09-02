# Syntax implementation

`marrow-syntax` turns UTF-8 source into an explicit AST and formats that AST
back to canonical source. It owns source shape, tokens, spans, comments, parser
recovery nodes, and syntax diagnostics. It does not resolve names or types.

## Code map

| Area | Files |
|---|---|
| Tokenization and literals | `lexer.rs`, `token.rs`, `literal.rs` |
| Declaration parsing | `parse_decl/` |
| Expression and statement parsing | `parse_expr.rs` |
| AST | `ast.rs` |
| Syntax diagnostics | `diagnostic.rs` |
| Formatting | `format.rs` |
| Public entrypoints and limits | `lib.rs` |

`parse_source` is total over input text. Malformed input produces diagnostics
and explicit error nodes, so a later pass reads a tree and never a string.
A missing brace is reported once, at the brace that opened the block:

```text
src/main.mw:3:24: parse.syntax: expected `}` to close this block
```

## Parse cost

`lib.rs` publishes what a parse costs. `MAX_PARSE_BYTES_PER_SOURCE_BYTE` caps
the heap a parse takes per source byte, and `MAX_PARSE_FIXED_BYTES` caps the
part that does not scale with length. `max_parse_bytes` combines them, so a
caller with a heap bound of its own refuses a file before parsing it.
`marrow-compile` re-derives the rate from the representation and fails if the
published constant drifts from it.

The AST keeps its allocations exactly sized. A block's statement list, a
`match` body's arm list, and a file's declaration list are `Box<[T]>`,
allocated once at a measured count. The pass that measures a count also decides
the region's structure, so no second counter disagrees about what the tree
holds. Every path is one `Box<[NameSegment]>` carrying spelling and span
together.

## Nesting depth

Depth is a separate bound with a separate owner. A measurement is keyed on a
`{`, and a trailing clause can nest without one: `else` followed by `if`, or a
`match` arm whose body is one statement. The statement parser therefore counts
frames on every descent and stops at the nesting limit, so the limit trips
before the native stack does.

The formatter consumes parser-owned structure. It preserves comments and
reparses to an equivalent AST. Tests under `crates/marrow-syntax/tests/` cover
token boundaries, parser families, error-node invariants, nesting limits,
formatting round trips, and every `mw` fence in `docs/`.
