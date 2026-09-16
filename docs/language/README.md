# The Marrow language

Marrow is a statically typed language in which durable data is ordinary program
state. A place written with `^` outlives the program and is read and written
the way a local value is.

## A first look

A resource, a store, one export that writes, one that reads, and a test:

```mw
module docs::tour::first_look

resource Book {
    required title: string
    read: bool
}

store ^books[id: int]: Book

pub fn add(id: int, title: string): bool {
    transaction {
        if exists(^books[id]) {
            return false
        }
        ^books[id] = Book(title: title)
    }
    return true
}

pub fn titleOf(id: int): string? {
    return ^books[id].title
}

test "a book reads back by its key" {
    assert add(1, "Small Gods")
    assert titleOf(1) ?? "" == "Small Gods"
    assert not add(1, "Small Gods again")
}
```

`resource Book` declares a shape with one required field and one sparse field.
The sparse field `read` is absent until a program assigns it.
`store ^books[id: int]: Book` gives that shape a durable root keyed by an
`int`, so `^books[id]` is one entry and `^books[id].title` is one field of it.
`add` writes inside a `transaction`, and the write commits when the block ends.
`titleOf` returns `string?` because the entry may be absent, and the test proves
both functions against a fresh in-memory store.

## Two kinds of state

A local value and a durable place hold the same shape. The `^` is the
difference:

```text
pub fn finish(id: int, title: string) {
    var book = Book(title: title)
    book.read = true
    transaction {
        ^books[id] = book
    }
}
```

The first two lines of `finish` change a local value that is gone when the call
returns. The assignment inside the block copies it to `^books[id]`, where it is
still there on the next run. Both sides of that assignment have the type
`Book`. Nothing stands between the code and the data.

## Absence is a type

A durable read yields `T?`, which holds a present `T` or `absent`. The program
says what happens when the value is not there:

```text
pub fn label(id: int): string {
    if const title = ^books[id].title {
        return title
    }
    return "no such book"
}

pub fn isRead(id: int): bool {
    return ^books[id].read ?? false
}
```

`if const title = ^books[id].title` binds `title` only when the field is
present. `?? false` supplies a default when it is absent. `exists(^books[id])`,
in the first look, asks the question directly and yields a `bool`. For an entry
nobody wrote, `label` answers `"no such book"` and `isRead` answers `false`,
without a fault.

## Writes commit together

Every durable write sits inside a `transaction` block, and a mutating export
owns one such block. When the block ends, its writes commit together:

```mw
module docs::tour::commit

resource Book {
    required title: string
}

resource Tally {
    required count: int
}

store ^books[id: int]: Book

store ^tallies[name: string]: Tally

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
        place tally = ^tallies["books"]
        tally = Tally(count: (tally.count ?? 0) + 1)
    }
}

pub fn count(): int {
    return ^tallies["books"].count ?? 0
}

test "each add advances the tally" {
    add(1, "first")
    add(2, "second")
    assert count() == 2
}
```

`add` writes to two roots, and either both writes become durable or neither
does. The tally is written as a whole entry through a place, so the first call
creates it and each later call replaces it. A fault before the block ends rolls back every write in it. A `return`
inside the block commits what was written before it, so `add` in the first look
leaves a duplicate key untouched. A durable write in an export body outside a
`transaction` block is a compile error, `check.requires_transaction`; a test
body reaches durable data only through the exports it calls ([tests](tests.md)).

## Traversal states its bound

A loop over a durable root names how many entries it visits and what happens
when more remain:

```text
pub fn count(): int {
    var n = 0
    for id in ^books at most 100 {
        n += 1
    } on more {
        n = -1
    }
    return n
}
```

`at most 100` caps the walk at one hundred keys, visited in key order. The
`on more` block runs when a hundred-and-first key exists. A durable loop with no
bound is a compile error, so a whole root is only ever read on purpose.

## Every test starts in a fresh store

A `test` that touches durable data runs against its own empty in-memory store.
Added to the first look's module, these two tests both add key `1`:

```text
test "this test starts empty" {
    assert add(1, "first")
}

test "so does this one" {
    assert add(1, "first")
}
```

Both pass. A test needs no fixture and no
cleanup, and no test observes another's writes. A body sets data up through
exports that own a `transaction` and observes it through ordinary read
functions; a durable operation written in the body itself is
`check.test_durable_operation` ([tests](tests.md)).
`marrow test` runs every test in the project:

```text
$ marrow test
ok    so does this one
ok    this test starts empty
2 passed, 0 failed, 0 errored (2/2 selected)
```

## Marks

