# Formal Grammar

This appendix gives an EBNF-style grammar for accepted v0.1 Marrow `.mw`
source.

The grammar uses these conventions:

- quoted text such as `"if"` is a token,
- `A?` means optional,
- `A*` means zero or more,
- `A+` means one or more,
- `A | B` means either alternative,
- `INDENT`, `DEDENT`, and `NEWLINE` are produced by indentation lexing.

## Lexical Tokens

```ebnf
identifier      = (letter | "_") (letter | digit | "_")* ;
qualified_name  = identifier ("::" identifier)* ;

integer_lit     = digit+ ;
decimal_lit     = digit+ "." digit+ ;
duration_lit    = digit+ "." duration_unit ;
duration_unit   = "second" | "seconds" | "minute" | "minutes"
                | "hour" | "hours" | "day" | "days" | "week" | "weeks" ;
string_lit      = "\"" string_char* "\"" ;
interp_lit      = "$\"" interp_part* "\"" ;
bytes_lit       = "b\"" byte_char* "\"" ;
string_char     = string_text | string_escape ;
string_escape   = "\\" ("\"" | "\\" | "n" | "r" | "t") ;
interp_part     = interp_text | interp_expr ;
interp_text     = (interp_text_char | string_escape | "{{" | "}}")+ ;
interp_expr     = "{" expression "}" ;
byte_char       = byte_text | byte_escape ;
byte_escape     = string_escape | "\\x" hex_digit hex_digit ;

comment         = ";" not_newline* ;
doc_comment     = ";;" not_newline* ;

letter          = "A".."Z" | "a".."z" ;
digit           = "0".."9" ;
hex_digit       = digit | "A".."F" | "a".."f" ;
```

`string_text` is any UTF-8 scalar value except `"`, `\`, or newline.
`byte_text` has the same source shape and contributes its UTF-8 bytes.
`interp_text_char` additionally excludes unescaped `{`. In interpolation text,
the lexer recognizes `{{` and `}}` before single-character text, so `}}` is the
escaped spelling for one `}` while a lone `}` remains text.

A `qualified_name` segment may also be a type keyword used as a name — the
`bytes` in `std::bytes::length` or `use std::bytes` — so standard-library
paths spell those names directly.

Type names have one canonical spelling in source.

## Source File

```ebnf
source_file     = module_decl? top_level_decl* EOF ;

module_decl     = "module" qualified_name NEWLINE ;

top_level_decl  =
      use_decl
    | doc_comment* const_decl
    | doc_comment* resource_decl
    | doc_comment* store_decl
    | doc_comment* enum_decl
    | doc_comment* function_decl
    | evolve_decl
    ;

use_decl        = "use" qualified_name NEWLINE ;
const_decl      = "const" identifier type_annotation? "=" expression NEWLINE ;
```

Module declarations are optional only for single-file scripts and entrypoints.
Importable project files declare a module name, and the module name matches
the source-root-relative file path.

## Resources

```ebnf
resource_decl   =
    "resource" identifier NEWLINE
    INDENT resource_member+ DEDENT ;

store_decl      =
    "store" saved_root key_params? ":" identifier NEWLINE
    (INDENT store_member+ DEDENT)? ;

saved_root      = "^" identifier ;

resource_member =
      doc_comment* field_decl
    | doc_comment* keyed_field_decl
    | doc_comment* group_decl
    ;

store_member =
      doc_comment* index_decl
    ;

field_decl      =
      required_marker? identifier type_annotation NEWLINE
    ;
keyed_field_decl =
    identifier key_params type_annotation NEWLINE ;
required_marker = "required" ;

group_decl      =
    identifier group_keying? NEWLINE
    INDENT resource_member+ DEDENT ;

group_keying    = key_params ;

Keyed layer members are `keyed_field_decl` and keyed `group_decl`. The reserved
post-name `retain` clause position belongs on keyed layer members, after future
keyed-layer `unique` or `counted` clauses if those clauses exist. v0.1 has no
`retain`, `unique`, or `counted` keyed-layer clause and rejects those spellings
there. A future declaration over populated data is a destructive decision;
ongoing bounding of future writes is write-plan maintenance.

index_decl      =
    "index" identifier "(" index_arg_list ")" unique_marker? NEWLINE ;

unique_marker   = "unique" ;

key_params      = "(" key_decl ("," key_decl)* ","? ")" ;
key_decl        = identifier type_annotation ;
index_arg_list  = index_arg ("," index_arg)* ","? ;
index_arg       = field_path ;
field_path      = identifier ("." identifier)* ;
```

## Enums

```ebnf
enum_decl       =
    visibility? "enum" identifier NEWLINE
    INDENT enum_member+ DEDENT ;

