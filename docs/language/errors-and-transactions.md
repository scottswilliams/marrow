# Errors and transactions

A `transaction` block groups the durable writes of one export so that they
commit together. A failure the program handles is an ordinary `Result<T, E>`
value ([Option and Result](types-and-values.md#option-and-result)); every other
failure stops the invocation and reports a code. Today, an invocation runs once,
on its own. Concurrent execution is future work
([served execution](../future/served-execution.md)).

## Transactions

A mutating export owns one `transaction` block:

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
        const current = ^books[id].loans ?? 0
        ^books[id].loans = current + 1
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

`add` and `bump` each own one block. In `bump`, the read and the write of
`loans` sit in the same block, so the two calls in the test commit one after
the other. `loans` only reads and needs no block. The test drives the exports,
and each call is its own invocation.

A durable write sits inside a `transaction` block, or inside a helper the block
calls. A write in an export body outside any block is
`check.requires_transaction`. A helper runs inside the block of the export that
calls it and carries no block of its own (`check.transaction_misplaced`). An
export that owns a block is called only from a [test](tests.md) body
(`check.transaction_owner_called`). A read may precede the block.

The block stages its writes. A read inside the block sees writes staged earlier
in the same block. The block commits at each of its exits: the closing brace and
every `return` written inside it. A `return` inside the block evaluates its
value, then commits, then returns, so `return ^books[id].loans` returns the
staged value. An export opens its block once (`check.transaction_reopened`), the
block touches at least one durable place (`check.transaction_empty`), and no
durable read or write follows the commit (`check.durable_after_commit`).

## Guards inside a block

A deliberate failure is a `return err(...)` placed before the first write:

```mw
module docs::errors::guard

resource Book {
    required title: string
    copies: int
}

store ^books[id: int]: Book

pub fn add(id: int, title: string, copies: int): Result<int, string> {
    transaction {
        if copies < 0 {
            return err("copies cannot be negative")
        }
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

Every `return` inside the block commits, whatever value it carries. A
`return err(...)` placed after a write commits that write, so the guard goes
before the write.

[Prefix `try`](control-flow.md#prefix-try) and a
[`require` guard](control-flow.md#require-guards) leave the function without
committing. In an export that owns a block, neither stands on a path before the
commit, inside the block or ahead of it; the report is
`check.transaction_uncommitted` at the `try` or the `require`. A helper called
inside the block owns no block, so its `try` and `require` keep their ordinary
meaning.

## Rollback and isolation

A fault inside the block discards every staged write:

```mw
module docs::errors::rollback

resource Book {
    required title: string
    copies: int
}

store ^books[id: int]: Book

pub fn faultBeforeCommit(id: int, divisor: int): int {
    transaction {
        ^books[id] = Book(title: "Small Gods")
        return 1 / divisor
    }
}

pub fn faultAtCommit(id: int, copies: int) {
    transaction {
        ^books[id].copies = copies
    }
}

pub fn faultAfterCommit(id: int, divisor: int): int {
    transaction {
        ^books[id] = Book(title: "Small Gods")
    }
    return 1 / divisor
}
```

Three tests call `faultBeforeCommit(1, 0)`, `faultAtCommit(2, 3)`, and
`faultAfterCommit(3, 0)` against a fresh store. `marrow test` reports them in
name order, each with the line and column of the faulting operation in the
module above:

```text
ERROR fault after commit (run.divide_by_zero at 27:16; incomplete, durable known_new)
ERROR fault at commit (run.required_missing at 18:17; incomplete, durable known_old)
ERROR fault before commit (run.divide_by_zero at 13:20)
0 passed, 0 failed, 3 errored (3/3 selected)
```

`faultBeforeCommit` faults on the division before the block commits. The staged
entry is discarded and the report carries the fault alone. `faultAtCommit` sets
a sparse field of an absent book; at commit the required `title` is unset, so
the block rolls back with `run.required_missing` and durable state `known_old`.
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
