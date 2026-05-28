# Types

Marrow `.mw` is statically typed. The compiler checks local variables,
resources, function signatures, expressions, indexes, history layers, and
saved data access before code runs whenever the schema is known.

## Primitive Types

| Type | Meaning |
|---|---|
| `int` | Signed whole number |
| `decimal` | Finite base-10 decimal value |
| `bool` | `true` or `false` |
| `string` | UTF-8 text |
| `bytes` | Arbitrary bytes |
| `date` | Calendar date without a time zone |
| `instant` | A moment in UTC |
| `duration` | Signed time span |
| `ErrorCode` | Stable error-code text |
| `unknown` | Dynamic boundary value that must be checked before typed use |

`decimal` is exact within Marrow's decimal envelope. It is not a binary
floating-point value. Numeric overflow, division results that cannot fit, and
invalid numeric conversions raise typed numeric errors.

`string` values are valid UTF-8. Marrow does not normalize Unicode text.
Equality and ordering use the exact UTF-8 text stored in the value. Use host
libraries when an application needs locale-aware collation or presentation.

`date` is a plain calendar date. `instant` is a saved point in UTC.
`duration` is elapsed time. Local time zone presentation belongs at host or
standard-library boundaries, not in the saved-data model.

String literals in an `ErrorCode` position are checked as error codes:

```mw
throw Error(code: "book.absent", message: "Book does not exist.")
```

Use `ErrorCode(text)` when a dynamic value must be validated as an error code.

Error codes are lowercase dotted text such as `parse.syntax` or
`book.already_loaned`. Segments use lowercase letters, digits, and
underscores.

`Error` is a builtin resource-shaped type for thrown errors. It is not a
scalar and it is not a managed saved resource.

Marrow does not include user-defined type aliases in the first language
surface. Use resources for named tree shapes and generated identity types for
saved resource identities.

## Saved Types

Saved fields use concrete types. A saved leaf field may use `int`,
`decimal`, `bool`, `string`, `bytes`, `date`, `instant`, `duration`,
`ErrorCode`, or a generated resource identity type such as `Book::Id`.
Nested resources, sequences, and keyed trees are saved by their declared
shape.

Saving an identity value in a field does not create an implicit foreign-key
constraint, cascade, or join. It is a typed value. Applications enforce
relationship rules in code or model them as resources and indexes.

Saved keys use ordered key types: scalars and generated resource identity
types. Keys do not use whole resources, sequences, keyed trees, or `unknown`.

`unknown` belongs at dynamic boundaries, not inside managed saved schemas. If
dynamic payload must be persisted, store it as `bytes`, `string`, or an
explicit resource shape.

## Sparse Fields

Resource fields are sparse by default. A field declaration says what type the
element has when it is populated. If it is not populated, there is simply no
node in the tree.

```mw
subtitle: string
```

Use `exists(path)` when code needs to branch on whether an element is
populated:

```mw
if exists(^books(id).subtitle)
    write(^books(id).subtitle)
```

Use `get(path, default)` when absence is expected:

```mw
let subtitle: string = get(^books(id).subtitle, "")
```

Directly reading an unpopulated element raises an absent-element error unless
the checker can prove the element exists. An `exists(...)` check narrows the
path inside the guarded block.

## Required Fields

Most elements are sparse because trees are the default. Mark a field
`required` when a resource is invalid without it:

```mw
required title: string
required author: string
```

Writing a resource value must populate required fields. Reading an
unpopulated required field is an error. Assigning absence to a required field
is an error; use `delete` only when deleting the surrounding keyed entry or
resource, or when running in explicit maintenance mode.

A local mutable resource can be built field by field. Required fields are
checked when the resource is saved, returned, or passed where a complete
resource value is required.

Inside a keyed layer, required fields are checked for entries that exist. They
do not require every possible key to be present.

An unkeyed nested group is part of the containing resource. Required fields
inside it are required for that containing resource. If an entire nested group
is optional, leave its fields sparse and guard reads with `exists(...)`.

