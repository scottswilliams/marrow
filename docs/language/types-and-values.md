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
| `date` | Proleptic-Gregorian calendar days in years 0001 through 9999 |
| `instant` | UTC nanosecond instants in years 0001 through 9999 |
| `duration` | Signed elapsed nanoseconds over the `i128` range |
| `decimal` | Exact base-10 values (future) |

The implemented scalar floor is `int`, `bool`, `string`, `bytes`, and the temporal
types `date`, `instant`, and `duration` (see [Temporal Types](#temporal-types)).
`decimal` is recorded here as direction and is not yet accepted by the compiler; a
program that uses it reports `check.unsupported` until its lane lands.

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

fn maybe(present: bool): MaybeCount {
    if present { return 1 }
    return absent
}

pub fn firstOr(present: bool, fallback: Count): Count {
    if const value = maybe(present) {
        return value
    }
    return fallback
}
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

pub fn older(a: Age, years: int): Age {
    return a + years
}

pub fn gap(a: Age, b: Age): int {
    return a - b
}

pub fn tryAge(n: int): Age? {
    return Age.checked(n)
}
```

Nominal values are ordinary copied values with the same value semantics as
`int`. Nominal type names share the project-wide type namespace with aliases
and resource names; a collision reports `check.name_conflict`. `Name?` is an
ordinary optional. Nominal types are not yet admitted as resource field types,
store key types, or constant types; those positions report `check.unsupported`
until their lanes land.

## Temporal Types

Marrow has three temporal value types. Each is a pure value: it has a fixed
integer representation, deterministic parse, format, and order, and no dependence
on a clock, timezone, or locale.

| Type | Representation | Range |
|---|---|---|
| `date` | proleptic-Gregorian days since 1970-01-01 | years 0001-9999 |
| `instant` | signed nanoseconds since 1970-01-01T00:00:00Z, in UTC | years 0001-9999 |
| `duration` | signed nanoseconds | the `i128` range |

### Construction

A temporal value is constructed from exactly one static string literal in the
type's canonical text form, validated and folded at compile time. There is no
suffix literal (the prototype's `1.second` form reports `check.unsupported`), no
ambient `today`/`now`, and no runtime string parse in the language floor.

| Constructor | Canonical text | Example |
|---|---|---|
| `date("…")` | `YYYY-MM-DD` (fixed width) | `date("2026-07-15")` |
| `instant("…")` | `YYYY-MM-DDTHH:MM:SS[.fraction]Z` (UTC, `Z` required) | `instant("2026-07-15T17:00:00Z")` |
| `duration("…")` | `[-]PT<seconds>[.fraction]S` (zero is `PT0S`) | `duration("PT3600S")` |

The forms are strict and canonical: a field must have its fixed width, an
optional sub-second `fraction` is one to nine digits with no trailing zero, whole
seconds carry no leading zero, and `-PT0S` is not a duration (zero is `PT0S`). A
malformed form, an impossible date such as `date("2021-02-29")`, or a `date`/
`instant` outside years 0001-9999 is a compile-time `check.type` diagnostic at the
literal, so no ordinary program produces an out-of-range temporal value. The
constructor argument must be a literal; a non-literal argument reports
`check.unsupported`.

### Operations

The closed operation floor:

- comparison and equality (`==`, `!=`, `<`, `<=`, `>`, `>=`) between two values of
  the same temporal type;
- `duration + duration` and `duration - duration`, yielding a `duration`;
- `instant + duration` and `instant - duration`, yielding an `instant`;
- `addDays(d: date, n: int): date` — the date `n` days after `d`;
- `daysBetween(a: date, b: date): int` — the signed number of days from `a`
  to `b`.

An arithmetic result outside the type's domain (a `date` or `instant` past years
0001-9999, or a `duration` beyond the `i128` range) faults `run.temporal_overflow`
at runtime, mapped to the operation's span and not catchable inside the program.
There is no other temporal arithmetic: no `date` operator, no `instant - instant`,
no scaling a `duration` by an `int`, and no calendar-month or calendar-year unit.

```mw
module docs::temporal

pub fn dueDate(assigned: date, leadDays: int): date {
    return addDays(assigned, leadDays)
}

pub fn isOverdue(due: date, onDay: date): bool {
    return due < onDay
}

pub fn reminderAt(deadline: instant, lead: duration): instant {
    return deadline - lead
}
```

### Order and keys

Each temporal type is orderable: `date` and `instant` by their instant on the
timeline, `duration` by signed length. The order is total and agrees with the
order-preserving durable key encoding, so a temporal type is a key type — a
`Map<date, V>` is admitted and iterates in ascending date order (see
[Key Types](#key-types)).

A clock is not part of the language. Reading the current instant or day is a
host effect a future lane introduces explicitly; until then a program takes the
relevant day or instant as an argument.

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
| `-value` | `int` | `int` |
| `not value` | `bool` | `bool` |
| `a + b` | `int`; `string`; `duration + duration`; `instant + duration` | matching type, or `instant` |
| `a - b` | `int`; `duration - duration`; `instant - duration` | matching type, or `instant` |
| `a * b` | `int` and `int` | `int` |
| `a / b` | `int` and `int` | `int` |
| `a % b` | `int` and `int` | `int` |
| `<`, `<=`, `>`, `>=` | matching `int`, `string`, `bytes`, `date`, `instant`, or `duration` | `bool` |
| `==`, `!=` | matching scalars, the same enum type, or identities for the same store root | `bool` |
| `and`, `or` | `bool` and `bool` | `bool` |
| `optional ?? fallback` | compatible present-arm types | presence follows the fallback |

The temporal operators are a closed set: a `duration` sums or differs with a
`duration`, and a `duration` shifts an `instant`. There is no `date` operator (use
`addDays`/`daysBetween`), no `instant - instant`, no `duration * int`,
and no calendar-month or calendar-year arithmetic. See
[Temporal Types](#temporal-types).

`int / int` is integer division truncated toward zero, paired with the `int % int`
remainder so that `a == (a / b) * b + a % b`. A zero divisor raises
`run.divide_by_zero`, and `i64::MIN / -1` (like `i64::MIN % -1`) raises
`run.overflow` because its result is unrepresentable.

Whole resources, lists, maps, and `Error` values have no top-level equality
operator, though a list or map reached inside a compared struct or enum
participates in that aggregate's structural equality. Arithmetic overflow,
invalid division, and invalid temporal
arithmetic raise typed runtime errors. Range endpoint and step combinations are
defined under [Traversal and indexes](traversal-and-indexes.md#ranges).

## Explicit Conversion

The scalar conversion names `bool`, `int`, `decimal`, `string`, `bytes`, and
`ErrorCode` use call syntax. Conversions validate their input and fail at runtime
when the value cannot be represented. `Id(^root, keys...)` uses the same call
shape but constructs a nominal entry identity rather than converting one scalar
type to another. The temporal names `date`, `instant`, and `duration` also use
call syntax, but they are compile-time literal constructors rather than runtime
conversions (see [Temporal Types](#temporal-types)).

```mw
module docs::conversion

pub fn render(amount: int): string {
    return string(amount)
}

pub fn renderFlag(active: bool): string {
    return string(active)
}
```

| Target | Accepted source values |
|---|---|
| `bool` | `bool`, or `int` equal to `0` or `1` |
| `int` | `int`, canonical integer text, or an integral `decimal` in range |
| `decimal` | `decimal`, `int`, or canonical decimal text in range |
| `string` | Any scalar or enum value, rendered canonically |
| `bytes` | `bytes`, or the UTF-8 bytes of a `string` |
| `ErrorCode` | validated `string` text |

On the current beta line the implemented conversions are `string(int)`,
`string(bool)`, and `bytes(string)`; every other row above — including parsing an
`int` or `decimal` from text — is documented direction that is not yet
implemented and reports `check.unsupported`. Temporal values are built with the
literal constructors in [Temporal Types](#temporal-types), not these conversions.

`ErrorCode` is represented as a string value. Its documented direction is that
`ErrorCode(text)` requires two or more nonempty dot-separated segments containing
lowercase ASCII letters, digits, or `_`, and that the same validation applies to
explicit initialization of a non-optional annotated local and to construction or
assignment of a declared resource `ErrorCode` member. The `ErrorCode` conversion
is not yet implemented on the beta line and reports `check.unsupported`.

The current checker does not preserve this refinement through function
parameters and returns, later reassignment of a bare local, local collection
elements, key columns, optional local initialization, an uninitialized
`ErrorCode` variable, or a module-level constant. Those positions can behave as
plain `string`; an uninitialized variable starts as the empty string. This is an
implementation limitation, not a separate persisted scalar representation.

## Optional Values

`T?` contains either a present `T` or `absent`. Optional types do not nest.
Fields, key columns, and keyed leaf declarations cannot themselves be optional;
`List<T>?` is valid because the optionality applies to the collection value.

Optional values arise from sparse reads, lookup operations, optional returns,
and optional standard-library functions. Four constructs consume them:

- `value ?? fallback` selects the present value or a fallback.
- `if const name = value` enters its block and binds `name` only when the value
  is present.
- `exists(place)` tests path presence and narrows the guarded path.
- `value?.member` reads a member through an optional composite (`resource` or
  `struct`) value: an absent value yields `absent`, and a present value yields the
  member wrapped optional, so the read never faults on absence and its result is
  itself optional.

`if const` accepts any optional expression. It is not limited to durable reads.

```mw
module docs::optionals

fn maybeLabel(enabled: bool): string? {
    if enabled { return "enabled" }
    return absent
}

pub fn show(enabled: bool) {
    if const label = maybeLabel(enabled) {
        print(label)
    } else {
        print("disabled")
    }
}
```

The optional-producing call is evaluated once. An optional expression used as
a key, address base, or durable read address must still satisfy the checker
rules for that context; a call is not rejected merely because it is a
user-defined function.

## Presence And Narrowing

A durable resource read is optional until the path is known present. A required
member is guaranteed only after its containing resource is present. For
example, `^books[id].title` has type `string?` if `^books[id]` might be absent,
even when `title` is declared `required`.

```mw
module docs::presence

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn display(id: int) {
    if exists(^books[id].subtitle) {
        print(^books[id].subtitle)
    }

    if const book = ^books[id] {
        print(book.title)
        print(book.subtitle ?? "(no subtitle)")
    }
}
```

Within the first branch, the exact guarded place is present. Within the second,
the materialized `Book` has a bare required `title` and an optional sparse
`subtitle`.

Presence knowledge is invalidated when code may change the relevant address or
data. This includes mutation of a key expression, a write to the guarded durable
path, or a helper call whose effects may write it. Code must test presence again
after invalidation.

## Option And Result

`Option<T>` and `Result<T, E>` are ordinary generic enums the toolchain defines
(see [Generic types](#generic-types)); they are not a built-in special case. Each is
monomorphized by its type arguments through the same machinery a user generic enum
uses — constructed, matched, and compared by value — and their type arguments may be
any value type, including nested `Option` and `Result`. `Option<T>` has the members
`none` and `some(v)`; `Result<T, E>` has `ok(v)` and `err(e)`. `Option` and `Result`
are reserved type names that cannot be redeclared. Their constructors `some`,
`none`, `ok`, and `err` are reserved value names that resolve to those enums'
variants: a function, constant, parameter, or local binding that reuses one is a
`check.name_conflict` at the declaration, so the constructor is never silently
shadowed.

A value is constructed with its member: `some(v)`, `none`, `ok(v)`, or `err(e)`.
`some(v)` infers its `Option<T>` from `v`; `none`, `ok(v)`, and `err(e)` cannot
infer the whole type argument set, so they need an expected type — an annotation, a
call argument, a return type, or the other side of a coercion. A `match` covers the
members exactly, binding the payload positionally:

```mw
module docs::optionvalue

fn firstEven(a: int, b: int): Option<int> {
    if a % 2 == 0 { return some(a) }
    if b % 2 == 0 { return some(b) }
    return none
}

pub fn describe(o: Option<int>): string {
    match o {
        some(v) => return "some"
        none => return "none"
    }
}
```

Nested `Option` is distinct: `none`, `some(none)`, and `some(some(v))` are three
different values of `Option<Option<int>>`. `==` and `!=` are exact equality — the
same member with equal payload, compared over the same instantiation.

`Result<T, E>` models a recoverable failure. Prefix `try <expr>` propagates it (see
[Control flow](control-flow.md#prefix-try-and-transaction)): on `ok(v)` it yields
`v`, and on `err(e)` it returns `err(e)` from the enclosing `Result<U, E>`-returning
function, with the same error type `E`.

`Option<T>` is distinct from the presence primitive `T?`. A `T?` place is absent or
present and is produced by sparse reads, lookups, and optional returns; `??`,
`if const`, `exists`, and `?.` consume it. An `Option<T>` is an ordinary value you
build with `some`/`none`, pass, return, and `match`. Use `T?` for the presence of a
place, and `Option<T>` when absence is a value the program passes around or stores
in a structure. A sparse field whose type is `Option<string>` reads as
`Option<string>?` — absent (the field is unset) versus a present `Option` that is
itself `none` or `some`.

A sparse field already models absence: an unset field reads `absent`. Declare a
field `Option<T>` only when a stored `none` must be distinguishable from the field
being unset — the three-state case. Such a field is read by proving presence and
then matching the stored `Option`:

```mw
module docs::three_state

resource Reading {
    measured: Option<int>
}

store ^readings[id: int]: Reading

pub fn describe(): string {
    const r = Reading(measured: some(7))
    if const stored = r.measured {
        match stored {
            some(v) => return "measured"
            none => return "recorded as unmeasurable"
        }
    }
    return "not recorded"
}
```

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
`Option<string>` keeps its three states distinct: absent (unset), a present
`Option` `none`, and a present `Option` `some(v)`.

```mw
module docs::sparselocal

resource Box {
    required id: int
    note: string
}

pub fn label(): string {
    var b = Box(id: 1)
    b.note = "draft"
    unset b.note
    return b.note ?? "unlabeled"
}
```

A field type is a scalar, a closed enum value type (a user `enum` or a built-in
`Option`/`Result`), or an alias that expands to one. A user-enum field holds a
local enum value, so a `match` may dispatch on a field read. A resource backing a
`store` may hold a scalar field or a widened value field — a dense `struct`/record,
a closed `enum`, or an `Option`/`Result` — each stored inline in its field-leaf
cell and round-tripped as a runtime value. A nominal-typed stored field reports
`check.unsupported` until its lane lands, and a collection is never stored inline in
a field — a collection belongs under a keyed `branch`, so a collection field is
rejected.

## Structs

A `struct` declares a dense product value type. Every field is required, held
inline, and named `name: Type` over any value type. Unlike a resource, a struct is
not durable and has no keyed layers, groups, or sparse fields.

```mw
struct Point {
    x: int
    y: int
}
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

A field type is any value type (or an alias that expands to one): a scalar, a
nominal, another struct, or a closed enum (a user `enum` or a built-in
`Option`/`Result`). A struct field may name a struct or enum declared later in the
file, and two structs may reference each other, because every value type is
declared before any field type is resolved. The only nesting restriction is
acyclicity: a value type that contains itself directly or transitively — a
`struct Node` with a `next: Node` field, or a `some(Self)` field — is an infinite
value and is a `check.recursion` diagnostic naming the cycle (the independent
verifier re-rejects any such cycle in the image). An unknown field type name is a
`check.unsupported` diagnostic. A missing, unknown, duplicated, or wrong-typed
field argument, and an unnamed (positional) argument, are `check.type`
diagnostics; a struct name that collides with another declared type is a
`check.name_conflict`.

## Generic Types

A `struct` or `enum` may declare rank-1 type parameters in brackets after its name,
making it a generic value type. Each distinct application `Name<Args>` is a separate
monomorphized value type; the type arguments substitute for the parameters in the
fields (a struct) or variant payloads (an enum).

```mw
struct Pair<A, B> {
    first: A
    second: B
}

enum Box<T> {
    empty
    full(value: T)
}
```

A type parameter is a bare name, optionally carrying one closed constraint —
`T supports equality` or `T supports order` — spelled the same way as on a
[generic function](modules-and-functions.md#generic-functions). The constraint
licenses `==`/`!=` (equality) or `<`/`>` and equality (order) over the parameter,
and every application revalidates that the concrete argument supports it; an
argument that does not is a `check.type`. An unconstrained parameter admits neither
operator over its values.

A generic value is constructed with the ordinary literal spelling; there is no
explicit `Name<Args>` construction form. The type arguments are inferred from the
field or payload values, so every parameter must appear in a value the construction
supplies:

```mw
const p = Pair(first: 7, second: "hello") // Pair<int, string>

const b = Box::full(value: 9) // Box<int>
```

A parameter that no value determines cannot be inferred at the construction site and
is a `check.type`. A generic type is also written in a type annotation
(`Pair<int, string>`, `Box<int>`), which drives the same monomorphization; a field,
parameter, return, or local may name one.

`Option<T>`, `Result<T, E>`, `List<T>`, and `Map<K, V>` are the toolchain's own
generic types over this one mechanism: `Option` and `Result` are generic enums (see
[Option and Result](#option-and-result)), and `List`/`Map` are the compiler
collections. Their names are reserved and cannot be redeclared.

A monomorphized instantiation is a private, image-local value type with no stable
identity; two applications with the same arguments are the same type, and
`Pair<int, string>` and `Pair<string, int>` are distinct. Acyclicity applies per
instantiation: a generic type whose instantiation contains itself — `struct Tree<T>`
with a `child: Tree<T>` field — is an infinite value and a `check.recursion` at the
template, while a self-reference broken by a collection (`kids: List<Tree<T>>`) is
finite and admitted. A program monomorphizes finitely many instantiations; a
divergent generic that nests inside itself over an ever-growing argument exceeds the
fixed instantiation bound and is a `check.instantiation_limit`.

## Lists And Maps

`List<T>` is a finite ordered collection of values of type `T`, and `Map<K, V>`
is a finite ordered map from keys of type `K` to values of type `V`. Both are
ordinary copied values with the same value semantics as every other type: passing,
returning, or reassigning one copies it, and there is no aliasing or shared
mutation. The element type `T` and the value type `V` may be any value type,
including a nested `List`, `Map`, `struct`, `enum`, or `Option`/`Result`. A map key
`K` is drawn from the ordered key scalars — `int`, `bool`, `string`, `bytes`, or a
nominal int type; a struct, enum, collection, or `decimal` key is a
`check.unsupported`. Collections are values, never durable storage: a resource
field or store key is not a `List` or `Map`.

An empty collection is constructed with `List()` or `Map()`, whose element and
key/value types come from the expected type (an annotation, argument, return type,
or coercion). The closed set of procedural collection operations is small — there
is no method syntax:

| Form | Result |
|---|---|
| `List()` / `Map()` | an empty collection of the expected type |
| `append(list, value)` | the list with `value` added after the last element |
| `length(collection)` | the element or entry count as an `int` |
| `isEmpty(collection)` | whether the collection has no elements |

Because collections are values, `append` yields an updated collection rather than
mutating in place; a `var` binding is reassigned to keep it.

### Bracket lookup and assignment

A collection is read and a map is written with bracket syntax, the same keyed
spelling as a durable place — local and durable data obey one presence algebra,
and the `^` alone marks which touches the store. A bracket read yields the
presence-typed optional consumed by `??`, `if const`, let-else, and an `else`
clause; there is no out-of-bounds fault class.

- `xs[i]` is the element at list position `i`, typed `T?`. List positions are
  1-based: `xs[1]` is the first element and `xs[length(xs)]` is the last. A
  position outside `1..=length(xs)` reads `absent`. The literal dead indexes
  `xs[0]` and `xs[-1]` are refused at check time with a `check.type` diagnostic,
  because a literal `0` or negative names no position.
- `m[k]` is the value stored at key `k`, typed `V?` — present when the key is in
  the map, `absent` otherwise. A `Map<int, V>` key of `0` is an ordinary key, not a
  dead index.
- `m[k] = value` on a `var` map binding creates or replaces the value at `k`. It is
  total except the `run.collection_limit` growth bound, and lowers as a
  read-modify-write with value semantics. A `const` binding is not reassignable.
  A list has no keyed write: `xs[i] = value` is a `check.type` diagnostic naming
  `append(xs, value)` for growth and `Map<int, T>` for replacement at a position.

Removing a map key (`unset m[k]`) and a nested bracket target (`outer[k1][k2] =
value`) are not yet admitted.

```mw
module docs::collections

pub fn total(): int {
    var xs: List<int> = List()
    xs = append(xs, 10)
    xs = append(xs, 20)
    var sum: int = 0
    for x in xs {
        sum += x
    }
    return sum
}

pub fn firstOr(xs: List<string>, fallback: string): string {
    return xs[1] ?? fallback
}

pub fn lookup(name: string): int {
    var scores: Map<string, int> = Map()
    scores["ada"] = 10
    return scores[name] ?? 0
}
```

A `List` iterates its elements in insertion order. A `Map` iterates in ascending
key order: `for key in map` binds each key, and `for key, value in map` binds each
key and its value. Map keys use the same typed order as durable traversal —
numeric keys ascend, `false` precedes `true`, and strings and bytes use
lexicographic order.

A collection has fixed representational bounds: at most 65536 elements and at most
1 MiB of aggregate value size. An `append` or a `m[k] = value` insert that would
exceed either faults `run.collection_limit` rather than allocating unboundedly.

## Key Types

Store identity columns, resource keyed layers, and local keyed collections
accept `int`, `bool`, `string`, `bytes`, `date`, `instant`, and `duration` keys.
The `ErrorCode` spelling is also accepted, but current key compilation erases
its refinement: the key behaves as `string` and is not grammar-validated.
`decimal`, enums, entry identities, resources, collections, optionals, and
`unknown` are not accepted in those key positions. A declared index component
projects that same closed orderable-key scalar set — `int`, `bool`, `string`,
`bytes`, `date`, or `instant`, a nominal erasing to its base scalar — so an
enum, an entry identity, `decimal`, and every non-scalar field are not
index-eligible.

Key order is part of observable traversal behavior. Numeric and temporal keys
use their natural order, `false` precedes `true`, and strings and bytes use
lexicographic order. Composite key tuples are ordered lexicographically by
column.

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
