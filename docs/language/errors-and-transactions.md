# Errors and transactions

A `transaction` block groups the durable writes of one export so that they
commit together. A failure the program handles is an ordinary `Result<T, E>`
value ([Option and Result](types-and-values.md#option-and-result)); every other
failure stops the invocation and reports a code. Today, an invocation runs once,
on its own. Concurrent execution is future work
([served execution](../future/served-execution.md)).

## Transactions

A mutating export groups its durable writes in a `transaction` block:

```mw
module docs::errors::bump

resource Book {
    required title: string
    loans: int
}

store ^books[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn bump(id: int) {
    transaction {
        ref m = ^books[id] else {
            return
        }
        m.loans = (m.loans ?? 0) + 1
    }
}

pub fn loans(id: int): int? {
    return ^books[id].loans
}

test "each call commits one increment" {
    add(1, "Small Gods")
    bump(1)
    bump(1)
    assert loans(1) ?? 0 == 2
}
```

`add` and `bump` each own one block. In `bump`, the `ref` binding checks entry
presence, and the read and the write of `loans` through the reference sit in the
same block, so the two calls in the test commit one after the other
([entry references](durable-data.md#entry-references)). `loans` only reads and needs no block. The test drives the exports,
and each call is its own invocation.

A durable write sits inside a `transaction` block, or inside a helper the block
calls. A write in an export body outside any block is
`check.requires_transaction`. A helper runs inside the block of the export that
calls it and carries no block of its own (`check.transaction_misplaced`). An
export that owns a block is called only from a [test](tests.md) body
(`check.transaction_owner_called`). A read may precede the block.

The block stages its writes. A read inside the block sees writes staged earlier
in the same block. The block commits at its closing brace and every normal
function exit inside it, including `return`, `try` failure and `require` failure.
An exit inside the block evaluates its value, then commits, then returns, so
`return ^books[id].loans` returns the staged value. An export begins its region
at most once on any path (`check.transaction_reopened`), the block touches at
least one durable address (`check.transaction_empty`), and no durable read or
write follows the commit (`check.durable_after_commit`).

Paths that meet agree on whether the block has run. A block in one arm of an
`if` or `match` whose arm continues past it is `check.transaction_conditional`;
return before the block when its work does not apply, or end the arm with
`return` after the block. A block in a loop body that a later iteration reaches
again is `check.transaction_reopened`; move the loop inside the block. A path
that returns, inside or after the block, meets no other path, so an arm that
ends with `return` after its block is legal. A `break` or `continue` that leaves
the block before it commits is `check.transaction_uncommitted`.

```mw
module docs::errors::paths

resource Book {
    required title: string
    loans: int
}

store ^books[id: int]: Book

pub fn shelve(id: int, title: string, restock: bool): string {
    if restock {
        transaction {
            ^books[id] = Book(title: title, loans: 0)
        }
        return "restocked"
    }
    transaction {
        ^books[id] = Book(title: title)
    }
    return "added"
}

pub fn lend(id: int, allowed: bool): bool {
    if not allowed {
        return false
    }
    transaction {
        ref m = ^books[id] else {
            return false
        }
        m.loans = (m.loans ?? 0) + 1
    }
    return true
}

pub fn loans(id: int): int? {
    return ^books[id].loans
}

test "each path begins the region at most once" {
    assert shelve(1, "Mort", true) == "restocked"
    assert shelve(2, "Eric", false) == "added"
    assert lend(1, false) == false
    assert lend(1, true)
    assert loans(1) ?? 0 == 1
    assert loans(2) ?? 0 == 0
}
```

`shelve` holds two blocks, and each path through it begins one: the `restock`
arm returns after its block, so it never meets the path that reaches the second
block. `lend` returns before its block when lending is not allowed, so every path
that continues past the `if` has not yet begun the region.

## Guards inside a block

A guard that intends no change precedes the first write:

```mw
module docs::errors::guard

resource Book {
    required title: string
    copies: int
}

store ^books[id: int]: Book

pub fn add(id: int, title: string, copies: int): Result<int, string> {
    transaction {
        require copies >= 0 else "copies cannot be negative"
        ^books[id] = Book(title: title, copies: copies)
    }
    return ok(id)
}

pub fn title(id: int): string? {
    return ^books[id].title
}

test "a rejected call writes nothing" {
    add(1, "Small Gods", -1)
    assert title(1) ?? "" == ""
    add(1, "Small Gods", 2)
    assert title(1) ?? "" == "Small Gods"
}
```

The guard returns from inside the block with nothing staged, so the first call
commits nothing and the test reads no title. The second call passes the guard
and commits the entry.

Every normal function exit inside the block commits, whatever value it carries.
A `return err(...)`, [prefix `try`](control-flow.md#prefix-try) failure or
[`require` guard](control-flow.md#require-guards) failure after a write commits
that write. A `Result` error is a value, not a rollback instruction.

Before the block begins, a normal exit has no staged writes to commit. After
the block commits, a normal exit does not commit again. A helper owns no block:
its return does not commit its caller's transaction. The caller can inspect the
helper's result and continue, or propagate it through its own committing exit.

## Rollback and isolation

A fault inside the block discards every staged write:

```mw
module docs::errors::rollback

resource Book {
    required title: string
    isbn: string
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
}

pub fn faultBeforeCommit(id: int, divisor: int): int {
    transaction {
        ^books[id] = Book(title: "Small Gods")
        return 1 / divisor
    }
}

pub fn uniqueWriteFault(id: int, isbn: string) {
    transaction {
        ^books[id] = Book(title: "Small Gods", isbn: isbn)
    }
}

pub fn faultAfterCommit(id: int, divisor: int): int {
    transaction {
        ^books[id] = Book(title: "Small Gods")
    }
    return 1 / divisor
}
```

Three tests call `faultBeforeCommit(1, 0)`, then `uniqueWriteFault(1, "111")`
followed by `uniqueWriteFault(2, "111")`, then `faultAfterCommit(3, 0)`, each
against a fresh store. `marrow test` reports them in name order, each with the
line and column of the faulting operation in the module above:

```text
ERROR fault after commit (run.divide_by_zero at 29:16; incomplete, durable known_new)
ERROR fault before commit (run.divide_by_zero at 15:20)
ERROR unique write fault (run.unique_index at 21:9)
0 passed, 0 failed, 3 errored (3/3 selected)
```

`faultBeforeCommit` faults on the division before the block commits. The staged
entry is discarded and the report carries the fault alone. `uniqueWriteFault`
writes a second book under an ISBN that the `unique` index already holds. The
write faults with `run.unique_index` before commit and the whole block rolls back,
so the second book is not in place
([index declarations](traversal-and-indexes.md#index-declarations)).
`faultAfterCommit` commits the entry and then faults. The entry stays in place
and the report says `known_new`.

Each invocation is its own boundary. A faulting invocation rolls back only its
own block and leaves every earlier committed invocation intact. A store
condition raised inside the block, such as an I/O failure, rolls the block back
the same way and reports its `store.*` code.

## Failure kinds

A program fails in one of four ways. A `Result` is a value and is none of them.

| Kind | When | Examples |
|---|---|---|
| Source diagnostic | the source does not parse or check | `parse.syntax`, `check.type` |
| Image rejection | a compiled image fails verification and does not run | `image.flow`, `image.envelope` |
| Runtime fault | a running invocation stops at one operation | `run.overflow`, `run.divide_by_zero` |
| Operational error | the command or the store fails | `store.locked`, `io.read` |

Every report carries a dotted code. A source diagnostic and a runtime fault also
carry a source position. A runtime fault stops the invocation; the program has
no way to catch it. [Error codes](../error-codes.md) lists every code with its
meaning.

## Interrupted invocations

An invocation that faults without returning also reports its durable state.
`known_old` means the block changed nothing. `known_new` means its writes are in
place. `unknown` means the store could establish neither; the store settles it
when it is next opened
([interrupted commits](../operations/README.md#interrupted-commits)).
