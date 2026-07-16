# Grammar

This page gives an EBNF summary of the current non-legacy `.mw` language. It
describes source shape only; the other reference pages define name resolution,
types, presence, effects, and runtime behavior. The prototype `surface`
syntax was deleted at B00 and is intentionally excluded.

Quoted text is literal. `A?`, `A*`, and `A+` mean optional, zero or more, and one
or more. `A | B` selects an alternative. The indentation lexer emits `INDENT`,
`DEDENT`, and `NEWLINE`.

## Lexical Tokens

```ebnf
identifier      = (letter | "_"), {letter | digit | "_"} ;
qualified_name  = identifier, {"::", identifier} ;

integer_lit     = digit, {digit} ;
decimal_lit     = digit, {digit}, ".", digit, {digit} ;
duration_lit    = digit, {digit}, ".", duration_unit ;
duration_unit   = "second" | "seconds"
                | "minute" | "minutes"
                | "hour" | "hours"
                | "day" | "days"
                | "week" | "weeks" ;

string_lit      = '"', {string_char}, '"' ;
bytes_lit       = 'b"', {byte_char}, '"' ;
interp_lit      = '$"', {interp_part}, '"' ;

string_char     = string_text | string_escape ;
string_escape   = "\", ('"' | "\" | "n" | "r" | "t") ;
byte_char       = byte_text | string_escape | hex_escape ;
hex_escape      = "\x", hex_digit, hex_digit ;
interp_part     = interp_text | "{{" | "}}" | "{", expression, "}" ;

comment         = ";", {not_newline} ;
doc_comment     = ";;", {not_newline} ;

letter          = "A"…"Z" | "a"…"z" ;
digit           = "0"…"9" ;
hex_digit       = digit | "A"…"F" | "a"…"f" ;
```

