# Built-ins

A built-in is a function or value available in every module without `use`.
There are few of them, and each does one thing.

| Group | Form | Result |
|---|---|---|
| Collections | `List()`, `List(a, b, c)`, `Map()` | An empty or filled collection |
| | `append(xs, x): List<T>` | `xs` with `x` added at the end |
| | `length(xs): int` | The element count of a list or map |
| | `isEmpty(xs): bool` | Whether a list, map, or string is empty |
| Text | `contains(text, part): bool` | Whether `part` occurs in `text` |
| | `trim(text): string` | `text` without leading and trailing whitespace |
| | `split(text, separator): List<string>` | The pieces of `text` between separators |
| | `lines(text): List<string>` | The lines of `text` |
| | `join(parts, separator): string` | `parts` concatenated with `separator` between them |
| Numbers | `maxInt`, `minInt` | The largest and smallest `int` |
| Dates and times | `date("…")`, `instant("…")`, `duration("…")` | A temporal value from its canonical text |
| | `addDays(d, n): date` | The date `n` days after `d` |
| | `daysBetween(a, b): int` | The signed number of days from `a` to `b` |
| Presence and identity | `exists(place): bool` | Whether a durable place is present |
| | `Id(^root, key): Id(^root)` | The identity of an entry under `^root` |
| Conversion and output | `string(value): string` | The canonical text of a scalar, enum value, or identity |
| | `bytes(text): bytes` | The UTF-8 bytes of `text` |
| Faults | `unreachable("…")`, `todo("…")` | A statement that stops the program |
| Option and Result | `some(v)`, `none`, `ok(v)`, `err(e)` | A member of `Option<T>` or `Result<T, E>` |

Every name in the table except `append` and `length` is reserved. `Id`,
`string`, `bytes`, `date`, `instant`, and `duration` are keywords, so a
declaration that reuses one is a `parse.syntax` error; a function, constant,
parameter, or local named after any other reserved name is a
`check.name_conflict` error. A module may declare its own `append` or
`length`, and that function is used throughout the module.

## Collections

`List` and `Map` are values. `append` yields a new list and leaves its
argument unchanged, so the result is assigned back:

```mw
module docs::builtins::collections

pub fn shelves(): List<string> {
    var names = List("fiction", "history")
    names = append(names, "travel")
    return names
}

test "collections are values" {
    const names = shelves()
    assert length(names) == 3
    assert names[3] ?? "" == "travel"
    assert not isEmpty(names)
}
```

