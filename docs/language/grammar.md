# Grammar

This page gives the syntax of a `.mw` source file in EBNF. It describes source
shape only; the other reference pages define name resolution, types, presence,
effects, and runtime behavior.

Quoted text is literal. Items separated by `,` occur in that order, and `;` ends
a rule. `A?` is optional, `{A}` is zero or more, `(A)+` is one or more, and
`A | B` selects an alternative. Parentheses group, and `"A"…"Z"` is a
character range. The lexer emits `NEWLINE` at each
significant line break; blocks are delimited by `{` and `}`, and a statement
terminates at a `NEWLINE` or a closing `}`. [Source and syntax](source-and-syntax.md)
defines which line breaks are significant.

Every production below is accepted by the parser and by `marrow check`. A form
the parser reads and `marrow check` reports as `check.unsupported` is not listed.

## Source file

```ebnf
source_file     = module_decl?, {use_decl | declaration}, EOF ;

module_decl     = "module", qualified_name, NEWLINE ;
use_decl        = "use", qualified_name, NEWLINE ;
qualified_name  = identifier, {"::", identifier} ;

declaration     = {doc_comment},
                  ( const_decl
                  | alias_decl
                  | nominal_decl
                  | resource_decl
                  | struct_decl
                  | store_decl
                  | enum_decl
                  | function_decl
                  | test_decl ) ;

block           = "{", NEWLINE?, {statement, NEWLINE}, "}" ;
```

A `module_decl` is the first line of a file that has one. A `use_decl` names a
module and may follow a declaration. A `doc_comment` run documents the
declaration, member, or parameter directly below it.

## Declarations

### Constants and type names

```ebnf
const_decl      = "const", identifier, type_annotation?, "=",
                  "-"?, (integer_lit | string_lit | "true" | "false"),
                  NEWLINE ;

alias_decl      = "alias", identifier, "=", type, NEWLINE ;

nominal_decl    = "type", identifier, ":", "int",
                  "in", range_expr,
                  ("supports", identifier, {",", identifier})?, NEWLINE ;
```

