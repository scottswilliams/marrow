# Types and values

Every Marrow value is copied when it is passed, returned, or assigned, and
absence has one model, `T?`. A scalar,
a struct, an enum, a list, a map, and a resource all copy on assignment, on a
call, and on return. A sparse field, a bracket lookup, and a durable read all
yield `T?`, and one set of forms consumes it.

Two values and two absences:

```mw
module docs::types::first

struct Pos {
    x: int
    y: int
}

fn shifted(p: Pos): Pos {
    var moved = p
    moved.x = moved.x + 1
    return moved
}

test "a value is copied" {
    const p = Pos(x: 1, y: 2)
    const q = shifted(p)
    assert p.x == 1
    assert q.x == 2
}

test "absence is one model" {
    const xs = List(10, 20)
    const third = xs[3] ?? 0
    assert third == 0
}
```

`shifted` changes its own copy of `p`; the caller's `p` keeps `x == 1`. `xs[3]`
names a position the list does not hold, so it is an `int?` and `?? 0` supplies
the value. A durable read `^books[id].title` has the same shape and the same
optional result; the `^` marks the durable one.

Marrow is statically typed. An expression has one type, and assignment,
arguments, and return values take that type exactly. There is no implicit
numeric or text conversion.

## Scalars

| Type | Values |
|---|---|
| `int` | signed 64-bit integers |
| `bool` | `true` or `false` |
| `string` | UTF-8 text |
| `bytes` | byte sequences |
| `date` | calendar days in years 0001 through 9999 |
| `instant` | UTC instants in years 0001 through 9999, to the nanosecond |
| `duration` | signed elapsed nanoseconds |

The bounds of `int` are the built-in values `maxInt` and `minInt`. Literal forms
for every scalar are on the [syntax](source-and-syntax.md) page. Today, these
seven scalars are the whole set. `decimal` is future work
([status](../status.md)).

## Temporal values

`date`, `instant`, and `duration` are plain values with a fixed representation
and a total order. They depend on no clock, time zone, or locale.

A temporal value is constructed from one string literal in its canonical form:

| Constructor | Canonical text | Example |
|---|---|---|
| `date("…")` | `YYYY-MM-DD` | `date("2026-07-15")` |
| `instant("…")` | `YYYY-MM-DDTHH:MM:SS[.fraction]Z` | `instant("2026-07-15T17:00:00Z")` |
| `duration("…")` | `[-]PT<seconds>[.fraction]S` | `duration("PT3600S")` |

A whole-unit `duration` also has a word literal: an integer followed by
`second`, `minute`, `hour`, `day`, or `week`, singular or plural. `3 days` is
`duration("PT259200S")`. Months and years have no fixed span, so `1 month` is a
parse error. A malformed form, an impossible date such as `date("2021-02-29")`,
and a year outside 0001 through 9999 are each a `check.type` at the literal. The
argument is a literal; a computed argument is a `check.unsupported`.