## Resources

A resource is a typed tree shape:

```mw
resource Book at ^books(id: int)
    required title: string
    required author: string
    shelf: string
```

Use the same type for local and saved data:

```mw
var draft: Book
draft.title = "Small Gods"
draft.author = "Terry Pratchett"
draft.shelf = "fiction"

let id = Book::Id(1)
^books(id) = draft
let saved: Book = ^books(id)
```

Resource constructors create local resource values:

```mw
let draft = Book(
    title: "Small Gods",
    author: "Terry Pratchett",
    shelf: "fiction",
)
```

Constructors must populate required fields. Omitted sparse fields remain
absent.

For saved resources, constructors build the resource body. Identity keys live
in the saved path and are supplied by `nextId(...)` or an explicit generated
identity value.

Nested fields and keyed layers are part of the type:

```mw
resource Patient
    name
        first: string
        last: string

    visits(date: date)
        note: string
```

The compiler knows `patient.name.first` is `string` and
`patient.visits(someDate).note` is `string`.

## Sequences

The fundamental collection shape is still a tree. A sequence is an
integer-keyed tree layer with 1-based keys:

```mw
tags(pos: int): string
```

Saved shape:

```text
^books(id).tags(1) = "fiction"
^books(id).tags(2) = "paperback"
```

Marrow also accepts `sequence[T]` as sugar for the same 1-based integer-keyed
tree shape:

```mw
tags: sequence[string]
```

`sequence[T]` is built-in type syntax. It does not introduce user-defined
generic types or generic functions.

Sequences are ordered by key. Holes can exist because they are trees
underneath; use `count(path)` for the number of populated immediate children,
not for the highest numeric key.

Sequence helpers use positive integer positions. If zero or negative integer
keys have meaning, use an integer-keyed tree rather than a sequence.

## Keyed Trees

Keyed trees are sparse and ordered by key:

```mw
var counts(day: date, category: string): int
counts(today, "open") = 3
```

The type declaration says:

- first layer key: `day: date`,
- second layer key: `category: string`,
- leaf value: `int`.

Keyed trees can be local or nested inside saved resources:

```mw
var localScores(playerId: string): int

resource Game at ^games(id: int)
    scores(playerId: string): int

let gameId = Game::Id(1)
^games(gameId).scores(playerId) = 42
```

## Identity Types

Keyed saved resources define identity types from their identity keys. For a
single-key resource:

```mw
resource Book at ^books(id: int)
    required title: string

let id: Book::Id = nextId(^books)
```

A singleton saved resource such as `resource Settings at ^settings` has no
generated identity type; the root itself is the resource.

`Book::Id` is a typed wrapper over the stored key. It prevents ordinary
integers from being accidentally passed as book identifiers, and it keeps IDs
from becoming meaningful business counters. Convert explicitly at boundaries
such as URLs, command arguments, or host IO.

Marrow provides default `nextId` allocation for a single `int` identity key.
Other identity shapes are application-provided.

A managed saved root is addressed by one identity value:

```mw
let id: Book::Id = nextId(^books)
let title = ^books(id).title
```

The declaration lists the stored key components; ordinary typed code passes
the generated identity type, not the raw key literal. Use the generated
identity constructor when an identity enters from a boundary:

```mw
let id = Book::Id(17)
```

Composite-key resources also define one identity type:

```mw
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string
```

`Enrollment::Id` represents both keys together. Application code treats it as
one identity value rather than a general tuple:

```mw
let id = Enrollment::Id(
    studentId: "student-1",
    courseId: "course-9",
)

^enrollments(id).status = "active"
```

Identity values are opaque. Do not encode business meaning into them, and do
not rely on them being gap-free. Failed or rolled-back work may leave unused
IDs behind.

## Mutability

`const` declares a module-level compile-time constant:

```mw
const MaxLoans: int = 5
```

`let` declares an immutable local:

```mw
let title = "Small Gods"
```

`var` declares a mutable local:

