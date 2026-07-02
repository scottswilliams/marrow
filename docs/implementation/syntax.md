# Syntax: `.mw` Text to AST

`marrow-syntax` is the compiler front end. It turns `.mw` source into a
`SourceFile` AST plus a span-sorted list of typed `Diagnostic`s, and renders an
AST back to canonical source. This page maps parser and formatter ownership; the
language syntax contract lives in `docs/language/syntax.md`. Meaning
(type/name resolution, enum/match validity, evolve semantics) is deferred to
later crates. Zero dependencies — it is the most upstream crate.

## Pipeline

`parse_source(&str) -> ParsedSource` is the file entry point. It runs two stages and merges their diagnostics:

1. `lex_source` — text to a flat token stream. The lexer maintains an indent stack and an `open_delimiters` counter, synthesizing `INDENT`/`DEDENT`/`NEWLINE` layout tokens (not lexical characters) and suppressing them inside `(`/`[`. It caps the indent stack at `NESTING_DEPTH_LIMIT`: a line nesting past the limit opens no block, has its content dropped, and reports a single `check.nesting_limit`, so the token stream stays bounded however deep the source nests. Tabs and obsolete operators (`&&`, `||`, `!`, `#`) and the reserved `~` are lexer diagnostics.
2. `DeclParser::parse` — tokens to declarations/statements/expressions via recursive descent.

`parse_expression(&str) -> (Option<Expression>, Vec<Diagnostic>)` is the expression-only public entry point used by callers that already know they are parsing one expression. It runs the same lexer, then feeds the token stream straight to `ExprParser::parse_complete`.

Diagnostics from each entry are sorted by `(line, start_byte)`. Output uses the `parse.syntax` code, except for the `check.nesting_limit` raised at `NESTING_DEPTH_LIMIT` (256) so deeply nested source fails closed rather than overflowing the stack. Layout nesting — statement blocks, resource groups, enum members — is capped in the lexer's indent stack, which keeps the token stream bounded so the parser and every later walk never recurse past the limit. Token-level nesting on one line — parentheses, unary and interpolated operands, and flat operator/postfix chains where each accumulation step counts and is unwound when the chain finishes — is capped by `ExprParser`'s depth counter. Nested `$"..."` interpolation string literals — a hole that itself contains an interpolated string — are capped in the lexer's hole scanner (`find_interpolation_expr_end`/`find_interpolation_string_end`), which raises the nesting-limit reason before the parser runs so an over-deep interpolation fails closed rather than mis-reporting as unterminated. A type annotation's spelling is stored as flat text and walked recursively downstream, so its bracket nesting is capped where every annotation is validated (`reject_structural_type_tokens`) before the `TypeRef` is built. Tests assert on the typed `reason`, never prose.

## Three parser layers, one token stream

Layout-token discipline differs by layer and mixing them breaks block framing:

- `DeclParser` — top-level dispatch and resource/store/surface/enum/function/const/evolve framing; keeps layout tokens to frame blocks by balanced `INDENT`/`DEDENT`. A keyword introduces its kind only when a literal space follows it (`module x` is a declaration, `module::x` is a name path; `evolve` is exempt).
- `StmtParser` — function/transform body statements; keeps layout tokens. Bodies are fed a byte-bounded token slice via `tokens_in_range(span)` so a trailing EOF `DEDENT` is excluded.
- `ExprParser` — a single expression over a trivia-filtered slice (no newlines/indents/comments); the full precedence ladder (or/and/is/equality/comparison/range/coalesce/additive/multiplicative/unary/postfix/primary).

A value the grammar cannot structure yields `None` plus a `parse.syntax` diagnostic, never a partial node. Diagnostics fire at most once per failing position (a `before = diagnostics.len()` guard suppresses the generic fallback when an inline rule already explained the failure).

## The AST

`ParsedSource = { file: SourceFile, diagnostics }`. `SourceFile` holds the optional `module`, `uses`, ordered `declarations`, and file-level `comments`, with name-lookup accessors downstream crates use. The AST records no parentheses — the formatter re-derives the minimum from a precedence table that must stay in sync with `parse_expr.rs`. `TypeRef` stores verbatim whitespace-stripped source text and is never resolved here; a return/parameter/local type may carry a trailing `?` optional suffix (`: string?`) recovered during type resolution, not stored as an AST flag. `absent` is an ordinary primary expression (`Expression::Absent`), so a `return` of `absent` is a `Return` of that value. Comments are retained as file, declaration-body, evolve-body, and statement-block trivia with `placement` and column so `parse -> format` round-trips losslessly. `SourceSpan` (file-absolute byte range plus 1-based line/column) is on every token, node, and diagnostic.