A mark means consequence. `^` is the one spelling of a durable place,
`transaction {` of a commit, `at most` of a bound, and `delete` of removal.
Grep `\^` and you have every point where durable data enters the code; grep
`transaction {` and you have every commit. [Marks](idioms.md#marks) lists the
whole set.

## Core terms

A place is a location a program reads by naming it: a variable, a field of a
local value, a collection element, or a durable path. The keyword `place`
binds a name to one durable entry address
([named places](durable-places.md#named-places)). A list element is
read-only; every other place is also assigned by naming it. A resource is a
declared value shape whose fields are sparse unless marked `required`. A durable
place is a path that begins with a declared store root. An entry identity,
`Id(^books)`, names one entry of one root and belongs to that root alone.
Presence is whether a value exists at a place; `T?` carries a present `T` or
`absent`. A transaction is a block whose durable changes commit together or roll
back together.

## Known gaps

These constructs parse and then refuse. Each is stated where it belongs; this
table is the one place to see them together.

| Construct | Today | Defined in |
|---|---|---|
| A decimal literal such as `12.50`, and the `decimal` type | `check.unsupported` | [Source and syntax](source-and-syntax.md), [Types and values](types-and-values.md) |
| A byte literal such as `b"Marrow"` | `check.unsupported`; use `bytes("Marrow")` | [Source and syntax](source-and-syntax.md) |
| A struct, list, map, or optional in an interpolation hole | `check.unsupported` | [Source and syntax](source-and-syntax.md) |
| A computed argument to a temporal literal | `check.unsupported`; the argument is a literal | [Types and values](types-and-values.md) |
| A nested bracket write, `outer[k1][k2] = value` | `check.unsupported` | [Types and values](types-and-values.md) |
| A collection enum payload, `Option<List<int>>` or `m(v: List<int>)` | `check.unsupported`; wrap the collection in a struct | [Types and values](types-and-values.md#enums) |
| An optional enum payload, `m(v: int?)` | `check.unsupported` | [Types and values](types-and-values.md#enums) |
| A resource as a type argument, `Option<Book>` or `List<Book>` | `check.unsupported` | [Resources](resources.md) |
| An optional parameter, `book: Book?` | `check.unsupported` | [Resources](resources.md) |
| A public aggregate parameter containing a nominal int | `check.unsupported` | [Types and values](types-and-values.md#aliases-and-nominal-ints) |
| A resource containing a nominal value bound to a store | `check.unsupported` | [Durable places](durable-places.md) |
| A nominal type as a store-root key, branch key, or module constant | `check.unsupported` | [Types and values](types-and-values.md#aliases-and-nominal-ints) |
| A call pairing two scalar names, `int("1")` or `bool(1)` | `check.unsupported` | [Types and values](types-and-values.md) |
| A type or constructor path longer than `alias::Name`, `graphtext::text::Pair` | `check.unsupported`; a type name is one or two segments | [Modules and functions](modules-and-functions.md#dependencies) |
| An enum path longer than that name plus one member, `graphtext::text::Color::red` | `check.unsupported` | [Modules and functions](modules-and-functions.md#dependencies) |
| An expression, call, `bytes`, or temporal value in a module `const` | `check.unsupported` | [Modules and functions](modules-and-functions.md) |
| `delete` on a local field | `check.unsupported`; `unset` clears one | [Grammar](grammar.md) |
| `for` over a composite-keyed root or branch | `check.unsupported`; walk a single-key branch | [Traversal and indexes](traversal-and-indexes.md) |
| An index walk taking `from` or a pin | `check.unsupported` | [Traversal and indexes](traversal-and-indexes.md) |
| A singleton root, `store ^settings: Settings`, and a group inside a group or branch | Declares and checks; operations are future work | [Durable places](durable-places.md), [status](../status.md#not-yet-available) |

## Reading order

The chapters build on one another:

- [Source and syntax](source-and-syntax.md): files, modules, literals, blocks,
  operators, and paths.
- [Types and values](types-and-values.md): scalars, optionals, structs, enums,
  `Option` and `Result`, lists and maps, generics, and entry identity.
- [Modules and functions](modules-and-functions.md): functions, generic
  functions, imports, visibility, and constants.
- [Control flow](control-flow.md): conditionals, let-else, loops, `match`,
  checked arithmetic, `require`, and `try` for `Result` propagation.
- [Resources](resources.md): fields, groups and branches, and local resource
  values.
- [Durable places](durable-places.md): store roots, keys, reads, writes, named
  places, deletion, and access demand.
- [Errors and transactions](errors-and-transactions.md): transaction blocks,
  guards inside a block, rollback, and the four failure kinds.
- [Traversal and indexes](traversal-and-indexes.md): bounded traversal, ranges,
  index declarations, and reading an index.
- [Tests](tests.md): `test` and `assert`, and durable tests.
- [Idioms](idioms.md): how Marrow is written, and the marks.

The appendices are for lookup:

- [Built-ins](builtins.md): the functions available without an import.
- [Execution limits](execution-limits.md): the fixed bounds.
- [Grammar](grammar.md): the syntax in EBNF.
- [Sample](sample.md): one complete module that uses most of the above.
