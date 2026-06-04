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
`duration` is elapsed time. A `duration` value is written either with a
[duration literal](syntax.md#duration-literals) such as `2.hours` or with
`std::clock::parseDuration`. Local time zone presentation belongs at host or
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
surface. Use resources for named tree shapes and `Id(^store)` for saved store
identities.

An `enum` is a named, fixed set of values — the user-defined generalization of
`bool`. It is a named scalar-valued type: a value such as `Status::archived`
compares nominally (it equals only a value of the same enum) and stores as the
member's stable catalog identity, decoded back to the current source member, so
reordering members does not change stored meaning. See [Enums](enums.md).

## Saved Types

Saved fields use concrete types. A saved leaf field may use `int`,
`decimal`, `bool`, `string`, `bytes`, `date`, `instant`, `duration`,
`ErrorCode`, or a store identity type such as `Id(^books)`.
Nested resources, sequences, and keyed trees are saved by their declared
shape.

A saved leaf field typed as `Id(^store)` is a typed reference. A field
`authorId: Id(^authors)` holds an `^authors` identity and only an `^authors`
identity: assigning an identity from a different store root, or a raw scalar, is
a check error because identities are nominal by store. A dynamic (`unknown`)
value is rejected the same way a scalar field rejects one — pass it through a
checked identity boundary first — so an unchecked value cannot land in a
reference where it would read back as a foreign or malformed identity. Writing
the field stores the referenced store identity's canonical key encoding, and
reading it back yields the same identity value, so a saved reference round-trips.
A field may reference its own store (`managerId: Id(^people)` on `Person`).

This rule covers every typed saved location, not just one field: an `unknown`
value must be converted before it is written to a scalar field, an identity field,
a whole resource (`^books(1) = value`), or a whole group entry. A dynamic record
could otherwise carry a raw scalar or foreign identity into one of the resource's
typed fields, so the value must first be made into that resource — a constructor or
a read of the same resource — before the write.

Two identities from the same store root compare with `==`; identities from
different store roots do not, even when those stores use the same resource shape.
Comparison is by the referenced keys after the nominal store match, so the same
reference written twice is equal.

Saving an identity in a field does not create a foreign-key constraint, cascade,
or join: it is a typed value, not an enforced relationship. The field is not an
unconditional write-time existence check — a reference may name a resource that
was never written or was later deleted, and a `delete` does not follow stored
references. Dangling references are still compiler-visible integrity facts:
data-attached compiler and integrity flows can report an `Id(^store)` value whose
referent is absent without turning that report into an implicit cascade or write
rejection. Applications enforce relationship policy in code or model it as
resources and indexes.

Saved keys are orderable scalar types — every scalar except `decimal`. A key
may not be `decimal`, an enum or other named type, a whole resource, a
sequence, a keyed tree, or `unknown`. Identity-typed keys such as `Id(^books)`
are not yet supported.

`unknown` belongs at dynamic boundaries, not inside managed saved schemas. If
dynamic payload must be persisted, store it as `bytes`, `string`, or an
explicit resource shape.

## Sparse Fields

An unmarked field is maybe-present. A field declaration says what type the
field has when it is populated; if it is not populated, there is simply no
node in the tree.

```mw
subtitle: string
```

Absence is a fork resolved at the read. Reading a maybe-present place never
raises an absent-element error and never yields a stored null. The read must be
resolved at the read site by one of: an absence-default `place ?? fallback`; an
`if exists(place)` branch; or optional chaining
`a?.b?.c` that ends in one of those. An unresolved maybe-present read is a
compile error that names the place and the available resolutions.

Use `exists(path)` when code needs to branch on whether a field is
populated; the check narrows the path inside the guarded block:

```mw
if exists(^books(id).subtitle)
    write(^books(id).subtitle)
```

Use the absence-default `??` when absence is expected:

```mw
const subtitle: string = ^books(id).subtitle ?? ""
```

## Required Fields

Most fields are sparse because trees are the default. Mark a field
`required` when a resource is invalid without it:

```mw
required title: string
required author: string
```

Writing a resource value must populate required fields. A required field that
is missing in stored data is a fatal data-attachment error at activation, not a
maybe-present read to resolve or a catchable branch. Assigning absence to a
required field is an error; use `delete` only when deleting the surrounding
keyed entry or resource, or when a tool/admin maintenance run grants that
capability.

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
resource Book
    required title: string
    required author: string
    shelf: string

store ^books(id: int): Book
```

Use the same type for local and saved data:

```mw
var draft: Book
draft.title = "Small Gods"
draft.author = "Terry Pratchett"
draft.shelf = "fiction"

const id: Id(^books) = nextId(^books)
^books(id) = draft
const saved: Book = ^books(id)
```

Resource constructors create local resource values:

```mw
const draft = Book(
    title: "Small Gods",
    author: "Terry Pratchett",
    shelf: "fiction",
)
```

Constructors must populate required fields. Omitted sparse fields remain
absent.

For saved resources, constructors build the resource body. Identity keys live
in the saved address and are supplied by `nextId(...)` or an explicit generated
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

Saved resource members also accept `map[K, V]` as sugar for a keyed leaf with
the implicit key name `key`:

```mw
scores: map[string, int]
```

This is equivalent to:

```mw
scores(key: string): int
```

This is declaration sugar for saved resources; it does not introduce local map
values or map operations. Map member sugar is a keyed leaf, so it is not
combined with `required`.

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

resource Game
    scores(playerId: string): int

store ^games(id: int): Game

const gameId: Id(^games) = nextId(^games)
^games(gameId).scores(playerId) = 42
```

## Identity Types

Identity is owned by the store, not derived from the resource. A keyed store
defines its identity type from the store plus its key; `Id(^store)` is the
canonical identity type. For a single-key store:

```mw
resource Book
    required title: string

store ^books(id: int): Book

const id: Id(^books) = nextId(^books)
```

A singleton store such as `store ^settings: Settings` has no generated identity
type; the root itself is addressed directly.

`Id(^books)` is a typed wrapper over the store's key. It prevents ordinary
integers from being accidentally passed as book identifiers, and it keeps IDs
from becoming meaningful business counters. Convert explicitly at boundaries
such as URLs, command arguments, or host IO.

Identity types are nominal: `Id(^books)` and `Id(^magazines)` are distinct even
when their stored keys share a shape, so an `Id(^magazines)` is rejected wherever
an `Id(^books)` is expected. Saved key arguments are type-checked statically the same
way — both a raw scalar of the wrong type (`^books("oops")`) and a foreign
identity spliced into a keyspace (`^books(magazineId)`) are reported as
`check.key_type`.

Each key passed through a store identity boundary must match the referenced
store's declared identity key type. A string key for an `int`-keyed `^books`
store is a `check.key_type`, as is a wrong-typed key of a composite identity.

At run time the key scalar type, arity, and identity store root are enforced
before any store write: a key whose scalar kind, count, or nominal store root
does not match the declared keyspace faults (`run.type`) rather than reaching the
store, and `marrow data integrity` reports an already-stored key of the wrong
scalar type as `data.key_type`. Dynamic data that arrives through `unknown` must
be checked at the identity boundary before it can reenter typed Marrow code or
managed saved data; same-shaped foreign identities are not accepted merely
because their scalar keys match. Raw scalar keys are accepted only as explicit key
arguments to a saved path; they are not `Id(^store)` values at dynamic, host, or
unknown boundaries.

Marrow provides default `nextId` allocation for a single `int` identity key.
Other identity shapes are application-provided.

A managed saved root is addressed by one identity value:

```mw
const id: Id(^books) = nextId(^books)
const title = ^books(id).title
```

The declaration lists the stored key components; ordinary typed code passes
the store identity type, not the raw key literal. Allocation and host or
application boundary helpers are responsible for producing checked identities:

```mw
const allocated: Id(^books) = nextId(^books)
const loaded: Id(^books) = loadBookId("book-17")
```

Composite-key stores also define one identity type:

```mw
resource Enrollment
    status: string

store ^enrollments(studentId: string, courseId: string): Enrollment
```

`Id(^enrollments)` represents both keys together. Application code treats it as
one identity value rather than a general tuple:

```mw
const id: Id(^enrollments) = loadEnrollmentId("student-1", "course-9")

^enrollments(id).status = "active"
```

Identity values are opaque. Do not encode business meaning into them, and do
not rely on them being gap-free. Failed or rolled-back work may leave unused
IDs behind.

`next(^books(id))` and `prev(^books(id))` type to that store's identity
(`Id(^books)`), so the neighbor is addressed like any identity:
`^books(next(^books(id))).title` is well-typed. Over a keyed child layer, `next`
and `prev` type to the layer's key. `reversed(...)` preserves its argument's
element type, so `for x in reversed(layer)` binds `x` exactly as `for x in layer`
does. Stepping off the edge yields no neighbor, so the result is maybe-present and is
resolved at the read like any maybe-present value, commonly with `??` — the
default's type drives the result. It does not raise a runtime fault.

## Mutability

`const` introduces an immutable binding; `var` introduces a mutable one. Scope
decides whether a `const` is a module constant or a local binding.

A module-level `const` is a compile-time constant; its initializer must be a
constant expression:

```mw
const MaxLoans: int = 5
```

A local `const` is an immutable binding; its initializer may be any expression,
including a runtime-computed value:

```mw
const id: Id(^books) = nextId(^books)
```

`var` declares a mutable local:

```mw
var loanCount = 0
loanCount = loanCount + 1
```

Function parameters are read-only unless declared `inout`.

## Type Inference

Local variables can infer obvious types:

```mw
const title = "Small Gods" ; string
var loanCount = 0          ; int
```

Public function parameters, return types, resource fields, keyed layers, and
saved roots are annotated.

A simple name used as a value must resolve to a binding in scope: a parameter, a
local `const` or `var`, a loop or catch binding, or a module constant.
Referencing a name that is not defined is a type error.

## Conversions

Marrow does not perform implicit conversions between scalar types. Convert at
the boundary where a value changes shape.

Conversion functions validate dynamic values:

```mw
const n: int = int(raw)
const amount: decimal = decimal(raw)
const text: string = string(raw)
const ok: bool = bool(raw)
const payload: bytes = bytes(text)
const code: ErrorCode = ErrorCode(raw)
const day: date = date(raw)
const at: instant = instant(raw)
const span: duration = duration(raw)
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

Use `unknown` for host IO, inspection tooling, and untyped boundaries.
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
- store identities save as canonical encodings of their declared key values.

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