Compound assignment is represented as its own `Statement::CompoundAssign` with a
typed `CompoundAssignOp`; the parser does not desugar it into a binary expression
plus assignment. Statement parsing recognizes both adjacent operator tokens
(`x+=1`) and the whitespace-separated token sequence (`x + = 1`) for the five
arithmetic compound operators, leaving equality and comparison operators
unchanged.

## Formatter

`format_source` re-parses the source string (it does not take an AST) then renders canonical `.mw`; it is idempotent. It re-emits retained comments from the AST and keeps a parse/format structural fingerprint over the documented corpus. Within a body it preserves a single grouping blank line wherever the source held one or more (collapsing runs and dropping leading/trailing blanks), read from the source layout rather than the AST; the shape digest in `marrow-check` renders each declaration comment-free through `durable_shape_rendering`, which strips grouping blanks and all comment trivia (`;` and `;;`, own-line and trailing) because comments are prose and blanks are layout, not durable shape. `format_preserves_comments` is the losslessness predicate the CLI applies before emitting in any `fmt` mode (stdout, `--check`, `--write`). `format_declaration` and `format_expression` are public node-level renderers; note `format_declaration(source, decl)` still also takes the source `&str` for any statement body it carries, while only `format_expression(expr)` renders from an AST node alone.

Compound assignment formats canonically as `target op= value`, so `x*=1` and
`x * = 1` both render as `x *= 1`.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-syntax/src/lib.rs` | Crate root; re-exports the public surface and defines `parse_source` (file parse) plus `parse_expression` (single-expression parse). |
| `crates/marrow-syntax/src/lexer.rs` | The lexer: line splitting, indentation, `INDENT`/`DEDENT`/`NEWLINE`, numbers/durations/strings/bytes/interpolation/punctuation, tab/operator rejection. |
| `crates/marrow-syntax/src/token.rs` | `Token`/`TokenKind`/`Keyword`/`LexedSource`, the keyword table, `duration_unit_seconds`, lexical predicates (`is_identifier`, `is_qualified_name`, `tokens_in_range`). |
| `crates/marrow-syntax/src/active_call.rs` | Editor callable context facts: active call at a cursor and batch callable-callee spans, sharing declaration/type/member suppression rules. |
| `crates/marrow-syntax/src/parse_decl/` | The declaration and statement parsers, split by concern: `decl` (top-level dispatch and declaration bodies), `cursor` (shared `DeclParser` navigation and diagnostics), `members` (resource/store/enum member bodies), `surface` (surface headers and contextual surface items), `evolve` (evolution steps), `head` and `params` (token-slice declaration heads), `stmt` (`StmtParser` and compound-statement framing), `statement_lines` (single-statement-line parsers), `tokens` (low-level token-slice helpers); `mod.rs` holds the shared types and re-exports `DeclParser`. |
| `crates/marrow-syntax/src/parse_expr.rs` | `ExprParser`: single-expression recursive descent with the full precedence ladder. |
| `crates/marrow-syntax/src/ast.rs` | The full AST: `ParsedSource`, `SourceFile`, every declaration/statement/expression node, comment trivia, `span()` accessors, `TypeRef`. |
| `crates/marrow-syntax/src/diagnostic.rs` | `Diagnostic`, the typed reason tree, `Severity`, `SourceSpan`, the `Diagnose` trait (its `kind` delegates to `marrow-codes`). Code identity and `kind_for_code` live in `marrow-codes`. |
| `crates/marrow-syntax/src/literal.rs` | Canonical string-literal codec (`decode_string_literal`/`decode_string_escapes` and `encode_string_literal`/`push_string_escapes`, `StringLiteralError`) — single owner of the five `.mw` escapes in both directions. The `data` tools' text format owns its own broader codec (`marrow-check`'s `data_text`). |
| `crates/marrow-syntax/src/format.rs` | The formatter: `format_source` (re-parses then renders), per-node renderers, minimal precedence parens, comment re-emission. |

## Read next

- `crates/marrow-syntax/src/lib.rs` — `parse_source` (the whole pipeline in one function).
- `crates/marrow-syntax/src/parse_decl/decl.rs` — `DeclParser::dispatch_top_level`, `DeclParser::parse_function_body` (space-after gate, resource/store split, body byte-bounding).
- `crates/marrow-syntax/src/parse_expr.rs` — `ExprParser::expression`, `ExprParser::primary_expr` (precedence ladder; pairs with `format.rs` `binary_precedence`).
- `crates/marrow-syntax/src/lexer.rs` — `Lexer::lex`, `Lexer::apply_indent`, `Lexer::lex_interpolation` (text to layout tokens).
- `crates/marrow-syntax/src/format.rs` — `format_source`, `format_block` (re-parse and lossless comment interleaving).
