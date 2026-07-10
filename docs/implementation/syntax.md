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

The formatter consumes parser-owned structure. It must preserve comments and
reparse to an equivalent AST; source formatting is not a semantic pass.

Tests under `crates/marrow-syntax/tests/` cover token boundaries, parser
families, error-node invariants, nesting limits, formatting round trips, and
every verified example in `docs/language/`.
