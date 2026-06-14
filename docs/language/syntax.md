# Syntax

Marrow `.mw` is indentation-delimited. There are no braces and no `end`
markers.

## Source Text

- Source files are UTF-8.
- Identifiers use ASCII letters, digits, and `_`; they must not begin with a
  digit.
- Keywords are lowercase ASCII. Built-in names such as `Error` and
  `ErrorCode` are reserved separately.
- Semicolon starts a comment to end of line.
- `;;` starts a documentation comment for the next declaration, resource or
  store member, enum member, or parameter.
- Tabs are an error in Marrow source. Use spaces.

## Blocks

Blocks are introduced by indentation:

```mw
if status == "open"
    print("open")
else
    print("not open")
```

Blank lines and comments do not close a block. A less-indented statement
closes as many open blocks as needed.

Lines inside open delimiters continue until the delimiter closes:

```mw
throw Error(
    code: "book.absent",
    message: $"Book {id} does not exist.",
)
```

## Declarations

```mw
module shelf::books

const MaxLoans: int = 5

resource Book
    required title: string
    required author: string

store ^books(id: int): Book

pub fn add(title: string): int
    return 1
```

Functions use one `fn` form. Parameters use `name: type`. Omitted return type
means the function produces no value.

## Resource Syntax

Resource indentation mirrors tree layers:

```mw
resource Patient
    required mrn: string
    required lastName: string

    name
        required first: string
        required last: string

    visits(date: date)
        note: string

store ^patients(id: string): Patient
```

Source stable-id annotations are not part of v0.1; durable field identity
belongs in the catalog.

Fields are sparse by default. Add `required` when a resource is invalid
without a populated field:

```mw
required title: string
```

Indexes are declared on stores:

```mw
store ^patients(id: string): Patient
    index byLastName(lastName, id)
    index byMrn(mrn) unique
```

History is modeled as an ordinary keyed child layer:

```mw
versions(version: int)
    title: string
    body: string
```

## Evolution

An `evolve` block declares durable intent about catalog-addressable entities,
which a plain source change never implies:

```mw
evolve
    rename Book.title -> Book.subtitle
    default Book.author = "unknown"
    retire ^books.byTitle
    transform Book.priceCents
        return old.price * 100
```

Each step targets an entity by the same surface form used to reference it: a
resource member (`Book.title`), a saved root (`^books`), a store index
(`^books.byTitle`), or an enum or enum member (`Status::archived`). `rename`
states that the entity now spelled on the right is the one formerly spelled on
the left; `default` gives the value to backfill where a newly populated member is
absent; `retire` states destructive intent to remove an entity; `transform`
computes a saved member per record from the record's other members through a pure
body, where `old` is the record before the evolution. See
[Resources And Saved Data](resources-and-storage.md).

## Statements

Marrow statements are explicit:

```mw
const title: string = "Small Gods"
var loanCount: int = 0
loanCount = loanCount + 1
^books(id).title = title
delete ^books(id).subtitle
var draftBook: Book
return id
print($"created {id}")
```

Assignment is a statement only. It cannot appear as a subexpression, cannot be
chained, and does not return a value.
The right-hand expression is evaluated before the target is changed.

`print(...)` uses call syntax, but it is a statement: it performs output and
produces no value. User-defined functions may still be effectful and return
values.

General statement chaining and postconditionals are not part of Marrow `.mw`.
Use normal `if` blocks.

`if const name = place` is a presence-binding guard for saved value reads:

```mw
if const title = ^books(id).title
    print(title)
else
    print("missing")
```

When the saved place is present, Marrow reads it once, binds the value as an
immutable local `const`, and runs the guarded block. When it is absent, execution
continues through `else if`, `else`, or the following statement. The right side
must be a saved value read, such as a field, singleton root, fully addressed
record or keyed-layer entry, or complete unique-index lookup. Address-only
collections such as bare keyed roots, unaddressed keyed child layers, and
non-unique index branches are not binding guards.

## Bindings

`const` introduces an immutable binding; `var` introduces a mutable one. Scope
decides whether a `const` is a module constant or a local binding.

