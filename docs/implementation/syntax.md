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
count, so amortized growth slack cannot survive into a finished tree. The pass
that measures a statement count is also the one that decides the parser
structures the region it counted: it stops measuring at the nesting limit, and
the parser builds exactly the brace-delimited regions that carry a measurement —
a block, and a `match` body, whose arm list is sized the same way — so no second
counter disagrees with it about what the tree holds and a list sized at nothing
and grown by doubling is not representable. Every path — a module or import name,
a `match` arm, an `index` argument, a name expression — is a `Box<[NameSegment]>`
rather than a spelling beside a parallel span vector, so an unequal length is not
representable.

How deep the parser descends is a separate bound with a separate owner. A
measurement is keyed on a `{`, so it can only refuse a body that opens one, and a
trailing clause takes a single inline statement in place of a block (`else`
followed by an `if` on the next line, a `match` arm whose body is a statement).
Such a nest recurses without opening a brace. The statement parser therefore
counts frames as well: every descent passes one counter that stops at the nesting
limit, so the typed limit trips before the native stack does on every path, and
the depth of a parsed tree is bounded by that limit rather than by how long the
file is.

The formatter consumes parser-owned structure. It must preserve comments and
reparse to an equivalent AST; source formatting is not a semantic pass.

Tests under `crates/marrow-syntax/tests/` cover token boundaries, parser
families, error-node invariants, nesting limits, formatting round trips, and
every verified example in `docs/language/`.