enum_member     =
    doc_comment* "category"? identifier NEWLINE
    (INDENT enum_member+ DEDENT)? ;
```

A member is a bare name; it takes no type or key parameters. Members may nest:
the indented block beneath a member is its nested members, so a member path
`Enum "::" member ("::" member)*` walks the tree. The optional `category` lead
marks a member a grouping node, not selectable as a value. A member is a category
exactly when it has nested members: a category must have nested members, and a
member with nested members must be a category. `category` is contextual —
recognized only as the lead of an enum-member line — so it is a valid identifier
elsewhere. A member reference walks the member path after the enum:
`Enum "::" member ("::" member)*` resolves
nominally to the enclosing module's enum, and the qualified `module "::" Enum "::"
member ...` names another module's enum exactly (see the `qualified_name` rule
under Primary Expressions). A bare `Enum "::" leaf` resolves only when that leaf
name is unique in the enum; a name shared by several parents must be written as its
full path.

## Functions

```ebnf
function_decl   =
    visibility?
    "fn"
    identifier
    "(" param_list? ")"
    return_type?
    NEWLINE
    block ;

visibility      = "pub" ;

param_list      = param_decl ("," param_decl)* ","? ;
param_decl      = doc_comment* identifier type_annotation ;

return_type     = ":" type ;

block           = INDENT statement+ DEDENT ;
```

In a multi-line parameter list, a line break separates parameters the same way
a comma does, and a run of `;;` documentation comments directly above a
parameter documents that parameter.

## Evolution

```ebnf
evolve_decl     =
    "evolve" NEWLINE
    INDENT evolve_step+ DEDENT ;

evolve_step     =
      "rename" evolve_target "->" evolve_target NEWLINE
    | "default" evolve_target "=" expression NEWLINE
    | "retire" evolve_target NEWLINE
    | "transform" evolve_target NEWLINE block
    ;

evolve_target   = saved_path | qualified_name | local_path ;
```

An `evolve` block declares durable intent about catalog-addressable entities: a
resource member, a saved root, a store index, an enum, or an enum member. Each
target is written in the same surface form the language already uses to reference
that entity (`Book.title`, `^books`, `^books.byTitle`, `Status::archived`).

`rename`, `default`, `retire`, and `transform` are contextual: they are step lead
words recognized only inside an `evolve` block, so they remain valid identifiers
elsewhere. `evolve` itself is reserved.

## Types

```ebnf
type_annotation = ":" type ;

type            =
      qualified_name
    | scalar_type
    | identity_type
    | sequence_type
    ;

scalar_type     =
      "int"
    | "decimal"
    | "bool"
    | "string"
    | "bytes"
    | "date"
    | "instant"
    | "duration"
    | "ErrorCode"
    | "unknown"
    ;

sequence_type   = "sequence" "[" type "]" ;
identity_type   = "Id" "(" saved_root ")" ;
```

Keyed tree shapes are not written as type annotations; they arise from
`key_params` on the declaration itself — a keyed `var`, field, group, or
store root.

`qualified_name` includes normal imported types. Store identity types use the
source form `Id(^root)`, for example `Id(^books)`. In expression position,
`Id(^root, key...)` is the explicit identity constructor. `Error` is the builtin
resource-shaped error type.

The checker restricts where some parsed types are valid. A missing return type
means the function produces no value. Managed saved fields and keys reject
`unknown`; use `bytes`, `string`, or an explicit resource shape for persisted
dynamic payloads.

## Statements

```ebnf
statement       =
      const_stmt
    | var_stmt
    | assignment_stmt
    | delete_stmt
    | if_stmt
    | if_const_stmt
    | match_stmt
    | while_stmt
    | for_stmt
    | break_stmt
    | continue_stmt
    | return_stmt
    | transaction_stmt
    | try_stmt
    | throw_stmt
    | expression_stmt
    ;

const_stmt      =
    "const" identifier type_annotation? "=" expression NEWLINE ;
var_stmt        =
    "var" identifier key_params? type_annotation? ("=" expression)? NEWLINE ;

assignment_stmt = assignable "=" expression NEWLINE ;
delete_stmt     = "delete" path_expr NEWLINE ;
return_stmt     = "return" expression? NEWLINE ;
break_stmt      = "break" NEWLINE ;
continue_stmt   = "continue" NEWLINE ;