```mw
const left: int = 1      ; immutable local binding
var right: int = 1       ; mutable local binding
var book: Book           ; mutable local resource, built up field by field
const id = nextId(^books); immutable local value; runtime-computed is fine
const MaxLoans: int = 5  ; module-level constant, evaluated at compile time
```

A module-level `const` must be a compile-time constant expression; a local
`const` may bind any value, including runtime results. A `const` cannot be
reassigned.

## Equality And Assignment

`=` is assignment only. It binds a value to a target in statement position:

```mw
book.title = "Small Gods"
```

Equality is `==` and inequality is `!=`. They read values; they never assign:

```mw
if book.title == "Small Gods"
    print("found")

const same: bool = (left == right)
```

Equality is non-associative. `a == b == c` is rejected; use parentheses if you
need to compare boolean results. A bare `=` in expression position is a parse
error, so a comparison can never be mistaken for an assignment.

## Operators

From tightest to loosest precedence:

| Level | Operator | Meaning |
|---|---|---|
| 1 | calls, key subscripts, dotted fields, `?.` | `f(x)`, `^books(id).title`, `book?.shelf` |
| 2 | unary `-`, `not` | negate, boolean not |
| 3 | `*`, `/`, `%` | multiply, divide, remainder |
| 4 | `+`, `-` | add, subtract |
| 5 | `??` | absence default (non-associative) |
| 6 | `..`, `..=` | exclusive and inclusive ranges |
| 7 | `<`, `<=`, `>`, `>=` | comparison |
| 8 | `==`, `!=` | equality, not equal |
| 9 | `is` | enum-subtree test (non-associative) |
| 10 | `and` | short-circuit and |
| 11 | `or` | short-circuit or |

`%` is remainder. Use `std::math::modulo(...)` when code needs modulo
behavior for negative operands.

Numeric arithmetic operands must have matching numeric types. Numeric `+`, `-`,
and `*` return that type. `/` returns `decimal`. `%` accepts `int` operands and
returns `int`.

`+` also concatenates two strings. Both operands must be `string`; Marrow does
not implicitly convert values to strings for concatenation.

Temporal arithmetic handles linear spans only: `instant - instant` returns
`duration`; `instant + duration` and `instant - duration` return `instant`; and
`duration + duration` and `duration - duration` return `duration`. Calendar math
for dates belongs in named `std::clock` helpers, not operators.

Equality requires comparable values of the same type. Ordering comparisons
require ordered values of the same type.

The absence-default `??` reads a maybe-present operand on its left and yields the
right operand when that read is absent. Its left operand must be a maybe-present
read — a path read, a `?.` chain, or a maybe-present call result such as
`next`/`prev` or a maybe-returning user function — since a value that is always
present has nothing to default; the default must match the read's type. It binds
looser than `+`/`-` and tighter than ranges and comparisons, so `count ?? 0 < 5`
is `(count ?? 0) < 5`,
`start ?? 1 .. n` is `(start ?? 1) .. n`, and `x ?? y + 1` is `x ?? (y + 1)`.
It does not chain: write one `??` per read.

The optional read `?.` accesses a field that may be absent. An absent step
short-circuits the rest of the chain to absent rather than failing the read, so
`^books(id)?.binding?.shelf` is absent when any step along the way is. An
unresolved maybe-present read — including a `?.` chain not terminated by a
resolution — is a compile error, not a runtime fault; resolve it with `??` or
an `if exists(...)` branch or an `if const name = place` binding guard. Only
absence is short-circuited — schema and decoding errors still surface.

Range endpoints must be a steppable type — `int`, `date`, or `instant` — and
both endpoints share that type. The checker accepts ranges for `for` loops, not
as saved values. See
[Control Flow And Errors](control-flow-and-effects.md) for step rules.

Operands and call arguments evaluate left to right. `and` and `or`
short-circuit; other operators evaluate their operands before applying the
operator.

## Strings

Ordinary strings are UTF-8 text:

```mw
const title = "Small Gods"
```

String literals decode `\\`, `\"`, `\n`, `\r`, and `\t`. Other backslash
escapes are rejected at check.

Interpolation is explicit with `$"..."`:

