# Types And Values

Marrow is statically typed. A checked expression has one type, and assignment,
arguments, and return values require compatible types. The language does not
perform implicit numeric or text conversions.

## Primitive Scalar Types

| Type | Values |
|---|---|
| `int` | Signed 64-bit integers |
| `bool` | `true` or `false` |
| `string` | UTF-8 text |
| `bytes` | Byte sequences |
| `decimal` | Exact base-10 values with at most 34 significant digits and 34 fractional places |
| `date` | Calendar dates in years 0001 through 9999 |
| `instant` | Nanosecond UTC instants in years 0001 through 9999 |
| `duration` | Signed fixed elapsed nanoseconds in the `i128` range |

On the current beta line the implemented scalar floor is `int`, `bool`, `string`,
and `bytes`. `decimal` and the temporal types (`date`, `instant`, `duration`) are
recorded here as direction and are not yet accepted by the compiler; a program that
uses one reports `check.unsupported` until its lane lands.

## Type Aliases

`alias Name = Type` declares a transparent alias: the name denotes exactly its
target type wherever a type annotation is written. An alias mints no new type
identity and no constructor — `alias Count = int` makes `Count` and `int` the
same type, and a `Count` value is an `int` value. Alias names are unique across
the project alongside resource names.

```mw
module docs::aliases

alias Count = int
alias MaybeCount = Count?

fn maybe(present: bool): MaybeCount
    if present
        return 1
    return absent

pub fn firstOr(present: bool, fallback: Count): Count
    if const value = maybe(present)
        return value
    return fallback
```

Aliases may chain; a cyclic chain reports `check.recursion` at each alias on
the cycle. An alias whose target names no known type reports `check.type`, even
when the alias is unused. Expansion happens before every other type rule, so an
alias cannot express anything its expansion could not — in particular `M?`
where `M` is itself optional is still a rejected nested optional. Alias names
are type annotations only: they are not conversion or constructor names in
expressions.

## Nominal Int Types

`type Name: int in lo..hi` declares a nominal type over `int`: a distinct type
whose every value lies in the declared interval. Unlike a transparent `alias`,
the name mints its own identity and constructor — an `int` is not a `Name` and
a `Name` is not an `int`; each conversion point is explicit in the source.

The interval follows the language's range operators: `in 0..150` admits `0`
through `149` (the end is excluded), and `in 0..=150` admits `0` through `150`.
The lower bound is always included. Both bounds are int literals (a leading `-`
is allowed), the range takes no step, and the interval must admit at least one
value; an empty interval reports `check.type`. The base type is `int` only;
a nominal over another scalar reports `check.unsupported`.

`Name(n)` constructs a value from an int expression. Construction validates the
interval at runtime: an out-of-interval value faults `run.range` at the
construction's source span, and the fault is not catchable inside the program.
`Name.checked(n)` is the fault-free form: it returns `Name?`, present exactly
when `n` lies in the interval and `absent` otherwise.

A function parameter declared with a nominal type revalidates the interval on
entry: in-language callers must already pass a value of the type, and a
terminal caller supplying an export's argument as a plain int faults
`run.range` when it lies outside the interval.

The optional `supports` clause draws from the closed capability set `add`,
`subtract`, `step`, `scale`. Each capability admits operators over the type;
every operator that produces a value of the type revalidates the interval the
same way construction does, so no expressible path yields an out-of-interval
value:

| Capability | Admits | Result |
|---|---|---|
| `add` | `Name + int`, `int + Name` | `Name`, revalidated |
| `subtract` | `Name - int` | `Name`, revalidated |
| `subtract` | `Name - Name` | plain `int`, no validation |
| `step` | `Name + 1`, `Name - 1` (the literal `1`) | `Name`, revalidated |
| `scale` | `Name * int`, `int * Name` | `Name`, revalidated |

A difference of two values (`Name - Name`) is a count, not a value of the
type, so it is a plain `int` and needs no interval. Comparisons between two
values of the same nominal type (`==`, `!=`, `<`, `<=`, `>`, `>=`) are always
admitted and need no capability: they compare the int representations and
construct nothing. Applying an operator the type does not support, or mixing a
nominal with a plain `int` in a comparison, reports `check.type` naming the
missing capability or the operand types. A nominal type without a `supports`
clause admits construction, `.checked`, and same-type comparisons only.

```mw
module docs::nominal

type Age: int in 0..=150 supports add, subtract

pub fn older(a: Age, years: int): Age
    return a + years

pub fn gap(a: Age, b: Age): int
    return a - b

pub fn tryAge(n: int): Age?
    return Age.checked(n)
```