```mw
var loanCount = 0
loanCount = loanCount + 1
```

Function parameters are read-only unless declared `out` or `inout`.

## Type Inference

Local variables can infer obvious types:

```mw
let title = "Small Gods"   ; string
var loanCount = 0          ; int
```

Public function parameters, return types, resource fields, keyed layers, and
saved roots are annotated.

## Conversions

Marrow does not perform implicit conversions between scalar types. Convert at
the boundary where a value changes shape.

Conversion functions validate dynamic values:

```mw
let n: int = int(raw)
let amount: decimal = decimal(raw)
let text: string = string(raw)
let ok: bool = bool(raw)
let payload: bytes = bytes(text)
let code: ErrorCode = ErrorCode(raw)
let day: date = date(raw)
let at: instant = instant(raw)
let span: duration = duration(raw)
```

`raw` means a value whose type is not known statically, usually from host IO
or an untyped saved tree. Prefer typed resources and typed
function signatures over passing `raw` values around.

`bool(...)` accepts only canonical Marrow boolean values: `false`, `true`, `0`,
and `1`.

## `unknown`

`unknown` is a safe dynamic boundary. It cannot be used as a concrete type
without conversion:

```mw
fn parseTitle(raw: unknown): string
    return string(raw)
```

Use `unknown` for host IO, raw inspection, and untyped boundaries.
Managed saved resources use concrete field and key types. If dynamic payload
must be persisted, store it as `bytes`, `string`, or an explicitly modeled
resource shape.

Marrow does not include a general `any` type in ordinary `.mw`. Dynamic data
comes through `unknown` and must be checked before typed use.

## Saved Encoding

Types do not make the saved database a hidden object store. Saved values are
bytes with compiler/runtime validation at Marrow boundaries. Each scalar has one
canonical saved form, so backup, diff, equality, and restore do not depend on
the backend:

- `bool` saves as `0` or `1`.
- `int` saves as canonical decimal text: an optional `-` then digits with no
  leading zeros. Zero is `0`; there is no `+` and no `-0`.
- `decimal` saves as canonical decimal text: an optional `-`, an integer part
  with no leading zeros (a magnitude below one is written as `0`), an optional
  `.` with fractional digits and no trailing zeros, and no exponent. Zero is
  `0`. The form is value-canonical, so trailing-zero scale is not preserved:
  `1.0` and `1.00` both save as `1`.
- `string` saves UTF-8 bytes.
- `bytes` saves arbitrary bytes.
- `date` saves as `YYYY-MM-DD`: a zero-padded ISO 8601 calendar date with no
  time zone, for years 0001 through 9999.
- `instant` saves as `YYYY-MM-DDTHH:MM:SSZ` in UTC: RFC 3339 with a literal `Z`,
  never a numeric offset. Fractional seconds appear only when non-zero, to at
  most nanosecond precision, with no trailing-zero groups.
- `duration` saves as a signed `PT<seconds>S` span: an optional `-` then seconds
  with no leading zeros and an optional trailing-zero-trimmed fraction to at
  most nanosecond precision. Zero is `PT0S`. A duration is an elapsed span, so
  it never uses calendar components.
- `ErrorCode` saves as stable UTF-8 text.
- generated resource identities save as canonical encodings of their declared
  key values.

The `decimal` envelope is a signed coefficient of up to 34 significant digits,
with up to 34 of them after the decimal point. Values outside the envelope, and
arithmetic that cannot fit, raise typed numeric errors.

Saved keys are also bytes, ordered by Marrow's key ordering rules. Typed key
layers validate and canonicalize keys before traversal.

Within a declared typed layer, key order is typed and locale-independent:
booleans sort false then true, numbers by numeric value, dates and instants
chronologically, durations by signed length, strings by UTF-8 byte order, and
bytes by byte order. Keys encode to order-preserving bytes, so this order holds
on any backend regardless of its locale or collation. Raw inspection uses the
stable encoded segment order.

Absence is represented by no value at a path, not by a stored null marker.
