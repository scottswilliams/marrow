# Source And Syntax

Marrow source files are UTF-8 text with the `.mw` extension. Curly braces `{`
and `}` delimit blocks and a line break terminates a statement. A tab in leading
whitespace is an error; the formatter emits four-space indentation, which carries
no meaning of its own.

## Files And Modules

A module source begins with one `module` declaration. The module path uses
`::`-separated ASCII identifiers and normally matches the source path beneath a
configured source root. A moduleless source file is accepted as a script by
commands that provide script execution.

```mw
module docs::source

const greeting = "hello"

fn label(value: int): string {
    return $"{greeting} {value}"
}

pub fn main() {
    var count: int = 1
    if count == 1 {
        print(label(count))
    }
}
```

A line beginning with `//` is a comment. `///` begins a documentation comment for
the following declaration. Comments continue to the end of the line.

## Identifiers And Keywords

Identifiers begin with an ASCII letter or `_` and continue with ASCII letters,
digits, or `_`. Names are case-sensitive. The current language reference uses
these reserved or contextual words:

```text
module use pub const var fn return alias type supports resource struct required store index unique test assert
enum category match is
if else while for in reversed by at most from on more
break continue transaction place checked try catch throw delete unset
and or not true false absent
int bool string bytes decimal date instant duration unknown Error ErrorCode Id
```

`surface` begins a reachable legacy declaration and is intentionally outside
the main reference. `merge` and `lock` are reserved statement heads that produce
parser diagnostics. `journal`, `sensitive`, and `declassify` are held keywords,
not current statement forms; `writes` and `reads` are held for a future
effect-signature clause. These spellings are unavailable as ordinary
identifiers.

Standard-library path segments such as `std::text` are resolved as declared
library names even when a segment resembles a type word.

## Blocks

A header line ends with `{`, and the block it opens closes with `}` (one-true-
brace). Braces are mandatory for every block, including a single-statement body,
and a statement terminates at a line break or the block's closing `}`. There is
no statement separator.

```text
if condition {
    first()
    if nested {
        second()
    }
    third()
} else {
    fourth()
}
```

A trailing clause cuddles the closing brace of the block before it — `} else {`,
`} else if c {`, `} on more { ... }`. A header may continue across a line break
after a trailing `and`, `or`, `,`, or `=`, and continuation is implicit inside an
open `(` or `[`; the header ends at its `{`. A trailing comma is allowed in
multiline argument lists, key groups, and constructors.

## Declarations

The top-level declaration forms are:

```text
module path
use path
const name = expression
const name: Type = expression
fn name(parameters): Type
pub fn name(parameters): Type
resource Name { ... }
store ^name: Resource
store ^name[keys]: Resource
enum Name { ... }
```

See [Modules and functions](modules-and-functions.md),
[Resources](resources.md), and [Durable places](durable-places.md) for their
semantic rules.

## Bindings And Assignment

`const` creates a binding that cannot be reassigned. `var` creates a mutable
binding. A top-level `const` is evaluated as a compile-time constant; a local
binding is evaluated when control reaches it.

```mw
module docs::bindings

pub fn total(base: int): int {
    const increment = 2
    var result: int = base
    result += increment
    return result
}
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
| Duration | `duration("PT600S")` | Canonical text constructor (see [temporal values](types-and-values.md#temporal-values)) |
| Absence | `absent` | The missing case of an optional value |

String escapes are `\\`, `\"`, `\n`, `\r`, `\t`, and `\u{H}`, where `H` is one to
six hexadecimal digits naming a Unicode scalar value (at most `10FFFF` and not a
surrogate); other Unicode characters may appear directly in UTF-8 source. In an
interpolated string the `\u{H}` escape is recognized as text, so its braces do not
open an expression hole, and a doubled `{{` or `}}` is one literal brace. Each
`{...}` hole holds one expression whose value is rendered into the string through
the same canonical conversions `string(...)` provides, so on the current beta line
an interpolable hole is a `string`, `int`, or `bool` value. Byte strings accept the five non-unicode escapes plus
`\xNN`; `\u{H}` is text-only and is not a byte escape. Date and instant values are constructed from one canonical text literal each — `date("YYYY-MM-DD")` and `instant("YYYY-MM-DDTHH:MM:SSZ")`; there is no clock builtin.

Duration units are `second`, `minute`, `hour`, `day`, and `week`, with singular
and plural spellings. Months and years are not fixed durations.

## Collection And Resource Construction

Lists and maps are introduced by `var` declarations, constructed with `List()`
or `Map()`, and grown with `append`; because collections are values, each `append`
yields an updated collection that the binding is reassigned to. A map value is set
with the bracket assignment `m[k] = value` and both are read with bracket lookup
(`xs[i]`, `m[k]`).
Resource constructors name the resource and use named members.

```mw
module docs::literals

resource Point {
    required x: int
    required y: int
    label: string
}

pub fn origin(): Point {
    var xs: List<int> = List()
    xs = append(xs, 1)
    xs = append(xs, 2)
    xs = append(xs, 3)
    return Point(x: 0, y: 0)
}
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

The optional member operator `?.` reads a member through an optional composite
value, yielding `absent` when the value is absent and the member wrapped optional
otherwise. Numeric binary operators require matching numeric types; there are no
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
The same member and key syntax extends either form; keyed access uses square
brackets and member access uses `.`:

```text
book.title
booksByName["Marrow"].author
^books[id].title
^books[id].notes[noteId].text
```

Key arguments on local collections, durable layers, and indexes are positional:
a bracket group holds an ordered tuple of key values and never a named argument.

An `Id(^books)` may be placed directly in `^books[id]`; explicit raw key
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
prefix try in bindings and return
```

The control statements are described in [Control flow](control-flow.md) and
[Errors and transactions](errors-and-transactions.md).
