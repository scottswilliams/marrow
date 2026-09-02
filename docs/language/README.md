# The Marrow language

Marrow is a statically typed language in which durable data is ordinary program
state. A place written with `^` outlives the program and is read and written
the way a local value is.

## A first look

A resource, a store, one export that writes, one that reads, and a test:

```mw
module docs::tour::first_look

resource Task {
    required title: string
    done: bool
}

store ^tasks[id: int]: Task

pub fn add(id: int, title: string): bool {
    transaction {
        if exists(^tasks[id]) {
            return false
        }
        ^tasks[id].title = title
    }
    return true
}

pub fn titleOf(id: int): string? {
    return ^tasks[id].title
}

test "a task reads back by its key" {
    assert add(1, "write the tour")
    assert titleOf(1) ?? "" == "write the tour"
    assert not add(1, "write it again")
}
```

`resource Task` declares a shape with one required field and one sparse field.
`store ^tasks[id: int]: Task` gives that shape a durable root keyed by an
`int`, so `^tasks[id]` is one entry and `^tasks[id].title` is one field of it.
`add` writes inside a `transaction`, and the write commits when the block ends.
`titleOf` returns `string?` because the entry may be absent, and the test proves
both functions against a fresh in-memory store.

## Two kinds of state

A local value and a durable place hold the same shape. The `^` is the
difference:

```text
pub fn finish(id: int, title: string) {
    var task = Task(title: title)
    task.done = true
    transaction {
        ^tasks[id] = task
    }
}
```

The first two lines of `finish` change a local value that is gone when the call
returns. The assignment inside the block copies it to `^tasks[id]`, where it is
still there on the next run. Both sides of that assignment have the type
`Task`. Nothing stands between the code and the data.

## Absence is a type

A durable read yields `T?`, which holds a present `T` or `absent`. The program
says what happens when the value is not there:

```text
pub fn label(id: int): string {
    if const title = ^tasks[id].title {
        return title
    }
    return "no such task"
}

pub fn isDone(id: int): bool {
    return ^tasks[id].done ?? false
}
```

`if const title = ^tasks[id].title` binds `title` only when the field is
present. `?? false` supplies a default when it is absent. `exists(^tasks[id])`,
in the first look, asks the question directly and yields a `bool`. For an entry
nobody wrote, `label` answers `"no such task"` and `isDone` answers `false`,
without a fault.

## Writes commit together

Every durable write sits inside a `transaction` block, and a mutating export
owns one such block. When the block ends, its writes commit together:

```mw
module docs::tour::commit

resource Task {
    required title: string
}

resource Tally {
    required count: int
}

store ^tasks[id: int]: Task

store ^tallies[name: string]: Tally

pub fn add(id: int, title: string) {
    transaction {
        ^tasks[id].title = title
        ^tallies["tasks"].count = (^tallies["tasks"].count ?? 0) + 1
    }
}

pub fn count(): int {
    return ^tallies["tasks"].count ?? 0
}

test "each add advances the tally" {
    add(1, "first")
    add(2, "second")
    assert count() == 2
}
```

`add` writes to two roots, and either both writes become durable or neither
does. A fault before the block ends rolls back every write in it. A `return`
inside the block commits what was written before it, so `add` in the first look
leaves a duplicate key untouched. A durable write outside a block is a compile
error, `check.requires_transaction`.

## Traversal states its bound

A loop over a durable root names how many entries it visits and what happens
when more remain:

```text
pub fn count(): int {
    var n = 0
    for id in ^tasks at most 100 {
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

A `test` that touches durable data runs against its own empty in-memory store:

```mw
module docs::tour::fresh_store

resource Task {
    required title: string
}

store ^tasks[id: int]: Task

pub fn add(id: int, title: string): bool {
    transaction {
        if exists(^tasks[id]) {
            return false
        }
        ^tasks[id].title = title
    }
    return true
}

test "this test starts empty" {
    assert add(1, "first")
}

test "so does this one" {
    assert add(1, "first")
}
```

Both tests add the same key, and both pass. A test needs no fixture and no
cleanup, and no test observes another's writes. A body either touches `^`
itself or drives exports that own a `transaction` ([tests](tests.md)).
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
Grep `\^` and you have every durable touch in a program; grep `transaction {`
and you have every commit. [Marks](idioms.md#marks) lists the whole set.

## Core terms

A place is a location a program reads by naming it: a variable, a field of a
local value, a collection element, or a durable path. A list element is
read-only; every other place is also assigned by naming it. A resource is a
declared value shape whose fields are sparse unless marked `required`. A durable
place is a path that begins with a declared store root. An entry identity,
`Id(^tasks)`, names one entry of one root and belongs to that root alone.
Presence is whether a value exists at a place; `T?` carries a present `T` or
`absent`. A transaction is a block whose durable changes commit together or roll
back together.

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
