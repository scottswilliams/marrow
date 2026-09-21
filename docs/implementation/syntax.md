# Syntax implementation

`marrow-syntax` turns UTF-8 source into an explicit AST and formats that AST
back to canonical source. It owns source shape, tokens, spans, comments, parser
recovery nodes, and syntax diagnostics. It does not resolve names or types.

## Code map

| Area | Files |
|---|---|
| Tokenization and literals | `lexer.rs`, `token.rs`, `literal.rs` |
| Declaration parsing | `parse_decl/decl.rs`, `parse_decl/members.rs` |
| Statement parsing | `parse_decl/stmt.rs`, `parse_decl/statement_lines.rs`, `parse_decl/body.rs` |
| Position-query syntax | `query.rs` |
| Expression parsing | `parse_expr.rs` |
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

`QuerySyntax::parse` builds transient syntax for completion and active-call
queries. It binds the byte position to a partial tree, retaining all declaration
headers, uses, and body spans. It lexes the whole file and uses the declaration
parser's existing block framing and recovery. Only a closed function/test body
containing the position has its statements constructed; other closed bodies
have empty statement lists. Body spans include both endpoints for position
selection. Const expressions and structural declarations remain fully parsed.
The partial type exposes no complete `SourceFile` and supplies no diagnostics.
Compilation, formatting and snapshot diagnostics continue to use full parsing.
The existing conservative full-parse charge also covers this query product.

## Parse cost

`lib.rs` publishes what a parse costs. `MAX_PARSE_BYTES_PER_SOURCE_BYTE` caps
the heap a parse takes per source byte, and `MAX_PARSE_FIXED_BYTES` caps the
part that does not scale with length. `max_parse_bytes` combines them, so a
caller with a heap bound of its own refuses a file before parsing it.
`marrow-compile` re-derives the rate from the representation and fails if the
published constant drifts from it.

The AST keeps its final lists in boxed slices. A block's statement list, a
`match` body's arm list, and a file's declaration list are grown by pushing and
boxed at close; the growth slack is part of the published per-source-byte
charge. Every path is one `Box<[NameSegment]>` carrying spelling and span
together. A binary expression holds its ordered left and right children in one
`Box<BinaryOperands>`. Each child's expression slot remains part of the parse
charge; sharing their allocation does not reduce the published heap term.

## Nesting depth

Each recursive descent owns its own bound. The statement parser counts frames
on every descent through `StmtParser::descend` — a braced block and a trailing
clause's single inline statement (`else` followed by `if`, a `match` arm whose
body is one statement) cost the same frame — and stops at the nesting limit,
reporting the `{` or statement it declines to open and skipping that region
whole. The declaration parser bounds nested member blocks the same way, the
expression and type parsers bound token-level nesting, and the lexer bounds
interpolation nesting. The limit trips before the native stack does on every
path.

The formatter consumes parser-owned structure. It preserves comments and
reparses to an equivalent AST. Tests under `crates/marrow-syntax/tests/` cover
token boundaries, parser families, error-node invariants, nesting limits,
formatting round trips, and every `mw` fence in `docs/`.
