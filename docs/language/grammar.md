# Grammar

This page gives an EBNF summary of the current non-legacy `.mw` language. It
describes source shape only; the other reference pages define name resolution,
types, presence, effects, and runtime behavior. The prototype `surface`
syntax was deleted at B00 and is intentionally excluded.

Quoted text is literal. `A?`, `A*`, and `A+` mean optional, zero or more, and one
or more. `A | B` selects an alternative. The lexer emits `NEWLINE` at each
significant line break; blocks are delimited by `{` and `}`, and a statement
terminates at a `NEWLINE` or a closing `}`.

## Blocks And Lines

Every block is a brace-delimited statement sequence. A block opens with `{` at
the end of its header line (one-true-brace) and closes with `}`. The closing `}`
either stands on its own line or is immediately followed by a cuddled trailing
clause (`else`, `else if`, `on more`, a `checked` fault arm). Braces are
mandatory for every block, including a single-statement body; there is no
brace-free block and no statement separator.

```ebnf
block           = "{", [ NEWLINE ], { statement, NEWLINE }, [ statement ], "}" ;
```

An empty body (`{}`) and an inline single-statement body (`{ statement }`) are
both admitted: the opening `{` is followed by an optional `NEWLINE`, any number
of `NEWLINE`-terminated statements, and an optional final statement before `}`.

A statement terminates at a `NEWLINE` or the block's closing `}`; there is no
`;` separator. A header line may continue across a physical line break after a
trailing `and`, `or`, `,`, or `=`, and continuation is also implicit inside an
open `(` or `[`; the header ends at its opening `{`. Indentation carries no
meaning and is pure formatter output.

## Lexical Tokens

```ebnf
identifier      = (letter | "_"), {letter | digit | "_"} ;
qualified_name  = identifier, {"::", identifier} ;

integer_lit     = digit, {digit} ;
decimal_lit     = digit, {digit}, ".", digit, {digit} ;
duration_word_lit = integer_lit, duration_unit ;
duration_unit   = "second" | "seconds"
                | "minute" | "minutes"
                | "hour" | "hours"
                | "day" | "days"
                | "week" | "weeks" ;

string_lit      = '"', {string_char}, '"' ;
bytes_lit       = 'b"', {byte_char}, '"' ;
interp_lit      = '$"', {interp_part}, '"' ;

string_char     = string_text | string_escape ;
string_escape   = "\", ('"' | "\" | "n" | "r" | "t")
                | unicode_escape ;
unicode_escape  = "\u{", hex_digit, {hex_digit}, "}" ;
byte_char       = byte_text | string_escape | hex_escape ;
hex_escape      = "\x", hex_digit, hex_digit ;
interp_part     = interp_text | "{{" | "}}"
                | unicode_escape | "{", expression, "}" ;

comment         = "//", {not_newline} ;
doc_comment     = "///", {not_newline} ;

letter          = "A"…"Z" | "a"…"z" ;
digit           = "0"…"9" ;
hex_digit       = digit | "A"…"F" | "a"…"f" ;
```