Temporal arithmetic is a short list. Two values of one temporal type compare with
`==`, `!=`, `<`, `<=`, `>`, and `>=`. A `duration` adds to or subtracts from a
`duration` or an `instant`. `addDays(d, n)` and `daysBetween(a, b)` are the
`date` operations, described under [builtins](builtins.md#dates-and-times).

```mw
module docs::types::temporal

pub fn dueDate(assigned: date, leadDays: int): date {
    return addDays(assigned, leadDays)
}

pub fn isOverdue(due: date, onDay: date): bool {
    return due < onDay
}

pub fn reminderAt(deadline: instant, lead: duration): instant {
    return deadline - lead
}

test "temporal values" {
    assert dueDate(date("2026-07-15"), 10) == date("2026-07-25")
    assert isOverdue(date("2026-07-15"), date("2026-08-01"))
    assert 3 days == duration("PT259200S")
    const start = instant("2026-07-15T17:00:00Z")
    assert reminderAt(start, 1 hour) == instant("2026-07-15T16:00:00Z")
}
```

`isOverdue` takes the current day as an argument. There is no clock in the
language, so a program receives the day or instant it reasons about. A result
outside the type's range faults `run.temporal_overflow`.

## Optionals

`T?` holds a present `T` or `absent`. Optional types do not nest: `int??` is a
parse error, and a struct field holds a bare type. `List<T>?` is an optional
whose value is a whole list.

A sparse field read, a bracket lookup, a durable read, and a function returning
`T?` produce an optional. Four forms consume one. `value ?? fallback` selects the
present value or the fallback. `if const name = value` enters its block with
`name` bound to the present value. A [let-else](control-flow.md#let-else-bindings)
binding diverges when the value is absent. `value?.field` reads a field through
an optional struct or resource and yields an optional.

`exists(place)` is not one of them: it tests a durable place and yields a `bool`.
It narrows nothing, and the read after it is still `T?`.

```mw
module docs::types::optionals

struct Pos {
    x: int
    y: int
}

fn origin(present: bool): Pos? {
    if present {
        return Pos(x: 0, y: 0)
    }
    return absent
}

pub fn describe(present: bool): string {
    if const p = origin(present) {
        return $"at {p.x}"
    }
    return "nowhere"
}

pub fn xOr(present: bool, fallback: int): int {
    const p = origin(present) else {
        return fallback
    }
    return p.x
}

test "optionals" {
    assert describe(true) == "at 0"
    assert xOr(false, 9) == 9
    const x = origin(false)?.x
    assert x ?? 9 == 9
}
```

`describe` binds `p` as a bare `Pos` inside the `if const` block. `xOr` binds
`p` for the rest of the function because the `else` block returns. `?.` on an
absent `origin(false)` yields an absent `int?` without faulting. The
optional-producing call runs once in each form.

A durable read is optional even when the field is `required`, because the entry
itself may be absent. `^books[id].title` is a `string?`, and
`if const book = ^books[id]` binds a whole `Book` whose required fields are
bare. Reading is described under [durable places](durable-places.md#reading).

## Structs

A `struct` is a value with named fields, all required. A field holds any value
type: a scalar, a nominal int, another struct, an enum, an `Option` or `Result`,
or a `List` or `Map`.

```mw
module docs::types::structs

struct Point {
    x: int
    y: int
}

struct Segment {
    from: Point
    to: Point
}

fn length(s: Segment): int {
    return s.to.x - s.from.x
}

test "a struct is built by name" {
    const p = Point(x: 3, y: 4)
    const q = Point(y: 4, x: 3)
    assert p.x == q.x
    const s = Segment(from: p, to: Point(x: 10, y: 4))
    assert length(s) == 7
}
```

A struct declares each field name once; a repeated name is a
`check.name_conflict` at the repeat. A struct is constructed by naming every
field once, in any order. A field is read with `.` and yields the field's type. A `var` binding assigns a field with
`s.to = Point(x: 1, y: 1)`. A field may name a struct or enum declared anywhere in
the project, including later in the same file and in another module. A value type that contains itself, directly or through other types,
is a `check.recursion` naming the cycle. Two structs have no `==`; compare their
fields.

A struct name is project-wide and is written bare from any module. A resource
is the durable counterpart: it adds sparse fields, groups, and keyed branches,
and a store may declare it as a root. Resource values are described under
[resources](resources.md#local-values).

## Enums

An `enum` declares a closed set of members. A member is bare or carries named
payload fields.

```mw
module docs::types::enums

enum Shape {
    dot
    circle(radius: int)
    rect(width: int, height: int)
}

pub fn area(s: Shape): int {
    match s {
        dot => {
            return 0
        }
        circle(r) => {
            return 3 * r * r
        }
        rect(w, h) => {
            return w * h
        }
    }
}

test "enum values" {
    assert area(Shape::rect(width: 2, height: 5)) == 10
    assert Shape::circle(radius: 3) == Shape::circle(radius: 3)
    assert Shape::circle(radius: 3) != Shape::dot
}
```

A value is written `Shape::dot` for a bare member and
`Shape::circle(radius: 3)` for a payload member, with the payload fields named.
`==` and `!=` compare the member and its payload. A `match` names every member
once and binds a payload positionally; it is described under
[control flow](control-flow.md#match).

A declared payload field is a scalar. A struct or enum reaches a payload through
a type parameter of a [generic enum](#generic-types). An enum declares each
member once, and a member declares each payload field once; a repeat is a
`check.name_conflict` at the repeated name. An enum name is project-wide, like a
struct name.

## Option and Result

`Option<T>` and `Result<T, E>` are ordinary generic enums the toolchain declares.
`Option<T>` has the members `none` and `some(v)`. `Result<T, E>` has `ok(v)` and
`err(e)`. The four member names are reserved: a function, constant, parameter, or
local that reuses one is a `check.name_conflict`.

`some(v)` infers `Option<T>` from `v`. `none`, `ok(v)`, and `err(e)` take their
type from where they are used: an annotation, an argument, or a return type. A
`match` over `some(v)` and `none` binds the payload positionally, as any enum
does.

`Result<T, E>` models a recoverable failure. `err(e)` carries a value of the
program's own error type, usually an enum. Prefix `try` unwraps an `ok` and
returns an `err` from the enclosing function, as described under
[control flow](control-flow.md#prefix-try).

```mw
module docs::types::result

enum NameError {
    empty
}

fn checkName(name: string): Result<string, NameError> {
    if name == "" {
        return err(NameError::empty)
    }
    return ok(trim(name))
}

pub fn greeting(name: string): Result<string, NameError> {
    const clean = try checkName(name)
    return ok($"hello {clean}")
}

test "result values" {
    const expected: Result<string, NameError> = ok("hello ada")
    assert greeting(" ada ") == expected
    const failed: Result<string, NameError> = err(NameError::empty)
    assert greeting("") == failed
}
```

`try checkName(name)` binds `clean` to the `ok` payload or returns the `err`.
The test annotates `expected` so that `ok("hello ada")` has a type to take.
`==` compares two values of one instantiation exactly. Nested `Option` is
distinct: `none`, `some(none)`, and `some(some(v))` are three values of
`Option<Option<int>>`.

`T?` and `Option<T>` answer different questions. Use `T?` for the presence of a
place, and `Option<T>` when absence is a value the program passes around or
stores in a structure. A sparse field already models absence: an unset field
reads `absent`. Declare a field `Option<T>` only when a stored `none` must be
distinguishable from the field being unset. Such a field reads as
`Option<T>?`, and the program proves presence and then matches:

```mw
module docs::types::three_state

resource Reading {
    measured: Option<int>
}

pub fn describe(r: Reading): string {
    if const stored = r.measured {
        match stored {
            some(v) => {
                return $"measured {v}"
            }
            none => {
                return "recorded as unmeasurable"
            }
        }
    }
    return "not recorded"
}

test "three states" {
    assert describe(Reading(measured: some(7))) == "measured 7"
    assert describe(Reading(measured: none)) == "recorded as unmeasurable"
    assert describe(Reading()) == "not recorded"
}
```

## Lists and maps

`List<T>` is an ordered collection of values of type `T`. `Map<K, V>` is an
ordered mapping from keys of type `K` to values of type `V`. Both are values:
passing, returning, or reassigning one copies it, and a change to one copy does
not reach another. `T` and `V` are any value type, including a nested `List`,
`Map`, struct, enum, or `Option`/`Result`. A map key `K` is `int`, `bool`,
`string`, `bytes`, `date`, `instant`, and `duration`, or a nominal int type. A
nominal Map key retains its source type and uses its base scalar for
representation and ordering. A struct, enum, collection, optional, or entry
identity is not a Map key. `ErrorCode` is not a local Map key.

`List()` and `Map()` construct an empty collection whose type comes from an
annotation, an argument, or a return type. `List(a, b, c)` constructs a list of
those elements, taking `T` from the first and checking the rest against it. A
map is filled with `m[k] = v`. The operations are `append`, `length`, and
`isEmpty`, described under [builtins](builtins.md#collections); there is no
method syntax.

```mw
module docs::types::collections

pub fn total(xs: List<int>): int {
    var sum = 0
    for x in xs {
        sum += x
    }
    return sum
}

pub fn scoreOf(name: string): int {
    var scores: Map<string, int> = Map()
    scores["ada"] = 10
    scores["bob"] = 7
    unset scores["bob"]
    return scores[name] ?? 0
}

pub fn keysJoined(): string {
    var m: Map<string, int> = Map()
    m["b"] = 2
    m["a"] = 1
    var out = ""
    for k, v in m {
        out += $"{k}{v}"
    }
    return out
}

test "lists and maps are values" {
    const xs = List(10, 20)
    const ys = append(xs, 12)
    assert length(xs) == 2
    assert total(ys) == 42
    assert xs[1] ?? 0 == 10
    assert xs[3] ?? 0 == 0
    assert scoreOf("ada") == 10
    assert scoreOf("bob") == 0
    assert keysJoined() == "a1b2"
}
```

`append(xs, 12)` yields a new list and leaves `xs` at two elements. A bracket
read yields `T?`, and there is no out-of-bounds fault. `xs[i]` is the element at
position `i`, and positions are 1-based: `xs[1]` is the first element and
`xs[length(xs)]` the last. A position outside `1..=length(xs)` reads `absent`.
The literal indexes `xs[0]` and `xs[-1]` name no position and are a `check.type`.
`m[k]` is the value at key `k`, typed `V?`, and a `Map<int, V>` key of `0` is an
ordinary key.

`m[k] = value` on a `var` map creates or replaces the value at `k`. `unset m[k]`
removes the entry at `k`, and removing an absent key does nothing. A list has no
keyed write and no keyed removal: `xs[i] = value` and `unset xs[i]` are each a
`check.type` naming `append` or `Map<int, T>`. A nested bracket target
`outer[k1][k2] = value` is a `check.unsupported`.

A list iterates in insertion order. A map iterates in ascending key order, the
order described under [key types](#key-types). `for k in m` binds each key and `for k, v in m` binds
each key with its value, as `keysJoined` shows.

A collection holds at most 65,536 elements and 1 MiB. An `append` or map insert
beyond either bound faults `run.collection_limit`. A collection is a local
value: a resource field and a store key hold no `List` or `Map`, and a keyed
[branch](durable-places.md#keyed-branches) is the durable shape for many
children.

## Generic types

A `struct` or `enum` declares type parameters in angle brackets after its name.
Each application `Name<Args>` is a distinct type: `Pair<int, string>` and
`Pair<string, int>` are different, and two applications with the same arguments
are the same type.

```mw
module docs::types::generics

struct Pair<A, B> {
    first: A
    second: B
}

enum Box<T> {
    empty
    full(value: T)
}

fn unbox(b: Box<int>): int {
    match b {
        empty => {
            return 0
        }
        full(v) => {
            return v
        }
    }
}

test "type arguments are inferred" {
    const p = Pair(first: 7, second: "hello")
    assert p.first == 7
    const b = Box::full(value: 9)
    assert unbox(b) == 9
}
```

The type parameters of one declaration have distinct names, as do its fields
or members; a repeat is a `check.name_conflict` at the repeated name. A generic
value is constructed with the ordinary spelling, and the type arguments are
inferred from the field or payload values. A parameter that no value
determines is a `check.type` at the construction. An annotation names an
application directly: `Pair<int, string>`, `Box<int>`.

A type parameter may carry one constraint, `T supports equality` or
`T supports order`, spelled as on a
[generic function](modules-and-functions.md#generic-functions). The constraint
admits `==` and `!=`, or the comparisons as well, over the parameter. An
argument that lacks the capability is a `check.type` at the construction. An
unconstrained parameter admits neither.

A payload that resolves to a collection, such as `Option<List<int>>`, is a
`check.unsupported`; wrap the collection in a struct. Acyclicity applies per
application: `Tree<int>` whose `child` is a `Tree<int>` is a `check.recursion`,
and `kids: List<Tree<T>>` is finite. `Option`, `Result`, `List`, and `Map` are
the toolchain's generic types over this mechanism, and their names are reserved.

## Aliases and nominal ints

`alias Name = Type` declares a transparent alias. The name denotes exactly its
target wherever a type is written. `alias Count = int` makes `Count` and `int`
the same type.

```mw
module docs::types::aliases

alias Count = int

alias MaybeCount = Count?

fn maybe(present: bool): MaybeCount {
    if present {
        return 1
    }
    return absent
}

pub fn firstOr(present: bool, fallback: Count): Count {
    return maybe(present) ?? fallback
}

test "an alias is its target" {
    const n: Count = 4
    assert n + 1 == 5
    assert firstOr(false, 9) == 9
}
```

Aliases chain, and a cycle is a `check.recursion` at each alias on it. An alias
whose target names no type is a `check.type`, even when unused. An alias is a
type annotation only and has no constructor.

An alias target is a type name, optionally followed by `?`. The name may be
another alias, but the complete chain admits at most one optional layer.
Generic applications and entry identities are not admitted alias targets.
Names inside an alias bind globally. A generic parameter shadows a same-named
alias in the generic declaration's written annotations; it does not change
the meaning of another alias's target.

`type Name: int in lo..hi` declares a nominal type over `int`: a distinct type
whose every value lies in the declared interval. Unlike a transparent `alias`,
the name mints its own identity and constructor. An `int` is not a `Name` and a
`Name` is not an `int`; each conversion point is explicit in the source.

The interval follows the range operators: `in 0..150` admits `0` through `149`,
and `in 0..=150` admits `0` through `150`. Both bounds are `int` literals, and
the interval admits at least one value. `Name(n)` constructs a value and faults
`run.range` when `n` lies outside the interval. `Name.checked(n)` yields a
`Name?` instead. A parameter of nominal type revalidates the interval on entry,
so an export called from the terminal with an out-of-interval `int` faults
`run.range`.

The `supports` clause admits operators over the type. Every operator that yields
a `Name` revalidates the interval:

| Capability | Admits | Result |
|---|---|---|
| `add` | `Name + int`, `int + Name` | `Name`, revalidated |
| `subtract` | `Name - int` | `Name`, revalidated |
| `subtract` | `Name - Name` | plain `int`, no validation |
| `step` | `Name + 1`, `Name - 1` (the literal `1`) | `Name`, revalidated |
| `scale` | `Name * int`, `int * Name` | `Name`, revalidated |

Comparisons between two values of one nominal type need no capability. An
operator the type does not support, or a comparison mixing a nominal with a
plain `int`, is a `check.type` naming the capability it lacks or the operand
types.

```mw
module docs::types::nominal

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

test "a nominal int keeps its interval" {
    assert older(Age(40), 2) == Age(42)
    assert gap(Age(42), Age(40)) == 2
    const missing = tryAge(200) ?? Age(0)
    assert missing == Age(0)
}
```

Alias, nominal, struct, enum, and resource names share one project-wide
namespace; a collision is a `check.name_conflict`. A nominal int type is
admitted as a resource field and is stored as its base `int`. Operations over a
root containing a nominal field remain unimplemented and report
`check.unsupported`. Nominal types are not admitted as store-root keys, branch
keys, or module-constant types; each position reports `check.unsupported`.

## Operators

Operands take the combinations below; there is no implicit widening.

| Form | Operands | Result |
|---|---|---|
| `-value` | `int` | `int` |
| `not value` | `bool` | `bool` |
| `a + b` | `int`; `string`; `duration + duration`; `instant + duration` | the operand type, or `instant` |
| `a - b` | `int`; `duration - duration`; `instant - duration` | the operand type, or `instant` |
| `a * b`, `a / b`, `a % b` | `int` and `int` | `int` |
| `<`, `<=`, `>`, `>=` | matching `int`, `string`, `bytes`, `date`, `instant`, or `duration` | `bool` |
| `==`, `!=` | matching scalars, nominal ints, enums, or identities of one root | `bool` |
| `value in lo..hi`, `value not in lo..hi` | an `int` and an `int` range | `bool` |
| `and`, `or` | `bool` and `bool` | `bool` |
| `optional ?? fallback` | a `T?` and a `T` | `T` |

Structs, resources, lists, and maps have no `==`. `+` on two strings
concatenates, and `<` on strings and bytes compares byte by byte. `and`, `or`,
and `??` evaluate their right operand only when the left leaves the answer open.

`int / int` is integer division truncated toward zero, paired with the
`int % int` remainder so that `a == (a / b) * b + a % b`. A zero divisor faults
`run.divide_by_zero`. `minInt / -1` and `minInt % -1` fault `run.overflow`
because the result is unrepresentable, as does any `int` operation whose
result leaves the 64-bit range. [Checked arithmetic](control-flow.md#checked-arithmetic)
handles those cases as arms.

`value in lo..hi` is `true` when `lo <= value` and `value < hi`; `in lo..=hi`
includes the upper bound, and `not in` is the negation. The value is evaluated
once. `in` does not chain: `a in r in s` is a parse error. The endpoints are
`int` values; range forms are described under
[traversal](traversal-and-indexes.md#ranges).

```mw
module docs::types::operators

pub fn grade(score: int): string {
    if score not in 0..=100 {
        return "out of range"
    }
    if score in 90..=100 {
        return "A"
    }
    return "below A"
}

test "operators" {
    assert grade(95) == "A"
    assert grade(101) == "out of range"
    assert 7 / 2 == 3
    assert -7 / 2 == -3
    assert -7 % 2 == -1
    assert "ab" + "c" == "abc"
    assert "ab" < "b"
}
```

## Conversion

Conversion is explicit and uses call syntax. `string(value)` renders a scalar,
an enum value, or an entry identity in its canonical text. `bytes(text)`
encodes a `string` as UTF-8.

```mw
module docs::types::conversion

enum Color {
    red
    green
}

test "string renders a value" {
    assert string(42) == "42"
    assert string(true) == "true"
    assert string(Color::red) == "Color::red"
    assert string(3 days) == "PT259200S"
    assert string(date("2026-07-15")) == "2026-07-15"
    assert string(bytes("a")) == "0x61"
}
```

The canonical rendering of each value, and the values `string` refuses, are
listed under [built-ins](builtins.md#conversion-and-output). Text interpolation
`$"…{value}…"` uses the same renderings.

A call pairing two current scalar names is a `check.unsupported`.
`int("1")` and `bool(1)` are examples.
`decimal` and `ErrorCode` have no current callable scalar owner, so
`decimal(1)` and `ErrorCode("run.example")` report `check.type`. The temporal
names `date`, `instant`, and `duration` construct a value from a literal and
convert nothing at run time.

## Key types

A key names one element of a collection or a durable place. Local `Map<K, V>`
keys use `int`, `bool`, `string`, `bytes`, `date`, `instant`, and `duration`, or
a nominal int type. A nominal Map key retains its source type and uses its base
scalar for representation and ordering. `ErrorCode` is not a local Map key.

Durable key positions use `int`, `bool`, `string`, `bytes`, `date`, or `instant`;
`duration` and nominal source types are not durable keys. A root or branch may
take several key components, up to 8, and every component is one of those
scalars, as described under [durable places](durable-places.md#keys).

Managed-index key positions use `int`, `bool`, `string`, `bytes`, `date`, or
`instant`, drawn from a root's identity keys or its top-level scalar fields. A
nominal stored field projects through its base scalar. Index declarations are
described under [traversal and indexes](traversal-and-indexes.md#index-declarations).

Key order is the same everywhere: numbers and temporal values ascend, `false`
precedes `true`, and strings and bytes compare byte by byte. A composite key
orders by its first component, then its second, and so on.

## Entry identity

`Id(^root)` is the type of an entry identity under one store root, and it is
tied to that root. `Id(^books)` and `Id(^authors)` are
different types even if both roots use integer keys. A root with several key
components still yields one `Id(^root)` value.

```mw
module docs::types::identity

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn titleOf(id: Id(^books)): string {
    return ^books[id].title ?? "(absent)"
}

test "an identity addresses an entry" {
    ^books[7] = Book(title: "Small Gods")
    const id = Id(^books, 7)
    assert titleOf(id) == "Small Gods"
    assert string(id) == "Id(7)"
    assert not exists(^books[Id(^books, 8)])
}
```

`Id(^books, 7)` wraps a key as an identity. It reads nothing and proves nothing:
`Id(^books, 8)` names an entry that is absent. An identity stands in for a
root's whole key: `^books[id]`, `^books[id].title`, `^books[id].notes[pos]`, and
`place p = ^books[id]` all take one. An identity of another root in that
position is a `check.type`. Two identities of one root compare with `==`, and
`string(id)` renders `Id(7)`. A [unique index](traversal-and-indexes.md#reading-an-index)
yields an identity, and a stored identity keeps addressing its entry after the
entry is deleted.
