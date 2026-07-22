# Machine-Readable Language Facts

Marrow exposes its language surface as structured facts a program can consume
without reconstructing the language. A tool — an editor extension, a code
generator, or a program that reads or writes `.mw` source — reads these facts
rather than reimplementing the lexer, parser, type system, or path model. This
page indexes those facts and gives the two lexical inventories a machine most
often needs: the reserved words and the token kinds. Both inventories are
**drift-checked against the parser**: a test in `marrow-syntax` renders each list
from the parser's own tables and fails when the committed list here no longer
matches, so this page cannot silently fall behind the compiler.

The parser is the single authority for syntax. This page publishes facts the
parser owns; it does not define syntax, and nothing here overrides the
[grammar](../language/grammar.md) or any other reference page.

## Structured outputs a tool consumes

| Fact | Where it comes from | Shape |
|---|---|---|
| Command outcomes | `marrow run`/`marrow test` with `--format jsonl` | One canonical JSON object per line; `outcome` is one of `value`, `diagnostic`, `artifact_rejected`, `fault`, or `error`, keeping the [four failure families](../language/errors-and-transactions.md) distinct. |
| Diagnostics | every command | A typed dotted code (`check.unsupported`, `parse.syntax`, …) with a 1-based source span; the closed registry is the [Error Code Reference](../error-codes.md). |
| Durable access demand | `marrow check` (summary), `marrow check --demand` (full) | The default summary groups each export's read/write durable demand by module, rolled up to roots. `--demand` prints one line per exported function naming every durable place its whole call graph reads and writes, in source spelling (`bookstore.put reads ^books; writes ^books`). |
| Editor facts | `marrow lsp` | Diagnostics, whole-document formatting, hover, and go-to-definition, served from the compiler's published analysis facts over the Language Server Protocol; see the [language server](lsp.md). |
| Wire interface | `marrow client typescript` | A generated strict client whose method signatures and transfer types are reconstructed from the verified image; see the [TypeScript client](typescript-client.md). |

Each of these is a projection of one compiler-owned model. A tool that needs a
fact Marrow does not yet publish asks for the fact to be added to the compiler
rather than recomputing it from source text.

## Reserved words

The following words are reserved: the lexer classifies each as a keyword, so none
is available as an identifier. The set is case-sensitive — `Error`, `ErrorCode`,
and `Id` are reserved with their capitalization, and a lowercase `error` is an
ordinary identifier. Some words are contextual in the grammar (`category`, `by`,
`at most`, `from`, `on more`, and the duration units are read as keywords only in
specific positions and are not in this set), and some reserved words are held for
a future clause and are not yet grammar (`writes`, `reads`, `merge`, `journal`,
`sensitive`, `declassify`, `lock`). A word being reserved means only that the
lexer will not treat it as an identifier.

<!-- BEGIN reserved-words -->
```text
absent alias and assert bool break bytes checked const continue date decimal
declassify delete duration else enum Error ErrorCode false fn for Id if in index
instant int is journal lock match merge module not or place pub reads required
require resource return sensitive store string struct supports test transaction
true try type unique unknown unset use var while writes
```
<!-- END reserved-words -->

## Token kinds

The lexer produces exactly these token kinds. Trivia (`Comment`, `DocComment`,
`Newline`, `Eof`) is included because a tool that reformats or spans source
observes it. `Keyword` carries one of the reserved words above.

<!-- BEGIN token-kinds -->
```text
Identifier Integer Decimal Duration String InterpolationStart InterpolationText
InterpolationExprStart InterpolationExprEnd InterpolationEnd Bytes Keyword Comment
DocComment Newline Eof LeftParen RightParen LeftBracket RightBracket LeftBrace
RightBrace FatArrow Colon DoubleColon Comma Dot DotDot DotDotEqual Equal EqualEqual
BangEqual Question QuestionDot QuestionQuestion Less LessEqual Greater GreaterEqual
Plus Minus Star Slash Percent PlusEqual MinusEqual StarEqual SlashEqual PercentEqual
Caret
```
<!-- END token-kinds -->

## Grammar

The current EBNF summary of the `.mw` surface is the [grammar](../language/grammar.md)
page. That grammar is **hand-maintained** against the recursive-descent parser and
verified two ways: every complete `mw` example in the reference is compiled and
independently verified by the documentation gate, and the syntax corpus proves the
same sources parse and format. The parser is a recursive-descent implementation
and exposes no production table to render a full grammar from, so the production
bodies on the grammar page are not mechanically derived from the parser the way
the two lexical inventories above are. This is the one recorded gap between the
published grammar and a fully generated artifact; the reserved-word and
token-kind inventories close the part of that gap that changes most often and is
most error-prone to restate by hand.

## Drift enforcement

The reserved-word and token-kind blocks above are read back by a test in the
`marrow-syntax` test tree. The test derives the same two sets from the parser —
the reserved-word set through the public `is_reserved_word` predicate over an
exhaustive enumeration of the keyword type, and the token-kind set through an
exhaustive match over the token-kind type — and asserts they equal the sets
parsed out of this page. Adding, removing, or renaming a keyword or token kind in
the parser makes that exhaustive match fail to compile, and changing the set makes
the comparison fail, so a parser change that outpaces this page fails a check
rather than passing silently. When the parser changes, update the block here in
the same change.