Nominal values are ordinary copied values with the same value semantics as
`int`. Nominal type names share the project-wide type namespace with aliases
and resource names; a collision reports `check.name_conflict`. `Name?` is an
ordinary optional. Nominal types are not yet admitted as resource field types,
store key types, or constant types; those positions report `check.unsupported`
until their lanes land.

## Nominal Values

An enum value belongs to one declared closed enum and selects one of its
members. A member is bare (`Color::red`) or carries a dense typed payload
(`Shape::circle(radius: 3)`); the payload fields are named at construction and
bound positionally by a `match`. `==` and `!=` are exact enum equality (the same
member with equal payload), and a `match` covers every member exactly once with
no wildcard arm. Enums are described under
[Enum matching](control-flow.md#enum-matching); hierarchical `category` enums and
the `is` operator are future. `Id(^root)` identifies an entry under one declared
store root. These are nominal values rather than primitive scalars: values from
different enum declarations or store roots are different types even when their
stored representations have the same shape.

`unknown` is an explicit dynamic boundary. A local `unknown` binding can carry a
value whose static shape is unresolved, but operations that require a concrete
type still reject it until a checked conversion or boundary supplies one.
Durable fields, keys, and collection elements cannot use `unknown`.

## Operators

Operators require the operand combinations below; Marrow does not widen or mix
numeric types implicitly.

| Form | Accepted operands | Result |
|---|---|---|
| `-value` | `int` or `decimal` | same type |
| `not value` | `bool` | `bool` |
| `a + b` | matching numeric types; `string`; `duration`; `instant + duration` | matching type, or `instant` |
| `a - b` | matching numeric types; `duration`; `instant - duration`; `instant - instant` | matching type, `instant`, or `duration` |
| `a * b` | matching `int` or matching `decimal` | matching type |
| `a / b` | matching `int` or matching `decimal` | `int` for `int / int`, else `decimal` |
| `a % b` | `int` and `int` | `int` |
| `<`, `<=`, `>`, `>=` | matching `int`, `decimal`, `string`, `bytes`, `date`, `instant`, or `duration` | `bool` |
| `==`, `!=` | matching scalars, the same enum type, or identities for the same store root | `bool` |
| `and`, `or` | `bool` and `bool` | `bool` |
| `optional ?? fallback` | compatible present-arm types | presence follows the fallback |

`int / int` is integer division truncated toward zero, paired with the `int % int`
remainder so that `a == (a / b) * b + a % b`. A zero divisor raises
`run.divide_by_zero`, and `i64::MIN / -1` (like `i64::MIN % -1`) raises
`run.overflow` because its result is unrepresentable.

Whole resources, sequences, keyed collections, and `Error` values have no
equality operator. Arithmetic overflow, invalid division, and invalid temporal
arithmetic raise typed runtime errors. Range endpoint and step combinations are
defined under [Traversal and indexes](traversal-and-indexes.md#ranges).

## Explicit Conversion

The scalar conversion names `bool`, `int`, `decimal`, `string`, `bytes`, `date`,
`instant`, `duration`, and `ErrorCode` use call syntax. Conversions validate
their input and fail at runtime when the value cannot be represented.
`Id(^root, keys...)` uses the same call shape but constructs a nominal entry
identity rather than converting one scalar type to another.

```mw
module docs::conversion

pub fn normalized(raw: string): string
    const amount: int = int(raw)
    const rendered: string = string(amount)
    return rendered

pub fn checkedCode(raw: string): ErrorCode
    return ErrorCode(raw)
```

| Target | Accepted source values |
|---|---|
| `bool` | `bool`, or `int` equal to `0` or `1` |
| `int` | `int`, canonical integer text, or an integral `decimal` in range |
| `decimal` | `decimal`, `int`, or canonical decimal text in range |
| `string` | Any scalar or enum value, rendered canonically |
| `bytes` | `bytes`, or the UTF-8 bytes of a `string` |
| `date` | `date` or validated date text |
| `instant` | `instant` or validated instant text |
| `duration` | `duration` or validated duration text |
| `ErrorCode` | validated `string` text |

On the current beta line the implemented conversions are `string(int)`,
`string(bool)`, and `bytes(string)`; the remaining rows are direction and report
`check.unsupported` until their scalar types land.

`ErrorCode` is represented as a string value. `ErrorCode(text)` requires two or
more nonempty dot-separated segments containing lowercase ASCII letters,
digits, or `_`. Explicit initialization of a non-optional annotated local and
construction or assignment of a declared resource `ErrorCode` member apply the
same validation.

The current checker does not preserve this refinement through function
parameters and returns, later reassignment of a bare local, local collection
elements, key columns, optional local initialization, an uninitialized
`ErrorCode` variable, or a module-level constant. Those positions can behave as
plain `string`; an uninitialized variable starts as the empty string. This is an
implementation limitation, not a separate persisted scalar representation.

## Optional Values

`T?` contains either a present `T` or `absent`. Optional types do not nest.
Fields, key columns, keyed leaf declarations, and sequence element declarations
cannot themselves be optional; `sequence[T]?` is valid because the optionality
applies to the sequence value.

Optional values arise from sparse reads, lookup operations, optional returns,
and optional standard-library functions. Four constructs consume them:

- `value ?? fallback` selects the present value or a fallback.
- `if const name = value` enters its block and binds `name` only when the value
  is present.
- `exists(place)` tests path presence and narrows the guarded path.
- `value?.member` propagates absence while reading a resource member.

`if const` accepts any optional expression. It is not limited to durable reads.

```mw
module docs::optionals

fn maybeLabel(enabled: bool): string?
    if enabled
        return "enabled"
    return absent

pub fn show(enabled: bool)
    if const label = maybeLabel(enabled)
        print(label)
    else
        print("disabled")
```

The optional-producing call is evaluated once. An optional expression used as
a key, address base, or durable read address must still satisfy the checker
rules for that context; a call is not rejected merely because it is a
user-defined function.

## Presence And Narrowing

A durable resource read is optional until the path is known present. A required
member is guaranteed only after its containing resource is present. For
example, `^books(id).title` has type `string?` if `^books(id)` might be absent,
even when `title` is declared `required`.

```mw
module docs::presence

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn display(id: int)
    if exists(^books(id).subtitle)
        print(^books(id).subtitle)

    if const book = ^books(id)
        print(book.title)
        print(book.subtitle ?? "(no subtitle)")
```

Within the first branch, the exact guarded place is present. Within the second,
the materialized `Book` has a bare required `title` and an optional sparse
`subtitle`.

Presence knowledge is invalidated when code may change the relevant address or
data. This includes mutation of a key expression, a write to the guarded durable
path, or a helper call whose effects may write it. Code must test presence again
after invalidation.

## Option And Result

`Option[T]` and `Result[T, E]` are built-in closed enum value types. They ride the
same machinery as a user `enum` — constructed, matched, and compared by value —
and their type arguments may be any value type, including nested `Option` and
`Result`. `Option[T]` has the members `none` and `some(v)`; `Result[T, E]` has
`ok(v)` and `err(e)`. `T`, `E`, `some`, `none`, `ok`, and `err` are reserved: the
constructors are the built-in members, and `Option` and `Result` cannot be
redeclared.

A value is constructed with its member: `some(v)`, `none`, `ok(v)`, or `err(e)`.
`some(v)` infers its `Option[T]` from `v`; `none`, `ok(v)`, and `err(e)` cannot
infer the whole type argument set, so they need an expected type — an annotation, a
call argument, a return type, or the other side of a coercion. A `match` covers the
members exactly, binding the payload positionally:

```mw
module docs::optionvalue

fn firstEven(a: int, b: int): Option[int]
    if a % 2 == 0
        return some(a)
    if b % 2 == 0
        return some(b)
    return none

pub fn describe(o: Option[int]): string
    match o
        some(v)
            return "some"
        none
            return "none"
```

Nested `Option` is distinct: `none`, `some(none)`, and `some(some(v))` are three
different values of `Option[Option[int]]`. `==` and `!=` are exact equality — the
same member with equal payload, compared over the same instantiation.

`Result[T, E]` models a recoverable failure. Prefix `try <expr>` propagates it (see
[Control flow](control-flow.md#prefix-try-and-transaction)): on `ok(v)` it yields
`v`, and on `err(e)` it returns `err(e)` from the enclosing `Result[U, E]`-returning
function, with the same error type `E`.

`Option[T]` is distinct from the presence primitive `T?`. A `T?` place is absent or
present and is produced by sparse reads, lookups, and optional returns; `??`,
`if const`, `exists`, and `?.` consume it. An `Option[T]` is an ordinary value you
build with `some`/`none`, pass, return, and `match`. Use `T?` for the presence of a
place, and `Option[T]` when absence is a value the program passes around or stores
in a structure. A sparse field whose type is `Option[string]` reads as
`Option[string]?` — absent (the field is unset) versus a present `Option` that is
itself `none` or `some`.

## Resources

A resource value has the declared resource type. Required fields are bare in a
materialized resource; sparse fields remain optional. Unkeyed nested groups are
members of the containing value. Keyed child layers are collections addressed
separately and are not materialized as part of a local resource value.

Resource values are copied by value through local bindings, parameters, and
returns. See [Resources](resources.md).

A resource value held in a local can be read and, when the binding is a `var`,
mutated field by field. Reading a required field yields its bare value; reading a
sparse field yields `T?` — present when the field holds a value, absent otherwise.
Assigning a field, required or sparse, sets it present to the assigned value.
`unset place` clears a sparse field back to absent, where `place` is a field
access on a local product (`r.note`); a required field cannot be unset, and a
durable place is erased with `delete`, not `unset`. Because a sparse field is
absent or a present value — never a stored empty — a sparse field typed
`Option[string]` keeps its three states distinct: absent (unset), a present
`Option` `none`, and a present `Option` `some(v)`.

```mw
module docs::sparselocal

resource Box
    required id: int
    note: string

pub fn label(): string
    var b = Box(id: 1)
    b.note = "draft"
    unset b.note
    return b.note ?? "unlabeled"
```

A field type is a scalar or a built-in `Option`/`Result` value type. A resource
backing a `store` has only scalar fields, since durable storage records scalar
leaves.

## Structs

A `struct` declares a dense product value type. Every field is required, held
inline, and named `name: Type` over a scalar type. Unlike a resource, a struct is
not durable and has no keyed layers, groups, or sparse fields.

```mw
struct Point
    x: int
    y: int
```

A struct value is built with a named-only literal that provides every field
exactly once; the field arguments may appear in any order and are evaluated in
field declaration order:

```mw
const p = Point(x: 3, y: 4)
const q = Point(y: 4, x: 3)
```

Fields are read with `.`, yielding the field's scalar type. Struct values are
copied by value through local bindings, assignments, parameters, and returns, like
every other value. A struct name is project-global and is written without a module
qualifier.

A struct is admitted as a parameter type and as a return type: a value travels by
value into and out of ordinary functions. A returned struct is rendered by
`marrow run` as a JSON object under `--format jsonl` (field names as keys, in
ascending byte order) and as `{field: value, ...}` in text; a struct has no
command-line argument spelling, so an export taking a struct parameter cannot be
invoked from the terminal.

A field type is a scalar type (or an alias that expands to one). The current
compiler does not admit a struct as the type of a struct field; that is a
`check.unsupported` diagnostic. A missing, unknown, duplicated, or wrong-typed
field argument, and an unnamed (positional) argument, are `check.type`
diagnostics; a struct name that collides with another declared type is a
`check.name_conflict`.

## Sequences And Keyed Collections

`sequence[T]` is a local or declared positional collection whose keys are
positive integers. A declaration such as `var scores(name: string): int`
creates a local keyed collection. Declared resource layers and keyed local
collections may have several typed key columns and may contain resource entries
rather than scalar leaves.

A collection read at a key has type `T?` because the position may be absent.
Iteration visits only present entries. Sequences may be passed and returned by
value. A keyed local collection may be passed to a parameter with the same keyed
shape, but the language has no keyed-collection return-type syntax.

## Key Types

Store identity columns, resource keyed layers, and local keyed collections
accept `int`, `bool`, `string`, `bytes`, `date`, `instant`, and `duration` keys.
The `ErrorCode` spelling is also accepted, but current key compilation erases
its refinement: the key behaves as `string` and is not grammar-validated.
`decimal`, enums, entry identities, resources, sequences, optionals, and
`unknown` are not accepted in those key positions. Declared index components may
also project enum and entry-identity fields; `decimal` remains unavailable.

Key order is part of observable traversal behavior. Numeric and temporal keys
use their natural order, `false` precedes `true`, and strings and bytes use
lexicographic order. Composite key tuples are ordered lexicographically by
column. Enum-valued index components follow member declaration order; an
identity-valued component follows the addressed root's key-tuple order.

## Entry Identity

`Id(^root)` is nominally tied to its store root. `Id(^books)` and
`Id(^authors)` are different types even if both roots use integer keys.
Composite store keys still produce one `Id(^root)` value.

`Id(^root, keys...)` wraps explicit key values as an entry identity. It does not
read the store and does not establish that the entry exists. Stored identity
values do not create a cascading relationship: deleting the addressed entry
does not rewrite other values that contain its identity. `marrow data integrity`
can report such dangling identities.

## Error Values

`Error` is the built-in resource value constructed by `Error(...)`, returned
like another resource, or transferred with `throw`. Its fields, construction
rules, catchability, and transaction interaction are defined in
[Errors and transactions](errors-and-transactions.md).

## Mutability

Type and mutability are separate. `const` prevents reassignment of its binding;
`var` permits reassignment. A local resource held by `var` permits member
assignment. Durable places are assigned directly and are not made mutable by a
local binding.