`string_text` excludes `"`, `\`, and newline. `byte_text` has the same source
shape and contributes its UTF-8 bytes. Interpolation text additionally excludes
an unescaped `{`. A `unicode_escape` carries one to six hexadecimal digits
naming a Unicode scalar value; it is admitted in `string_lit` and the text parts
of `interp_lit`, where it is recognized before the `{`-hole detection so its
braces never open an expression. It is not a byte escape: `bytes_lit` spells
non-ASCII bytes with `hex_escape` only. A `//` line is a comment and a `///`
line is a documentation comment for the following declaration; both run to the
end of the line.

## Source File

```ebnf
source_file     = module_decl?, {use_decl}, {top_level_decl}, EOF ;

module_decl     = "module", qualified_name, NEWLINE ;
use_decl        = "use", qualified_name, NEWLINE ;

top_level_decl  = {doc_comment},
                  (alias_decl
                 | nominal_decl
                 | const_decl
                 | resource_decl
                 | struct_decl
                 | store_decl
                 | enum_decl
                 | function_decl) ;

alias_decl      = "alias", identifier, "=", type, NEWLINE ;

nominal_decl    = "type", identifier, ":", type,
                  "in", expression,
                  ("supports", identifier, {",", identifier})?, NEWLINE ;

const_decl      = "const", identifier, type_annotation?,
                  "=", expression, NEWLINE ;
```

## Resources And Stores

```ebnf
resource_decl   = "resource", identifier, "{", NEWLINE,
                  resource_member+, "}" ;

resource_member = {doc_comment},
                  (field_decl | keyed_field_decl | group_decl) ;

field_decl      = required_marker?, identifier,
                  type_annotation, NEWLINE ;

keyed_field_decl = identifier, key_params,
                   type_annotation, NEWLINE ;

group_decl      = identifier, key_params?, "{", NEWLINE,
                  resource_member+, "}" ;

required_marker = "required" ;

store_decl      = "store", saved_root, key_params?,
                  ":", identifier,
                  ("{", NEWLINE, store_member+, "}")?, NEWLINE ;

store_member    = {doc_comment}, index_decl ;
index_decl      = "index", identifier, "[",
                  index_arg_list, "]", "unique"?, NEWLINE ;

key_params      = "[", key_decl, {",", key_decl}, ","?, "]" ;
key_decl        = identifier, type_annotation ;

index_arg_list  = field_path, {",", field_path}, ","? ;
field_path      = identifier, {".", identifier} ;

saved_root      = "^", identifier ;
```

Key declarations use square brackets — `store ^books[id: int]: Book`,
`notes[noteId: string]`, `tags[pos: int]: string` — mirroring the bracketed key
access that reads them (the declaration-mirrors-access law). A store with no
index members is written header-alone, without a `{}` body.

A dense product value type shares the resource member syntax, but a struct field
is the bare `identifier, type_annotation` form; the `required` marker, key
parameters, and groups are rejected by the checker.

```ebnf
struct_decl     = "struct", identifier, type_params?, "{", NEWLINE,
                  struct_field+, "}" ;

struct_field    = {doc_comment}, identifier, type_annotation, NEWLINE ;
```

## Enums

```ebnf
enum_decl       = visibility?, "enum", identifier, type_params?, "{", NEWLINE,
                  enum_member+, "}" ;

enum_member     = {doc_comment}, "category"?, identifier, payload?, NEWLINE,
                  ("{", NEWLINE, enum_member+, "}")? ;

payload         = "(", payload_field, {",", payload_field}, ","?, ")" ;
payload_field   = identifier, ":", type ;
```

Members are newline-separated, one per line; there is no separator token. A
member with a `payload` is a payload variant; a bare member carries no payload. A
payload is a parenthesized field list — a constructor parameter list matched by
name at construction, not a key tuple. The `category` modifier and nested
members are parsed but currently rejected by the checker (`check.unsupported`):
the flat enum is the current form and hierarchical enums are future.

## Functions

```ebnf
function_decl   = visibility?, "fn", identifier, type_params?,
                  "(", param_list?, ")", return_type?, block ;

visibility      = "pub" ;
type_params     = "<", type_param, {",", type_param}, ","?, ">" ;
type_param      = identifier, ("supports", ("equality" | "order"))? ;
param_list      = param_decl, {",", param_decl}, ","? ;
param_decl      = {doc_comment}, identifier, key_params?,
                  type_annotation ;
return_type     = ":", type ;
```

Line breaks may separate parameters in a multiline list. A keyed parameter uses
the same `key_params` shape as a keyed local declaration. An optional
`type_params` list declares rank-1 generic type parameters between angle
brackets, each usable as a type in the body and optionally carrying one closed
`supports equality`/`supports order` constraint; see
[functions](modules-and-functions.md#generic-functions).

## Types

```ebnf
type_annotation = ":", type ;
type             = base_type, optional_suffix? ;
optional_suffix  = "?" ;

base_type        = scalar_type
                 | qualified_name
                 | "Error"
                 | identity_type
                 | generic_type ;

scalar_type      = "int" | "bool" | "string" | "bytes"
                 | "decimal" | "date" | "instant" | "duration"
                 | "ErrorCode" | "unknown" ;

identity_type    = "Id", "(", saved_root, ")" ;
generic_type     = identifier, "<", type, {",", type}, ","?, ">" ;
```

`generic_type` is a generic type application between angle brackets: any
identifier head carrying a comma-separated type-argument list. The head is either
a reserved toolchain generic — the value types `Option<T>` and `Result<T, E>` and
the finite collection types `List<T>` and `Map<K, V>` — or a user-declared generic
`struct`/`enum` template. The parser accepts any head; the checker resolves it and
enforces argument arity (`Option` and `List` take one type, `Result` and `Map`
take two; a user template takes its declared number). Angle brackets are
unambiguous because Marrow has no expression-position type application: within an
expression `<` and `>` are always comparison operators, and a type-argument list
is reached only through an anchored type position. When the type parser needs a
closing `>` and the next token is the glued `>=` (as in `Map<string, int>= Map()`),
it splits that token into `>` and `=`; this is the only such split, since no `>>`
token exists. An optional suffix composes after the close: `Option<string>?`.

Keyed local-collection shapes are written on declarations, not as standalone
type annotations.

## Statements

```ebnf
statement       = const_stmt
                | var_stmt
                | place_stmt
                | assignment_stmt
                | compound_assignment_stmt
                | delete_stmt
                | if_stmt
                | if_const_stmt
                | while_stmt
                | for_stmt
                | match_stmt
                | break_stmt
                | continue_stmt
                | return_stmt
                | require_stmt
                | transaction_stmt
                | expression_stmt ;

const_stmt      = "const", identifier, type_annotation?,
                  "=", (try_value | expression), let_else_tail?, NEWLINE ;

var_stmt        = "var", identifier, key_params?,
                  type_annotation?, ("=", (try_value | expression))?,
                  let_else_tail?, NEWLINE ;

assignment_stmt = assignable, "=", expression, NEWLINE ;

compound_assignment_stmt =
                  assignable, compound_op, expression, NEWLINE ;
compound_op     = "+=" | "-=" | "*=" | "/=" | "%=" ;

place_stmt      = "place", identifier, "=", expression, NEWLINE ;

delete_stmt     = "delete", path_expr, NEWLINE ;
break_stmt      = "break", NEWLINE ;
continue_stmt   = "continue", NEWLINE ;
return_stmt     = "return", (try_value | expression)?, NEWLINE ;
require_stmt    = "require", expression, "else", expression, NEWLINE ;
expression_stmt = (try_value | expression), NEWLINE ;

let_else_tail   = "else", (statement | block) ;
```

A `let_else_tail` is the let-else form: a `const`/`var` binding may carry an
`else` clause that runs a diverging statement or block when the bound value is
absent (see [Control flow](control-flow.md#let-else-bindings)). A `require_stmt`
is the boolean guard: the first expression is a `bool` condition ending at the
first top-level `else`, and the second is the bare failure value of the
enclosing function's `Result` error type (see
[Control flow](control-flow.md#require-guards)).

## Conditionals, Loops, And Match

```ebnf
if_stmt         = "if", expression, block,
                  else_if_clause*, else_clause? ;

if_const_stmt   = "if", "const", identifier, type_annotation?,
                  "=", expression,
                  {"and", if_const_chain_part}, block,
                  else_if_clause*, else_clause? ;

if_const_chain_part = "const", identifier, type_annotation?, "=", expression
                    | expression ;

else_if_clause  = "else", "if", expression, block ;
else_clause     = "else", block ;

while_stmt      = "while", expression, block ;

for_stmt        = "for", for_binding, "in", "reversed"?,
                  expression, ("by", expression)?,
                  ("at", "most", expression, ("from", expression)?)?,
                  block, on_more_clause? ;

on_more_clause  = "on", "more", block ;

for_binding     = identifier, {",", identifier} ;

match_stmt      = "match", expression, "{", NEWLINE, match_arm+, "}" ;
match_arm       = identifier, {"::", identifier}, arm_bindings?,
                  "=>", (statement | block) ;
arm_bindings    = "(", identifier, {",", identifier}, ","?, ")" ;
```

A trailing clause cuddles the closing brace of the block before it: `} else {`,
`} else if c {`, `} on more { … }`. The B5 `if const` chaining form — one or
more `and`-joined existence bindings followed by an optional trailing condition
— is parsed so the grammar is complete but rejected by the checker
(`check.unsupported`) until adopted. A `match` arm is a member pattern, an
optional positional binding group, `=>`, and then one statement or a braced
block; the formatter renders every arm body as a braced multiline block cuddled
after `=>`. `by`, `at most`, `from`, and the trailing `on more` are
contextual in a `for` head and its bounded durable-traversal clause. `category`
is contextual in an enum body.

## Transactions And `try`

```ebnf
transaction_stmt = "transaction", block ;

try_value        = "try", expression ;
```

Prefix `try_value` propagates a `Result<T, E>` failure. It is a statement-level
value form only: it may stand as the top-level right-hand side of a `const_stmt`,
`var_stmt`, `return_stmt`, or `expression_stmt`, but never nested inside a larger
expression. The throw/catch channel was removed; a `throw`, block `try`/`catch`,
or `finally` is a rejected removed form.

## Expressions

```ebnf
expression      = or_expr ;

or_expr         = and_expr, {"or", and_expr} ;
and_expr        = is_expr, {"and", is_expr} ;
is_expr         = equality_expr, ("is", equality_expr)? ;

equality_expr   = comparison_expr,
                  (("==" | "!="), comparison_expr)? ;

comparison_expr = range_expr,
                  ( ("<" | "<=" | ">" | ">="), range_expr
                  | ("in" | "not", "in"), range_expr )? ;

range_expr      = coalesce_expr, range_tail?
                | open_range ;
range_tail      = ("..", coalesce_expr?
                 | "..=", coalesce_expr), range_step? ;
open_range      = ("..", coalesce_expr?
                 | "..=", coalesce_expr), range_step? ;
range_step      = "by", coalesce_expr ;

coalesce_expr   = additive_expr, ("??", coalesce_expr)? ;
additive_expr   = multiplicative_expr,
                  {("+" | "-"), multiplicative_expr} ;
multiplicative_expr =
                  unary_expr, {("*" | "/" | "%"), unary_expr} ;

unary_expr      = ("-" | "not"), unary_expr
                | postfix_expr ;

postfix_expr    = primary_expr, {postfix_op} ;
postfix_op      = paren_suffix
                | key_suffix
                | field_suffix
                | optional_field_suffix ;

paren_suffix    = "(", argument_list?, ")" ;
key_suffix      = "[", expression, {",", expression}, ","?, "]" ;
field_suffix    = ".", identifier ;
optional_field_suffix = "?.", identifier ;
```

A comparison is single and non-associative — the `?` on `comparison_expr` admits
at most one operator — so `a < b > c` is a parse error and `<`/`>` never chain.
Interval membership (`value in lo..hi`, `value not in lo..hi`) sits at this level
with a range right operand and shares the non-association: `a in r in s` and
`a in r < b` are parse errors. Because the `in` here is a comparison operator, an
expression-level `in` never appears at the top level of a `for` head's iterable or
a nominal-type interval — those heads split on their leading `in` before the
expression grammar runs.
A `paren_suffix` is invocation or construction; a `key_suffix` is keyed address —
an ordered tuple of positional key values selecting an entry (`^books[id]`,
`^grid[a, b]`, `visit.obs[oid]`). The two never mix: a bracket group holds
positional expressions and never a named argument, and a parenthesized group is a
call or constructor and never an address.

## Primary Expressions And Paths

```ebnf
primary_expr    = literal
                | "true"
                | "false"
                | "absent"
                | identifier
                | qualified_name
                | saved_path
                | conversion_call
                | identity_constructor
                | resource_constructor
                | "(", expression, ")" ;

literal         = integer_lit | decimal_lit | duration_word_lit
                | string_lit | bytes_lit | interp_lit ;

conversion_call = conversion_type, "(", argument_list?, ")" ;
conversion_type = "int" | "bool" | "string" | "bytes" | "decimal"
                | "date" | "instant" | "duration" | "ErrorCode" ;

identity_constructor = "Id", "(", saved_root,
                       {",", expression}, ")" ;

resource_constructor =
                  qualified_name, "(", named_argument_list?, ")"
                | "Error", "(", named_argument_list?, ")" ;

struct_literal  = identifier, "(", named_argument_list?, ")" ;

path_expr       = saved_path | local_path ;
saved_path      = "^", identifier, {path_suffix} ;
local_path      = identifier, path_suffix, {path_suffix} ;
path_suffix     = key_suffix | field_suffix ;
assignable      = identifier | path_expr ;
```

Keyed address is carried by the parse: a `key_suffix` builds a keyed-access node
directly. The checker distinguishes a function call, resource constructor, struct
literal, conversion, and entry-identity constructor from the common parenthesized
call shape after parsing.

In `duration_word_lit`, the `duration_unit` is contextual: it is read as a unit only
immediately after an integer literal, a position where an identifier is otherwise a
parse error, so an ordinary name spelling a unit (`const seconds = 5`) is unaffected.
The unit set is the fixed spans only; `month` and `year` in that position are a parse
error, since their span is not fixed. The dotted `NUMBER.UNIT` form (`1.day`) is not a
duration literal — it is refused (`check.unsupported`), and a fractional-second span
uses the `duration("PT…")` constructor.

## Arguments

```ebnf
argument_list       = argument, {",", argument}, ","? ;
named_argument_list = named_argument, {",", named_argument}, ","? ;
argument            = expression | named_argument ;
named_argument      = identifier, ":", expression ;
```

After a named argument, later arguments are also named.