`string_text` excludes `"`, `\`, and newline. `byte_text` has the same source
shape and contributes its UTF-8 bytes. Interpolation text additionally excludes
an unescaped `{`.

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
                 | function_decl
                 | evolve_decl) ;

alias_decl      = "alias", identifier, "=", type, NEWLINE ;

nominal_decl    = "type", identifier, ":", type,
                  "in", expression,
                  ("supports", identifier, {",", identifier})?, NEWLINE ;

const_decl      = "const", identifier, type_annotation?,
                  "=", expression, NEWLINE ;
```

## Resources And Stores

```ebnf
resource_decl   = "resource", identifier, NEWLINE,
                  INDENT, resource_member+, DEDENT ;

resource_member = {doc_comment},
                  (field_decl | keyed_field_decl | group_decl) ;

field_decl      = required_marker?, identifier,
                  type_annotation, NEWLINE ;

keyed_field_decl = identifier, key_params,
                   type_annotation, NEWLINE ;

group_decl      = identifier, key_params?, NEWLINE,
                  INDENT, resource_member+, DEDENT ;

required_marker = "required" ;

store_decl      = "store", saved_root, key_params?,
                  ":", identifier, NEWLINE,
                  (INDENT, store_member+, DEDENT)? ;

store_member    = {doc_comment}, index_decl ;
index_decl      = "index", identifier, "(",
                  index_arg_list, ")", "unique"?, NEWLINE ;

key_params      = "(", key_decl, {",", key_decl}, ","?, ")" ;
key_decl        = identifier, type_annotation ;

index_arg_list  = field_path, {",", field_path}, ","? ;
field_path      = identifier, {".", identifier} ;

saved_root      = "^", identifier ;
```

A dense product value type shares the resource member syntax, but a struct field
is the bare `identifier, type_annotation` form; the `required` marker, key
parameters, and groups are rejected by the checker.

```ebnf
struct_decl     = "struct", identifier, NEWLINE,
                  INDENT, struct_field+, DEDENT ;

struct_field    = {doc_comment}, identifier, type_annotation, NEWLINE ;
```

## Enums

```ebnf
enum_decl       = visibility?, "enum", identifier, NEWLINE,
                  INDENT, enum_member+, DEDENT ;

enum_member     = {doc_comment}, "category"?, identifier, payload?, NEWLINE,
                  (INDENT, enum_member+, DEDENT)? ;

payload         = "(", payload_field, {",", payload_field}, ","?, ")" ;
payload_field   = identifier, ":", type ;
```

A member with a `payload` is a payload variant; a bare member carries no payload.
The `category` modifier and nested members are parsed but currently rejected by
the checker (`check.unsupported`): the flat enum is the current form and
hierarchical enums are future.

## Functions

```ebnf
function_decl   = visibility?, "fn", identifier, type_params?,
                  "(", param_list?, ")", return_type?,
                  NEWLINE, block ;

visibility      = "pub" ;
type_params     = "[", type_param, {",", type_param}, "]" ;
type_param      = identifier, ("supports", ("equality" | "order"))? ;
param_list      = param_decl, {",", param_decl}, ","? ;
param_decl      = {doc_comment}, identifier, key_params?,
                  type_annotation ;
return_type     = ":", type ;
block           = INDENT, statement+, DEDENT ;
```

Line breaks may separate parameters in a multiline list. A keyed parameter uses
the same `key_params` shape as a keyed local declaration. An optional
`type_params` list declares rank-1 generic type parameters, each usable as a type
in the body and optionally carrying one closed `supports equality`/`supports
order` constraint; see [functions](modules-and-functions.md#generic-functions).

## Evolution

```ebnf
evolve_decl     = "evolve", NEWLINE,
                  INDENT, evolve_step+, DEDENT ;

evolve_step     = "rename", evolve_target, "->",
                            evolve_target, NEWLINE
                | "default", evolve_target, "=",
                             expression, NEWLINE
                | "retire", evolve_target, NEWLINE
                | "transform", evolve_target, NEWLINE, block ;

evolve_target   = saved_path | qualified_name | local_path ;
```

`rename`, `default`, `retire`, and `transform` are contextual within an
`evolve` block.

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
generic_type     = identifier, "[", type, {",", type}, "]" ;
```

`generic_type` is a generic type application: any identifier head carrying a
bracket group of comma-separated type arguments. The head is either a reserved
toolchain generic — the value types `Option[T]` and `Result[T, E]` and the finite
collection types `List[T]` and `Map[K, V]` — or a user-declared generic
`struct`/`enum` template. The parser accepts any head; the checker resolves it and
enforces argument arity (`Option` and `List` take one type, `Result` and `Map`
take two; a user template takes its declared number).

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
                | transaction_stmt
                | expression_stmt ;

const_stmt      = "const", identifier, type_annotation?,
                  "=", (try_value | expression), NEWLINE ;

var_stmt        = "var", identifier, key_params?,
                  type_annotation?, ("=", (try_value | expression))?, NEWLINE ;

assignment_stmt = assignable, "=", expression, NEWLINE ;

compound_assignment_stmt =
                  assignable, compound_op, expression, NEWLINE ;
compound_op     = "+=" | "-=" | "*=" | "/=" | "%=" ;

place_stmt      = "place", identifier, "=", expression, NEWLINE ;

delete_stmt     = "delete", path_expr, NEWLINE ;
break_stmt      = "break", NEWLINE ;
continue_stmt   = "continue", NEWLINE ;
return_stmt     = "return", (try_value | expression)?, NEWLINE ;
expression_stmt = (try_value | expression), NEWLINE ;
```

## Conditionals, Loops, And Match

```ebnf
if_stmt         = "if", expression, NEWLINE, block,
                  else_if_clause*, else_clause? ;

if_const_stmt   = "if", "const", identifier, type_annotation?,
                  "=", expression, NEWLINE, block,
                  else_if_clause*, else_clause? ;

else_if_clause  = "else", "if", expression, NEWLINE, block ;
else_clause     = "else", NEWLINE, block ;

while_stmt      = "while", expression, NEWLINE, block ;

for_stmt        = "for", for_binding, "in", "reversed"?,
                  expression, ("by", expression)?,
                  ("at", "most", expression, ("from", expression)?)?,
                  NEWLINE, block, on_more_clause? ;

on_more_clause  = "on", "more", NEWLINE, block ;

for_binding     = identifier, {",", identifier} ;

match_stmt      = "match", expression, NEWLINE,
                  INDENT, match_arm+, DEDENT ;
match_arm       = identifier, {"::", identifier}, arm_bindings?, NEWLINE, block ;
arm_bindings    = "(", identifier, {",", identifier}, ","?, ")" ;
```

`by`, `at most`, `from`, and the trailing `on more` are contextual in a `for`
head and its bounded durable-traversal clause. `category` is contextual in an
enum body.

## Transactions And `try`

```ebnf
transaction_stmt = "transaction", NEWLINE, block ;

try_value        = "try", expression ;
```

Prefix `try_value` propagates a `Result[T, E]` failure. It is a statement-level
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
                  (("<" | "<=" | ">" | ">="), range_expr)? ;

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
                | field_suffix
                | optional_field_suffix ;

paren_suffix    = "(", argument_list?, ")" ;
field_suffix    = ".", identifier ;
optional_field_suffix = "?.", identifier ;
```

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

literal         = integer_lit | decimal_lit | duration_lit
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
path_suffix     = paren_suffix | field_suffix ;
assignable      = identifier | path_expr ;
```

The checker distinguishes a function call, resource constructor, struct literal,
conversion, entry-identity constructor, and keyed path access after parsing the
common call shape.

## Arguments

```ebnf
argument_list       = argument, {",", argument}, ","? ;
named_argument_list = named_argument, {",", named_argument}, ","? ;
argument            = expression | named_argument ;
named_argument      = identifier, ":", expression ;
```

After a named argument, later arguments are also named.
