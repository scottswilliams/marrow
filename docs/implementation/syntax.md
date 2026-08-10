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
| Formatting | `format.rs` |
| Public entrypoints and limits | `lib.rs` |

`parse_source` is total over input text: malformed input produces diagnostics
and explicit error nodes rather than a partial AST that later passes attempt to
repair by parsing strings.

`lib.rs` publishes what a parse costs: `MAX_PARSE_BYTES_PER_SOURCE_BYTE`, the
fixed charge that does not scale with length, and `max_parse_bytes`, which
combines them for a given source length. The rate is a declared cap over every
node family the parser builds, so a consumer with a heap bound of its own can
compute a file's whole parse charge from its byte length and refuse it before
parsing. `marrow-compile` re-derives the rate from the representation and fails
if the published constant drifts from it.

The AST keeps its allocations exactly sized where it can. A block's statement
list and the file's declaration list are `Box<[T]>`, allocated once at a measured
count, so amortized growth slack cannot survive into a finished tree. A name path
is a `Box<[NameSegment]>` rather than a spelling vector beside a span vector, so
an unequal length is not representable.

The formatter consumes parser-owned structure. It must preserve comments and
reparse to an equivalent AST; source formatting is not a semantic pass.

Tests under `crates/marrow-syntax/tests/` cover token boundaries, parser
families, error-node invariants, nesting limits, formatting round trips, and
every verified example in `docs/language/`.
