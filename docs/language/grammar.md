# Formal Grammar

This appendix gives an EBNF-style grammar for Marrow `.mw`. It describes the
source language, not the implementation parser.

The grammar uses these conventions:

- quoted text such as `"if"` is a token,
- `A?` means optional,
- `A*` means zero or more,
- `A+` means one or more,
- `A | B` means either alternative,
- `INDENT`, `DEDENT`, and `NEWLINE` are produced by indentation lexing.

## Lexical Tokens

```ebnf
identifier      = letter (letter | digit | "_")* ;
qualified_name  = identifier ("::" identifier)* ;

integer_lit     = digit+ ;
decimal_lit     = digit+ "." digit+ ;
string_lit      = "\"" string_char* "\"" ;
interp_lit      = "$\"" interp_part* "\"" ;
bytes_lit       = "b\"" byte_char* "\"" ;

comment         = ";" not_newline* ;
doc_comment     = ";;" not_newline* ;

letter          = "A".."Z" | "a".."z" ;
digit           = "0".."9" ;
```

Type names have one canonical spelling in source.

## Source File

```ebnf
source_file     = module_decl? top_level_decl* EOF ;

module_decl     = "module" qualified_name NEWLINE ;

top_level_decl  =
      use_decl
    | doc_comment* const_decl
    | doc_comment* resource_decl
    | doc_comment* function_decl
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
    "resource" identifier resource_store? NEWLINE
    INDENT resource_member+ DEDENT ;

resource_store  = "at" saved_root key_params? ;
saved_root      = "^" identifier ;

resource_member =
      doc_comment* stable_id? field_decl
    | doc_comment* stable_id? keyed_field_decl
    | doc_comment* stable_id? group_decl
    | doc_comment* stable_id? index_decl
    ;

stable_id       = "@id" "(" string_lit ")" NEWLINE ;

field_decl      = required_marker? identifier type_annotation NEWLINE ;
keyed_field_decl =
    identifier key_params type_annotation NEWLINE ;
required_marker = "required" ;

group_decl      =
    identifier group_keying? NEWLINE
    INDENT resource_member+ DEDENT ;

group_keying    = key_params ;

index_decl      =
    "index" identifier "(" index_arg_list ")" unique_marker? NEWLINE ;

unique_marker   = "unique" ;

key_params      = "(" key_decl ("," key_decl)* ","? ")" ;
key_decl        = identifier type_annotation ;
index_arg_list  = index_arg ("," index_arg)* ","? ;
index_arg       = field_path ;
field_path      = identifier ("." identifier)* ;
```

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
param_decl      = param_mode? identifier type_annotation ;
param_mode      = "out" | "inout" ;

return_type     = ":" type ;

block           = INDENT statement+ DEDENT ;
```

## Types

```ebnf
type_annotation = ":" type ;

type            =
      qualified_name
    | scalar_type
    | sequence_type
    | keyed_tree_type
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
keyed_tree_type = "(" key_decl ("," key_decl)* ","? ")" ":" type ;
```

`qualified_name` includes normal imported types and generated resource identity
types such as `Book::Id`. `Error` is the builtin resource-shaped error type.

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
    | merge_stmt
    | if_stmt
    | while_stmt
    | for_stmt
    | break_stmt
    | continue_stmt
    | return_stmt
    | transaction_stmt
    | lock_stmt
    | try_stmt
    | throw_stmt
    | expression_stmt
    ;

const_stmt      = "const" identifier type_annotation? "=" expression NEWLINE ;
var_stmt        =
    "var" identifier key_params? type_annotation? ("=" expression)? NEWLINE ;

assignment_stmt = assignable "=" expression NEWLINE ;
delete_stmt     = "delete" path_expr NEWLINE ;
merge_stmt      = "merge" assignable "=" expression NEWLINE ;

return_stmt     = "return" expression? NEWLINE ;
break_stmt      = "break" identifier? NEWLINE ;
continue_stmt   = "continue" identifier? NEWLINE ;

throw_stmt      = "throw" expression NEWLINE ;
expression_stmt = expression NEWLINE ;
```

## Conditionals And Loops

```ebnf
if_stmt         =
    "if" expression NEWLINE block
    else_if_clause*
    else_clause? ;

else_if_clause  = "else" "if" expression NEWLINE block ;
else_clause     = "else" NEWLINE block ;

while_stmt      = loop_label? "while" expression NEWLINE block ;

for_stmt        =
    loop_label?
    "for" for_binding "in" expression NEWLINE block ;

loop_label      = identifier ":" ;
for_binding     = identifier | identifier "," identifier ;
```

## Transactions, Locks, Try/Catch

```ebnf
transaction_stmt = "transaction" NEWLINE block ;
lock_stmt        = "lock" saved_path NEWLINE block ;

try_stmt         =
    "try" NEWLINE block
    (catch_clause finally_clause? | finally_clause) ;

catch_clause     =
    "catch" identifier type_annotation? NEWLINE block ;

finally_clause   = "finally" NEWLINE block ;
```

## Expressions

Assignment is not an expression. `=` in expression position is equality.

```ebnf
expression      = or_expr ;

or_expr         = and_expr ("or" and_expr)* ;
and_expr        = equality_expr ("and" equality_expr)* ;

equality_expr   =
    comparison_expr (("=" | "!=") comparison_expr)? ;

comparison_expr = range_expr (("<" | "<=" | ">" | ">=") range_expr)? ;
range_expr      = concat_expr ((".." | "..=") concat_expr)? ;
concat_expr     = additive_expr ("_" additive_expr)* ;
additive_expr   = multiplicative_expr (("+" | "-") multiplicative_expr)* ;
multiplicative_expr =
    unary_expr (("*" | "/" | "%") unary_expr)* ;

unary_expr      = ("-" | "not") unary_expr | postfix_expr ;

postfix_expr    =
    primary_expr postfix_op* ;

postfix_op      =
      paren_suffix
    | field_suffix
    ;

paren_suffix    = "(" argument_list? ")" ;
field_suffix    = "." field_name ;

field_name      = identifier | string_lit ;
```

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

argument            =
      arg_mode? expression
    | named_argument
    ;

named_argument      = identifier ":" expression ;
arg_mode            = "out" | "inout" ;
```

After the first named argument, remaining arguments must be named.

## Ambiguity Rules

These rules are part of the grammar contract:

- At statement start, `target = expr` is assignment.
- Inside expressions, `=` is equality.
- Assignment cannot be nested inside calls, conditions, returns, or subscripts.
- Expression statements must be effectful calls or call-shaped builtins such
  as `write(...)` and `print(...)`; useless pure expression statements are
  rejected.
- Conversion calls use supported scalar type keywords in expression position.
  They take one positional argument.
- Calls to generated resource identity types, such as `Book::Id(...)`, are
  checked as identity constructors.
- Calls to resource types and `Error(...)` are checked as resource
  constructors; calls to functions are checked as calls.
- `throw` requires an `Error` value.
- `catch name` binds `name` as `Error`; if a catch annotation is present, it
  must be `Error`.
- `finally` blocks reject `return`, `break`, and `continue`.
- `index` declarations are checked as direct members of keyed saved resources.
- Parenthesized suffixes are calls on callable values and key lookups on tree
  values; the checker resolves the value kind.
- `out` and `inout` arguments must be assignable places.
- Direct tree iteration `for id in ^books` yields keys from the next declared
  layer; for a managed resource root, that means resource identities.
- `keys`, `values`, and `entries` make traversal intent explicit.
- Documentation comments attach to the next const, resource, function, or
  resource element at the same indentation level.
- `@id(...)` attaches to the next resource element at the same indentation
  level.