```mw
print($"book {id}: {title}")
```

Inside interpolation strings, text segments decode the same string escapes;
`{{` emits `{` and `}}` emits `}`. A lone `}` is ordinary text. A lone `{`
starts an interpolation expression, so literal `{` text must be written as `{{`.
Interpolation expressions are ordinary expressions and cannot contain statements.

Interpolation formats values as text for that string. It does not create an
implicit conversion for assignment, calls, or saved writes.

Byte literals use `b"..."`:

```mw
const marker: bytes = b"marrow"
```

Byte literals decode `\\`, `\"`, `\n`, `\r`, `\t`, and `\xNN`, where each `N`
is a hex digit. Unescaped non-ASCII text contributes its UTF-8 bytes.

## Duration Literals

A whole number followed by a dot and a unit is a `duration`:

```mw
const window: duration = 2.hours
```

The units are fixed spans, singular or plural: `second`/`seconds`,
`minute`/`minutes`, `hour`/`hours`, `day`/`days`, `week`/`weeks`. A literal is a
fixed elapsed span, so `1.day` is exactly 86400 seconds — the same value as
`std::clock::parseDuration("PT86400S")`. Months and years vary in length and are
not units.

Only a known unit after the dot reads as a duration. `1.5` is a decimal, and
`x.field` is field access; an unknown word such as `1.month` is a number, a dot,
and a name, not a literal.

## Paths And Calls

| Syntax | Meaning |
|---|---|
| `book.title` | local field |
| `books(id)` | local keyed layer |
| `^books(id)` | saved keyed layer |
| `^books(id).title` | saved field |
| `module::name` | code namespace |

Dots are data fields. `::` is for code namespaces. A field segment after `.`
or `?.` is an identifier; a bare keyword after `.` is a parse error.

## Named Arguments And Resource Literals

Function calls may use named arguments:

```mw
saveBook(book: draft, notify: true)
```

Resource values can be constructed with the resource name:

```mw
const err = Error(
    code: "book.absent",
    message: $"Book {id} does not exist.",
)
```

Store identity values are produced by allocation or by wrapping checked key
components explicitly:

```mw
const id: Id(^books) = nextId(^books)
const loaded: Id(^books) = Id(^books, "book-17")
```

## Spelling

Marrow uses full statement keywords such as `if`, `else`, `for`,
`transaction`, and `delete`. Output uses the call-shaped builtin `print(...)`.
Single-letter statement abbreviations are not part of `.mw`.

Type names have one source spelling: `int`, `decimal`, `bool`, `string`,
`bytes`, `date`, `instant`, `duration`, `ErrorCode`, and `unknown`.

## Reserved And Held Names

Marrow parser-reserved words are:

```text
module use pub fn resource store index unique
required
enum evolve match is
const var if else while for in break continue return maybe absent delete merge
journal sensitive declassify
transaction lock try catch throw true false
not and or
int decimal bool string bytes date instant duration
sequence
unknown Error ErrorCode Id
```

A parser-reserved word cannot be used as a name. Bindings, parameters,
resources, fields, functions, and user module segments must not be spelled as a
parser-reserved word; doing so is a parse error. The standard-library import
`use std::bytes` is an explicit descriptor-path exception.

The reserved word `Id` remains the current identity type and constructor
spelling in `Id(^store)` and `Id(^store, key...)`. Outside those grammar
positions, it is not an identifier.

Reserved-word recognition happens before identifier parsing. Builtin names and
standard-library descriptors dispatch through the builtin table rather than
through user declarations.

`merge` and `lock` are reserved even though they are not v0.1 statements. They
have no accepted statement form or formatter round trip in v0.1; the parser
reports them as reserved when they are used as statement keywords.

The `evolve` step words `rename`, `default`, `retire`, and `transform` are
contextual: they lead a step only inside an `evolve` block, so they remain valid
identifiers elsewhere.

A bare type spelling in value position is also a parse error. A type keyword such
as `int` is valid in a type annotation or as a conversion call `int(raw)`, but
naming the type alone where a value is expected — `const Bad = int` — does not
parse, because a type spelling is not an expression.
