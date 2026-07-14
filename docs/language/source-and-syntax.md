# Source And Syntax

Marrow source files are UTF-8 text with the `.mw` extension. Indentation forms
blocks. A tab in indentation is an error; spaces must be used consistently
within a block.

## Files And Modules

A module source begins with one `module` declaration. The module path uses
`::`-separated ASCII identifiers and normally matches the source path beneath a
configured source root. A moduleless source file is accepted as a script by
commands that provide script execution.

```mw
module docs::source

const greeting = "hello"

fn label(value: int): string
    return $"{greeting} {value}"

pub fn main()
    var count: int = 1
    if count == 1
        print(label(count))
```

A line beginning with `;` is a comment. `;;` begins a documentation comment for
the following declaration. Comments continue to the end of the line.

## Identifiers And Keywords

Identifiers begin with an ASCII letter or `_` and continue with ASCII letters,
digits, or `_`. Names are case-sensitive. The current language reference uses
these reserved or contextual words:

```text
module use pub const var fn return resource required store index unique
enum category match is evolve rename default retire transform
if else while for in reversed by
break continue transaction try catch throw delete
and or not true false absent
int bool string bytes decimal date instant duration sequence unknown Error ErrorCode Id
```

`surface` begins a reachable legacy declaration and is intentionally outside
the main reference. `merge` and `lock` are reserved statement heads that produce
parser diagnostics. `journal`, `sensitive`, and `declassify` are held keywords,
not current statement forms. These spellings are unavailable as ordinary
identifiers.

Standard-library path segments such as `std::text` are resolved as declared
library names even when a segment resembles a type word.

## Layout

A header ending at the line break introduces a block when the following line is
more deeply indented. Sibling statements use the same indentation. Blank lines
and comment-only lines do not close a block.

```text
if condition
    first()
    if nested
        second()
    third()
else
    fourth()
```

There are no braces or statement terminators. A trailing comma is allowed in
multiline argument lists and constructors.

## Declarations

The top-level declaration forms are:

```text
module path
use path
const name [: Type] = expression
[pub] fn name(parameters) [: Type]
resource Name
store ^name: Resource
store ^name(keys): Resource
[pub] enum Name
evolve ...
```

See [Modules and functions](modules-and-functions.md),
[Resources](resources.md), [Durable places](durable-places.md), and
[Evolution](evolution.md) for their semantic rules.

## Bindings And Assignment

`const` creates a binding that cannot be reassigned. `var` creates a mutable
binding. A top-level `const` is evaluated as a compile-time constant; a local
binding is evaluated when control reaches it.

```mw
module docs::bindings

pub fn total(base: int): int
    const increment = 2
    var result: int = base
    result += increment
    return result
```

Assignment is a statement, not an expression. The simple and compound forms are:

```text
place = expression
place += expression
place -= expression
place *= expression
place /= expression
place %= expression
```

Equality uses `==`; `=` is never equality. Only `var` bindings and assignable
member or collection places may appear on the left of assignment.

## Literals

| Kind | Examples | Notes |
|---|---|---|
| Integer | `0`, `-12`, `1000` | Signed decimal integer |
| Decimal | `12.50`, `-0.25` | Fixed decimal value |
| Boolean | `true`, `false` | |
| String | `"text"`, `"line\n"` | UTF-8 text with escapes |
| Interpolated string | `$"id: {id}"` | Expressions occur inside `{...}` |
| Bytes | `b"Marrow"`, `b"\x00\xff"` | Byte string |
| Duration | `10.seconds`, `1.day` | Whole-number duration literal |
| Absence | `absent` | The missing case of an optional value |

String escapes are `\\`, `\"`, `\n`, `\r`, and `\t`; other Unicode characters
may appear directly in UTF-8 source. Byte strings accept those five escapes plus
`\xNN`. Date and instant values are obtained with conversions or `std::clock`
functions rather than dedicated source literals.

Duration units are `second`, `minute`, `hour`, `day`, and `week`, with singular
and plural spellings. Months and years are not fixed durations.

## Collection And Resource Construction

Sequences and keyed local collections are introduced by `var` declarations and
populated by assignment or `append`. Resource constructors name the resource
and use named members.

```mw
module docs::literals

resource Point
    required x: int
    required y: int
    label: string

pub fn origin(): Point
    var xs: sequence[int]
    append(xs, 1)
    append(xs, 2)
    append(xs, 3)
    var labels(axis: string): string
    labels("x") = "horizontal"
    labels("y") = "vertical"
    print(count(xs))
    print(count(labels))
    return Point(x: 0, y: 0)
```

Nested keyed layers are declared through resources, not with a deeper literal
type syntax.

## Expressions

Primary expressions include literals, names, paths, calls, constructors, and
parenthesized expressions. Postfix operations include member access, keyed or
positional access, calls, and optional member access.

Operators, from tighter to looser binding, are summarized here:

```text
-value  not value
*  /  %
+  -
optional ?? fallback
..  ..=
<  <=  >  >=
==  !=
is
and
or
```

The optional member operator `?.` propagates absence through a resource member
read. Numeric binary operators require matching numeric types; there are no
implicit numeric conversions. String and temporal operator combinations are
listed in [Types and values](types-and-values.md#operators). Division and
remainder by zero are runtime errors.

## Calls

The common call syntax accepts positional and named arguments. Once a named
argument is used, remaining arguments are named. A trailing comma is accepted.
Which form has meaning depends on the callee: project functions match argument
labels to parameter names, resource and `Error` constructors name fields, and
conversions, `Id`, built-ins, and standard-library operations are called
positionally. The checker does not yet reject labels consistently on intrinsic
and local-collection calls. A mislabeled call can check without producing an
executable function body, or fail only when evaluated. Do not use labels for a
positional-only callee.

```text
record(title: "Draft", priority: 2)
std::text::contains("Draft", "aft")
```

Function declarations and call rules are defined in
[Modules and functions](modules-and-functions.md).

## Paths

Local paths begin with a binding. Durable paths begin with `^` and a store name.
The same member and key syntax extends either form:

```text
book.title
booksByName("Marrow").author
^books(id).title
^books(id).notes(noteId).text
```

Key arguments on local collections, durable layers, and indexes are
positional.

An `Id(^books)` may be placed directly in `^books(id)`; explicit raw key
arguments are also accepted at the store boundary. Path presence and write
behavior are defined in [Durable places](durable-places.md).

## Statements

The statement grammar includes:

```text
const and var bindings
assignment and compound assignment
expression statements
if, if const, and else
while and for
match
break and continue
return
delete
transaction
try, catch, and throw
```

The control statements are described in [Control flow](control-flow.md) and
[Errors and transactions](errors-and-transactions.md).