`List("fiction", "history")` takes its element type from the first argument.
`names[3]` is the third element, because positions start at 1, and it is a
`string?` because the position may be past the end. A `Map()` starts empty and
is filled with `m[k] = v`. The rest of the collection rules are under
[lists and maps](types-and-values.md#lists-and-maps).

## Text

Five text functions are built in, and `isEmpty` accepts a string as well as a
collection. There is no string library beyond them.

```mw
module docs::builtins::text

pub fn authors(line: string): List<string> {
    return split(trim(line), ", ")
}

test "text functions" {
    const names = authors(" Pratchett, Gaiman ")
    assert length(names) == 2
    assert join(names, " & ") == "Pratchett & Gaiman"
    assert contains(names[1] ?? "", "chett")
    assert length(lines("a\r\nb\n")) == 2
    assert isEmpty(trim("   "))
}
```

`trim` removes Unicode whitespace at both ends. `split` cuts at each
non-overlapping occurrence of the separator, in order; an empty separator
yields the one-element list `[text]`. `lines` cuts at each line feed, drops a
carriage return before a line feed, and adds no empty line after a final line
terminator. `join` concatenates a `List<string>` with the separator between
adjacent elements. `isEmpty` accepts a string as well as a collection;
`length` takes a list or map only. A result honors the same
[text and collection limits](execution-limits.md) as any other value.

## Numbers

`maxInt` is `9223372036854775807` and `minInt` is `-9223372036854775808`. Both
are ordinary `int` values and take no arguments; a source file names the bound
instead of spelling the literal.

```mw
module docs::builtins::bounds

const capacity = maxInt

pub fn hasRoom(count: int): bool {
    return count < capacity
}

test "a bound is a value" {
    assert hasRoom(0)
    assert not hasRoom(maxInt)
    assert minInt + maxInt == -1
}
```

`const capacity = maxInt` is accepted where a constant otherwise holds one
literal. `maxInt(1)` is a `check.type` error.

## Dates and times

`date("…")`, `instant("…")`, and `duration("…")` construct a value from a
literal in its canonical text, described under
[temporal values](types-and-values.md#temporal-values). Date arithmetic is two
named functions:

```mw
module docs::builtins::dates

pub fn dueDate(loaned: date): date {
    return addDays(loaned, 14)
}

test "loan arithmetic" {
    const due = dueDate(date("2026-03-01"))
    assert due == date("2026-03-15")
    assert daysBetween(due, date("2026-03-20")) == 5
    assert daysBetween(due, date("2026-03-10")) == -5
}
```

`addDays` takes a signed count, and `daysBetween` returns a signed count. A
result outside years 0001 through 9999 faults `run.temporal_overflow`. There
is no clock built-in: the current day or instant is passed in as an argument.

## Presence and identity

`exists(place)` reports whether a durable place is present and yields a
`bool`. Its argument is a `^` path: a store root, an entry, a field, a keyed
branch family, or a complete key of a `unique` index. A local optional is
resolved with `??`, `if const`, or `?.` instead, described under
[optionals](types-and-values.md#optionals).

```mw
module docs::builtins::presence

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn add(id: Id(^books), title: string) {
    transaction {
        ^books[id].title = title
    }
}

pub fn known(id: Id(^books)): bool {
    return exists(^books[id])
}

test "presence" {
    const id = Id(^books, 7)
    assert not known(id)
    add(id, "Small Gods")
    assert known(id)
}
```

`Id(^books, 7)` wraps a key as an identity and reads nothing: the entry is
absent until `add` commits. `exists(^books[id].subtitle)` asks about one
sparse field. `exists(^books.byIsbn[isbn])` asks a unique index whether some
entry carries that key. `exists` narrows nothing; a read after it is still
optional. Identity as a type is described under
[entry identity](types-and-values.md#entry-identity) and index reads under
[reading an index](traversal-and-indexes.md#reading-an-index). An application
that needs a fresh key keeps its own durable counter
([counter allocation](idioms.md#counter-allocation)).

## Conversion and output

`string(value)` renders a scalar, an enum value, or an entry identity as its
canonical text. `bytes(text)` encodes a string as UTF-8. There are no implicit
conversions.

An enum renders as `Enum::member`, bytes as lowercase hexadecimal with a `0x`
prefix, a temporal value as its canonical text, and an identity as `Id(7)`,
without its root. `string(bytes("hi"))` is `"0x6869"`. `marrow run` prints an
export's result in this same canonical text. `string(...)` and interpolation
use the same scalar, enum, and identity renderings but reject bare aggregates
and presence optionals. Rendering a list, map, struct, or optional is not
available today ([conversion](types-and-values.md#conversion)). Three exports
returning a `Shelf` enum value, an `Id(^books)`, and a `date` print:

```text
$ marrow run shelf
Shelf::history
$ marrow run ident
Id(7)
$ marrow run due
2026-03-15
```

## Faults

`unreachable("…")` and `todo("…")` are statements that stop the program with
`run.unreachable` or `run.todo`, carrying their text. Each takes one string
literal, described under [divergence](control-flow.md#divergence).

```text
$ marrow run never
run.unreachable at 23:5: no path reaches here
```

A recoverable failure is a value. `ok(v)` and `err(e)` construct a
`Result<T, E>`, and `some(v)` and `none` construct an `Option<T>`; a `match`
takes them apart, described under
[Option and Result](types-and-values.md#option-and-result).

A `return err($"unknown shelf {name}")` in a function returning
`Result<int, string>` takes its type from the return type. The caller matches
on the result or propagates it with [prefix try](control-flow.md#prefix-try).
The four failure kinds a program can meet are described under
[errors and transactions](errors-and-transactions.md#failure-kinds).

## No standard library

The current toolchain supplies no `std::` modules. An absent module reports
`check.import` and an absent function reports `check.type`; a cross-module
call to a non-public function reports `check.visibility`. A project-declared
`std::` path is project code, not an ambient library. A source standard
library is future work
([source standard library](../future/source-standard-library.md)).