A top-level constant holds one scalar literal, optionally negated
([constants](modules-and-functions.md#constants)). The `supports` words of a
nominal type are listed under
[aliases and nominal ints](types-and-values.md#aliases-and-nominal-ints).

### Resources, structs, and stores

```ebnf
resource_decl   = "resource", identifier, "{", NEWLINE,
                  {resource_member, NEWLINE}, "}" ;

resource_member = {doc_comment}, (field_decl | group_decl | branch_decl) ;

field_decl      = "required"?, identifier, type_annotation ;
group_decl      = identifier, "{", NEWLINE,
                  {{doc_comment}, field_decl, NEWLINE}, "}" ;
branch_decl     = identifier, key_params, "{", NEWLINE,
                  {branch_member, NEWLINE}, "}" ;
branch_member   = {doc_comment}, (field_decl | group_decl | branch_decl) ;

key_params      = "[", key_decl, {",", key_decl}, "]" ;
key_decl        = identifier, type_annotation ;

struct_decl     = "struct", identifier, type_params?, "{", NEWLINE,
                  {struct_field, NEWLINE}, "}" ;
struct_field    = {doc_comment}, identifier, type_annotation ;

store_decl      = "store", durable_root, key_params?, ":", identifier,
                  ("{", NEWLINE, {index_decl, NEWLINE}, "}")?, NEWLINE ;

index_decl      = {doc_comment}, "index", identifier,
                  "[", identifier, {",", identifier}, "]", "unique"? ;

durable_root    = "^", identifier ;
```

Keys are declared with the same brackets that read them:
`store ^books[id: int]: Book` declares the root and `^books[id]` reads one entry.
A store with no index is written on its header line alone. A root without
`key_params` declares a singleton ([durable places](durable-places.md)). A group
holds fields; a branch holds fields, groups, and further branches under its own
keys ([members](resources.md#members)). Index rules are under
[index declarations](traversal-and-indexes.md#index-declarations).

### Enums

```ebnf
enum_decl       = "enum", identifier, type_params?, "{", NEWLINE,
                  (enum_member, NEWLINE)+, "}" ;

enum_member     = {doc_comment}, identifier, payload? ;
payload         = "(", payload_field, {",", payload_field}, ")" ;
payload_field   = identifier, ":", base_type ;
```

A member is one name per line. A payload member lists named fields, which a
constructor supplies by name: `Shape::rect(w: 2, h: 3)`.

### Functions and tests

```ebnf
function_decl   = "pub"?, "fn", identifier, type_params?,
                  "(", param_list?, ")", return_type?, block ;

type_params     = "<", type_param, {",", type_param}, ">" ;
type_param      = identifier, ("supports", ("equality" | "order"))? ;
param_list      = param_decl, {",", param_decl}, ","? ;
param_decl      = {doc_comment}, identifier, type_annotation ;
return_type     = ":", type ;

test_decl       = "test", string_lit, block ;
```

`pub` marks an export. A type parameter carries at most one constraint
([generic functions](modules-and-functions.md#generic-functions)). In a
multi-line parameter list a line break separates parameters as a comma does.
A `test` takes a string title and a body ([tests](tests.md)).

## Types

```ebnf
type             = base_type, "?"? ;
type_annotation  = ":", base_type ;
local_annotation = ":", type ;

base_type       = scalar_type
                | identifier
                | identity_type
                | generic_type ;

scalar_type     = "int" | "bool" | "string" | "bytes"
                | "date" | "instant" | "duration" ;

identity_type   = "Id", "(", durable_root, ")" ;
generic_type    = identifier, "<", base_type, {",", base_type}, ">" ;
```

A bare `identifier` names a resource, struct, enum, alias, nominal type, or
type parameter. `generic_type` applies `Option`, `Result`, `List`, `Map`, or a
generic struct or enum to its arguments; arity is checked after parsing. In an
expression `<` and `>` are comparison operators; a type-argument list appears
only in a type position. The `?` suffix composes after the close:
`Option<string>?`.

## Statements

```ebnf
statement       = const_stmt
                | var_stmt
                | assignment_stmt
                | compound_assignment_stmt
                | place_stmt
                | delete_stmt
                | unset_stmt
                | if_stmt
                | while_stmt
                | for_stmt
                | match_stmt
                | checked_stmt
                | require_stmt
                | assert_stmt
                | transaction_stmt
                | break_stmt
                | continue_stmt
                | return_stmt
                | expression_stmt ;

const_stmt      = "const", identifier, local_annotation?, "=", value, let_else? ;
var_stmt        = "var", identifier, local_annotation?, "=", value, let_else? ;
let_else        = "else", clause_body ;

value           = "try"?, expression ;
clause_body     = block | statement ;

assignable      = identifier | path_expr ;
assignment_stmt = assignable, "=", expression ;
compound_assignment_stmt =
                  assignable, ("+=" | "-=" | "*=" | "/=" | "%="), expression ;

place_stmt      = "place", identifier, "=", expression ;
delete_stmt     = "delete", path_expr ;
unset_stmt      = "unset", path_expr ;

require_stmt    = "require", expression, "else", expression ;
assert_stmt     = "assert", expression ;
transaction_stmt = "transaction", block ;

break_stmt      = "break" ;
continue_stmt   = "continue" ;
return_stmt     = "return", value? ;
expression_stmt = value ;
```

Prefix `try` is a statement-level value: it stands at the top of a `const`,
`var`, `return`, or expression statement
([prefix try](control-flow.md#prefix-try)). A `let_else` tail runs when the
bound value is absent ([let-else bindings](control-flow.md#let-else-bindings)).
A `clause_body` written as one statement parses; the formatter writes it as a
block. `require` takes a condition and a bare failure value
([require guards](control-flow.md#require-guards)). `assert` is legal inside a
`test` body. `delete` clears a durable place and `unset` a local field:
`unset ^books[id].isbn` reports `check.type`, and `delete` on a local field
reports `check.unsupported`.

### Conditionals and loops

```ebnf
if_stmt         = "if", if_head, block, {else_if}, else_clause? ;
if_head         = expression
                | const_binding, {"and", const_binding}, ("and", expression)? ;
const_binding   = "const", identifier, local_annotation?, "=", expression ;
else_if         = "else", "if", expression, block ;
else_clause     = "else", clause_body ;

while_stmt      = "while", expression, block ;

for_stmt        = "for", identifier, {",", identifier}, "in",
                  ( expression, ("by", expression)?, block
                  | expression, "at", "most", expression,
                    ("from", expression)?, block, "on", "more", clause_body ) ;
```

A trailing clause cuddles the closing brace before it: `} else {`,
`} else if c {`, `} on more {`. An `if const` head chains bindings with `and`
and may end with a condition. `by` steps a range. `at most`, `from`, and
`on more` belong to a bounded durable traversal
([bounded durable traversal](traversal-and-indexes.md#bounded-durable-traversal));
the words `by`, `at`, `most`, `from`, `on`, and `more` are contextual and stay
ordinary names elsewhere.

### Match and checked arithmetic

```ebnf
match_stmt      = "match", expression, "{", NEWLINE, (match_arm, NEWLINE)+, "}" ;
match_arm       = identifier, arm_bindings?, "=>", clause_body ;
arm_bindings    = "(", identifier, {",", identifier}, ")" ;

checked_stmt    = checked_bind, "checked", expression, NEWLINE, checked_arm,
                  {checked_arm} ;
checked_bind    = "return"
                | ("const" | "var"), identifier, local_annotation?, "=" ;
checked_arm     = "on", ("out_of_range" | "zero_divisor"), clause_body ;
```

A match arm names a member of the matched enum and binds its payload
positionally ([match](control-flow.md#match)). A `checked` form wraps one
arithmetic operation; its first `on` arm starts a new line and later arms
cuddle the brace before them
([checked arithmetic](control-flow.md#checked-arithmetic)).

## Expressions

```ebnf
expression      = or_expr ;

or_expr         = and_expr, {"or", and_expr} ;
and_expr        = equality_expr, {"and", equality_expr} ;

equality_expr   = comparison_expr, (("==" | "!="), comparison_expr)? ;

comparison_expr = range_expr,
                  ( ("<" | "<=" | ">" | ">="), range_expr
                  | ("in" | "not", "in"), range_expr )? ;

range_expr      = coalesce_expr,
                  ((".." | "..="), coalesce_expr, ("by", coalesce_expr)?)? ;

coalesce_expr   = additive_expr, ("??", coalesce_expr)? ;
additive_expr   = multiplicative_expr, {("+" | "-"), multiplicative_expr} ;
multiplicative_expr =
                  unary_expr, {("*" | "/" | "%"), unary_expr} ;

unary_expr      = ("-" | "not"), unary_expr
                | postfix_expr ;

postfix_expr    = primary_expr, {postfix} ;
postfix         = "(", argument_list?, ")"
                | "[", expression, {",", expression}, ","?, "]"
                | ".", identifier
                | "?.", identifier ;

argument_list   = argument, {",", argument}, ","? ;
argument        = (identifier, ":")?, expression ;
```

A comparison is single and non-associative: `a < b > c` is a parse error.
Membership (`x in lo..hi`, `x not in lo..hi`) sits at the same level with a
range on its right and shares the rule, so `a in r in s` is a parse error.
`??` is right-associative. A range names both ends.

Parentheses call or construct. Brackets select a durable entry by its keys, a
list position, or a map key. A named argument (`title: t`) belongs to a
constructor, and after a named
argument every later argument is named. Precedence and operand types are under
[operators](types-and-values.md#operators).

### Primary expressions and paths

```ebnf
primary_expr    = literal
                | "true" | "false" | "absent"
                | qualified_name
                | durable_root
                | constructor_call
                | identity_value
                | interp_lit
                | "(", expression, ")" ;

literal         = integer_lit | string_lit | duration_words ;

constructor_call = ("string" | "bytes" | "date" | "instant" | "duration"),
                   "(", argument_list?, ")" ;

identity_value  = "Id", "(", durable_root, {",", expression}, ")" ;

path_expr       = (durable_root | identifier), {path_suffix} ;
path_suffix     = "[", expression, {",", expression}, "]"
                | ".", identifier ;
```

A `qualified_name` is a local name, a function path (`shelf::books::add`), or
an enum member (`Color::red`); a constructor is a name followed by a
parenthesized argument list. `^books[id].title` is a durable path and
`book.title` a local one. `Id(^books)` in a type position is an identity type;
`Id(^books, id)` in an expression is an identity value
([entry identity](types-and-values.md#entry-identity)).

## Lexical tokens

```ebnf
identifier      = (letter | "_"), {letter | digit | "_"} ;

integer_lit     = digit, {digit} ;
duration_words  = integer_lit, duration_unit ;
duration_unit   = "second" | "seconds" | "minute" | "minutes"
                | "hour" | "hours" | "day" | "days"
                | "week" | "weeks" ;

string_lit      = '"', {string_char}, '"' ;
string_char     = string_text | string_escape ;
string_escape   = "\", ('"' | "\" | "n" | "r" | "t")
                | "\u{", hex_digit, {hex_digit}, "}" ;

interp_lit      = '$"', {interp_part}, '"' ;
interp_part     = interp_text | string_escape | "{{" | "}}"
                | "{", expression, "}" ;

comment         = "//", {not_newline} ;
doc_comment     = "///", {not_newline} ;

letter          = "A"…"Z" | "a"…"z" ;
digit           = "0"…"9" ;
hex_digit       = digit | "A"…"F" | "a"…"f" ;
```

`string_text` excludes `"`, `\`, and a line break; `interp_text` also excludes
a bare `{`. A `duration_unit` is read as a unit only directly after an integer
literal, so `const seconds = 5` is an ordinary name. Escape rules and literal
values are under [literals](source-and-syntax.md). Reserved words are listed in
[AI legibility](../tools/ai-legibility.md).
