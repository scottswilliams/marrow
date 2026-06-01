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
- `;;` starts a documentation comment for the next declaration or resource
  element.
- Tabs are an error in Marrow source. Use spaces.

## Blocks

Blocks are introduced by indentation:

```mw
if status == "open"
    write("open")
else
    write("not open")
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

resource Book at ^books(id: int)
    required title: string
    required author: string

pub fn add(title: string): int
    return 1
```

Functions use one `fn` form. Parameters use `name: type`. Omitted return type
means the function produces no value.

## Resource Syntax

Resource indentation mirrors tree layers:

```mw
resource Patient at ^patients(id: string)
    name
        required first: string
        required last: string

    visits(date: date)
        note: string
```

Documentation comments apply to the next resource element. Source stable-id
annotations are not part of v0.1; durable element identity belongs in the
catalog.

Fields are sparse by default. Add `required` when a resource is invalid
without a populated element:

```mw
required title: string
```

Indexes are declared as direct members of keyed saved resources:

```mw
index byName(name.last, id)
index byMrn(mrn) unique
```

History is modeled as an ordinary keyed child layer:

```mw
versions(version: int)
    title: string
    body: string
```

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
write($"created {id}")
print($"created {id}")
```

Assignment is a statement only. It cannot appear as a subexpression, cannot be
chained, and does not return a value.
The right-hand expression is evaluated before the target is changed.

`write(...)` and `print(...)` use call syntax, but they are statements: they
perform output and produce no value. User-defined functions may still be
effectful and return values.

General statement chaining and postconditionals are not part of Marrow `.mw`.
Use normal `if` blocks.

## Bindings

`const` introduces an immutable binding; `var` introduces a mutable one. Scope
decides whether a `const` is a module constant or a local binding.

```mw
const left: int = 1      ; immutable local binding
var right: int = 1       ; mutable local binding
var book: Book           ; mutable local resource, built up field by field
const id = Book::Id(1)   ; immutable local value; runtime-computed is fine
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
    write("found")

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
| 5 | `_` | concatenate |
| 6 | `..`, `..=` | exclusive and inclusive ranges |
| 7 | `<`, `<=`, `>`, `>=` | comparison |
| 8 | `??` | absence default |
| 9 | `==`, `!=` | equality, not equal |
| 10 | `and` | short-circuit and |
| 11 | `or` | short-circuit or |

`%` is remainder. Use `std::math::modulo(...)` when code needs modulo
behavior for negative operands.

Arithmetic operands must be numeric. `+`, `-`, `*`, and `/` require matching
numeric types. `+`, `-`, and `*` return that type. `/` returns `decimal`.
`%` accepts `int` operands and returns `int`.

Equality requires comparable values of the same type. Ordering comparisons
require ordered values of the same type.

Concatenation with `_` requires `string` operands.

The absence-default `??` reads a possibly-absent path on its left and yields the
right operand when that path is absent. Its left operand must be a path read or a
`?.` chain — a value that is always present has nothing to default — and the
default must match the path's leaf type. It binds tighter than `==`, so
`name ?? "anon" == "anon"` is `(name ?? "anon") == "anon"`. It does not chain:
write one `??` per read.

The optional read `?.` accesses a field that may be absent. An absent step
short-circuits the rest of the chain to absent rather than failing the read, so
`^books(id)?.binding?.shelf` is absent when any step along the way is. An
unguarded `?.` chain that ends absent raises an absent-element error like any
other read; pair it with `??` to supply a default. Only absence is short-circuited
— schema and decoding errors still surface.

Ranges use `int` endpoints and yield `int` values when iterated. The checker
accepts them for `for` loops, not as saved values.

Use spaces around `_` when it is the concatenation operator; without spaces,
`_` is part of an identifier.

Operands and call arguments evaluate left to right. `and` and `or`
short-circuit; other operators evaluate their operands before applying the
operator.

## Strings

Ordinary strings are UTF-8 text:

```mw
const title = "Small Gods"
```

String literals decode `\\`, `\"`, `\n`, `\r`, and `\t`. Other backslash
escapes are rejected at runtime.

Interpolation is explicit with `$"..."`:

```mw
write($"book {id}: {title}")
```

Inside interpolation strings, text segments decode the same string escapes;
`{{` emits `{` and `}}` emits `}`. Interpolation expressions are ordinary
expressions and cannot contain statements.

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

Dots are data fields. `::` is for code namespaces.

Quoted field segments are allowed for existing data with non-identifier names:

```mw
^books(id)."old-title"
```

Managed resource declarations use identifier field names. Quoted segments are
for raw data, import, export, data evolution, and repair paths.

Reserved words are not identifiers, so a data name spelled like a keyword must
be quoted too (`^events(id)."at"`). A bare keyword after `.` is a parse error.

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

Generated resource identity types are constructed explicitly at boundaries:

```mw
const id = Book::Id(17)
```

## Spelling

Marrow uses full statement keywords such as `if`, `else`, `for`,
`transaction`, and `delete`. Output uses the call-shaped builtins `write(...)`
and `print(...)`. Single-letter statement abbreviations are not part of `.mw`.

Type names have one source spelling: `int`, `decimal`, `bool`, `string`,
`bytes`, `date`, `instant`, `duration`, `ErrorCode`, and `unknown`.

## Reserved Words

Marrow reserves:

```text
module use pub fn resource at index unique
required
const var if else while for in break continue return delete merge
transaction lock try catch finally throw out inout true false
not and or
int decimal bool string bytes date instant duration
sequence
unknown Error ErrorCode
```

A reserved word cannot be used as a name. Bindings, parameters, resources,
fields, functions, and module segments must not be spelled as a reserved word;
doing so is a parse error. Existing raw data named like a keyword is reached
through a quoted segment, as above.

`merge` and `lock` are reserved even though they are not v0.1 statements.

A bare type spelling in value position is also a parse error. A type keyword such
as `int` is valid in a type annotation or as a conversion call `int(raw)`, but
naming the type alone where a value is expected — `const Bad = int` — does not
parse, because a type spelling is not an expression.