throw_stmt      = "throw" expression NEWLINE ;
expression_stmt = expression NEWLINE ;
```

Field and path assignments preserve omitted fields and children at the written
entry. `if exists(place)` narrows a maybe-present read inside the guarded
block.

## Conditionals And Loops

```ebnf
if_stmt         =
    "if" expression NEWLINE block
    else_if_clause*
    else_clause? ;

if_const_stmt   =
    "if" "const" identifier "=" expression NEWLINE block
    else_if_clause*
    else_clause? ;

else_if_clause  = "else" "if" expression NEWLINE block ;
else_clause     = "else" NEWLINE block ;

while_stmt      = "while" expression NEWLINE block ;

for_stmt        =
    "for" for_binding "in" expression ("by" expression)? NEWLINE block ;

for_binding     = identifier | identifier "," identifier ;

match_stmt      = "match" expression NEWLINE INDENT match_arm+ DEDENT ;
match_arm       = identifier ("::" identifier)* NEWLINE block ;
```

The `by` step is valid only on a range iterable (`lo..hi` or `lo..=hi`); the checker
rejects it on any other iterable. `by` is contextual — recognized only in this
position, so a name `by` elsewhere is unaffected.

A `match` dispatches on an enum value. Each arm is a member path relative to the
scrutinee enum (the scrutinee supplies the enum, so an arm is `archived` or
`tiger::bengal`, not `Status::archived`). For a nested enum an arm may be a
qualified path to one leaf or a category to cover its whole subtree; a bare arm
name must be unambiguous, else it is qualified. The checker requires the arms to
cover every selectable leaf exactly once; there is no wildcard arm. See
[Enums](enums.md).

## Transactions And Try/Catch

```ebnf
transaction_stmt = "transaction" NEWLINE block ;

try_stmt         =
    "try" NEWLINE block
    catch_clause ;

catch_clause     =
    "catch" identifier type_annotation? NEWLINE block ;
```

## Expressions

Assignment is not an expression. Equality is `==` and inequality is `!=`; the
single `=` is assignment only and is a parse error in expression position. The
absence-default `??` and the optional read `?.` apply to possibly-absent path
reads.

```ebnf
expression      = or_expr ;

or_expr         = and_expr ("or" and_expr)* ;
and_expr        = is_expr ("and" is_expr)* ;

is_expr         = equality_expr ("is" equality_expr)? ;

equality_expr   =
    comparison_expr (("==" | "!=") comparison_expr)? ;

comparison_expr = range_expr (("<" | "<=" | ">" | ">=") range_expr)? ;
range_expr      =
      coalesce_expr range_tail?
    | open_range
    ;
range_tail      = ".." coalesce_expr? | "..=" coalesce_expr ;
open_range      = ".." coalesce_expr | "..=" coalesce_expr ;
coalesce_expr   = additive_expr ("??" additive_expr)? ;
additive_expr   = multiplicative_expr (("+" | "-") multiplicative_expr)* ;
multiplicative_expr =
    unary_expr (("*" | "/" | "%") unary_expr)* ;

unary_expr      = ("-" | "not") unary_expr | postfix_expr ;

postfix_expr    =
    primary_expr postfix_op* ;

postfix_op      =
      paren_suffix
    | field_suffix
    | optional_field_suffix
    ;

paren_suffix    = "(" argument_list? ")" ;
field_suffix    = "." field_name ;
optional_field_suffix = "?." field_name ;

field_name      = identifier ;
```

Open and half-open range forms are parsed here because saved-key traversal uses
`start..end`, `start..=end`, `start..`, `..end`, and `..=end` as bounded key
arguments. General range enumeration still requires both endpoints (`lo..hi` or
`lo..=hi`); the checker rejects a bare `..`, a missing upper bound for `..=`
such as `start..=`, and `by` steps inside saved-key arguments.

`??` is deliberately non-associative: `a ?? b ?? c` is rejected. Layer defaults
one read at a time, using parentheses or local bindings when a later extension
needs nested defaults. It binds looser than additive expressions and tighter than
ranges and comparisons: `count ?? 0 < 5` is `(count ?? 0) < 5`,
`start ?? 1 .. n` is `(start ?? 1) .. n`, and `x ?? y + 1` is `x ?? (y + 1)`.
Its left operand must be a maybe-present read — a path read (including a keyed
child such as `^patients(id).visits(date)`), a `?.` chain, or a maybe-present
builtin result such as `next`/`prev`; that constraint is enforced by the
checker, not the grammar.

`is` is the enum-subtree test: `value is Enum::member` is `true` when the value is
at or under that member, exact for a concrete leaf. It is a reserved word, sits
between `and` and `==`, and is non-associative (`a is X is Y` is rejected). The
right operand is a member path of the same enum (a full path reaches a duplicated
leaf, a bare name must be unambiguous); that constraint is enforced by the checker.
See [Enums](enums.md).

## Primary Expressions

```ebnf
primary_expr    =
      literal
    | "true"
    | "false"
    | identifier
    | qualified_name
    | saved_path
    | conversion_call
    | resource_literal
    | "(" expression ")"
    ;

