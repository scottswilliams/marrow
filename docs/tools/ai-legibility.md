# Machine-readable language facts

Marrow publishes its language surface as structured facts a program can consume.
An editor extension, a code generator, or a program that reads or writes `.mw`
source reads these facts instead of reimplementing the lexer, parser, type
system, or path model. The parser is the single authority for syntax; this page
publishes facts the parser owns and defines none of its own.

## Structured outputs

| Fact | Source | Shape |
|---|---|---|
| Command outcomes | `marrow run` and `marrow test` with `--format jsonl` | One canonical JSON object per line; `outcome` is `value`, `diagnostic`, `artifact_rejected`, `fault`, or `error`, keeping the [failure kinds](../language/errors-and-transactions.md#failure-kinds) distinct. |
| Diagnostics | every command | A dotted code (`check.type`, `parse.syntax`) with a 1-based source span; the closed registry is the [error code reference](../error-codes.md). |
| Durable access demand | `marrow check` and `marrow check --demand` | The summary groups each export's reads and writes by module. `--demand` prints one line per export naming every durable place its call graph reads and writes, in source spelling: `main.put reads ^books; writes ^books`. |
| Editor facts | `marrow-lsp` | Diagnostics, formatting, hover, definition, completion, signature help, and document symbols over the Language Server Protocol, from the [language server](lsp.md). |
| Wire interface | `marrow client typescript` | A generated strict client whose method signatures and transfer types come from the verified image, described under [TypeScript client](typescript-client.md). |

Each of these is a projection of one compiler model. A tool that needs a fact
Marrow does not publish asks for the fact to be added to the compiler.

## Reserved words

The lexer classifies each of the following words as a keyword, so none is
available as an identifier. The set is case-sensitive: `Error`, `ErrorCode`, and
`Id` are reserved with their capitalization, and a lowercase `error` is an
ordinary identifier. Some words are contextual in the grammar: `by`, `at most`,
`from`, `on more`, and the duration units are read as keywords only in specific
positions and are outside this set. Some reserved words are held for a future
clause: `writes`, `reads`, `merge`, `journal`, `sensitive`, `declassify`, `lock`.
A reserved word means only that the lexer treats it as a keyword.

<!-- BEGIN reserved-words -->
```text
absent alias and assert bool break bytes checked const continue date decimal
declassify delete duration else enum Error ErrorCode false fn for Id if in index
instant int is journal lock match merge module not or place pub reads require
required resource return sensitive store string struct supports test transaction
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

## Drift checks

A test in the `marrow-syntax` crate renders the two inventories above from the
parser's own tables and compares them with the blocks on this page, word for
word. The reserved-word set comes from the `is_reserved_word` predicate over an
exhaustive enumeration of the keyword type; the token-kind set comes from an
exhaustive match over the token-kind type. Adding, removing, or renaming a keyword
or token kind in the parser fails that test until the block here changes in the
same commit.

## Grammar

The [grammar](../language/grammar.md) page is the EBNF summary of the `.mw`
surface. It is maintained by hand against the recursive-descent parser, which
has no production listing to render it from. It is checked two ways: every
complete `mw` example in the reference compiles and passes `marrow test`, and the
syntax corpus proves the same sources parse and format. The two inventories above
are the mechanically derived part of the published grammar.