literal         =
      integer_lit
    | decimal_lit
    | duration_lit
    | string_lit
    | interp_lit
    | bytes_lit
    ;

conversion_call =
    conversion_type "(" argument_list? ")" ;

conversion_type  =
      "int"
    | "decimal"
    | "bool"
    | "string"
    | "bytes"
    | "date"
    | "instant"
    | "duration"
    | "ErrorCode"
    ;

resource_literal =
    resource_constructor "(" named_argument_list? ")" ;

resource_constructor =
      qualified_name
    | "Error"
    ;
```

## Paths

```ebnf
path_expr       = saved_path | local_path ;
saved_path      = "^" identifier path_suffix* ;
local_path      = identifier path_suffix+ ;
path_suffix     = paren_suffix | field_suffix ;

assignable      = path_expr | identifier ;
```

`book.title` is local data parsed through postfix field access.
`^books(id).title` is saved data.

## Arguments

```ebnf
argument_list       = argument ("," argument)* ","? ;
named_argument_list = named_argument ("," named_argument)* ","? ;

argument            = expression | named_argument ;

named_argument      = identifier ":" expression ;
```

After the first named argument, remaining arguments must be named.

## Ambiguity Rules

These rules are part of the grammar contract:

- At statement start, `target = expr` is assignment; the single `=` is always
  assignment and never equality, so a `=` in expression position is a parse
  error. Equality is `==` and inequality is `!=`.
- `if const name = place` is a presence-binding guard. The right side must be a
  saved value read, such as a saved field, singleton root, fully addressed
  record or keyed-layer entry, or complete unique-index lookup. It does not
  bind address-only durable collections.
- The absence-default `??` is non-associative and binds looser than additive
  expressions and tighter than ranges and comparisons. Its left operand must be
  a maybe-present read — a path read, a `?.` chain, or a maybe-present builtin
  result such as `next`/`prev`; an always-present left operand is rejected as an
  operator misuse.
- The optional read `?.` is a postfix field access that short-circuits the chain
  to absent when a step is absent; only absence is short-circuited, not schema or
  decoding errors.
- Assignment cannot be nested inside calls, conditions, returns, or subscripts.
- An expression statement may be any expression except a bare range; a range
  is valid only as a `for` iterable, so a range in statement position is
  rejected.
- Conversion calls use supported scalar type keywords in expression position.
  They take one positional argument. A bare type spelling with no call, such as
  `const Bad = int`, is a parse error: a type keyword is not an expression.
- Reserved words are not identifiers, so a reserved word cannot be used as a
  name for a binding, parameter, resource, field, function, or module segment.
- Store identity values are typed as `Id(^root)`.
- A bare `Id(^root, key...)` call is checked as an identity constructor. It is
  not a resource constructor and does not perform a saved read.
- Calls to resource types and `Error(...)` are checked as resource constructors;
  calls to functions are checked as calls.
- `throw` requires an `Error` value.
- `catch name` binds `name` as `Error`; if a catch annotation is present, it
  must be `Error`.
- `index` declarations are checked as direct members of keyed stores.
- Parenthesized suffixes are calls on callable values and key lookups on tree
  values; the checker resolves the value kind.
- Direct durable collection iteration yields addresses. For a managed store root,
  that means store identities; for a sequence or keyed layer, that means child
  keys; for a non-unique index branch, that means the identities in the branch.
- `keys` and `values` expose address-only and element-only traversal forms.
  `entries(...)` is only valid as a two-name loop-head form, including
  `reversed(entries(...))` in that same position.
- Documentation comments attach to the next const, resource, store, enum, or
  function declaration, or to the next resource/store element, enum member, or
  parameter.

`~` is reserved for future typed ephemeral roots. v0.1 rejects `~` everywhere
in source: root forms such as `~scratch`, identity types such as
`Id(~scratch)`, and compound root sigils such as `^~` and `~^`.
