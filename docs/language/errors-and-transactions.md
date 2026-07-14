# Errors And Transactions

Marrow uses typed `Error` resource values for explicit throws and selected
runtime faults. Transactions group durable changes under commit or rollback.

## The `Error` Shape

The built-in shape is:

```text
resource Error
    required code: ErrorCode
    required message: string
    help: string
    data: unknown
```

`code` and `message` are always present. `help` and `data` are sparse.
`ErrorCode` is represented as validated string text; valid codes contain two or
more nonempty lowercase segments separated by dots. Digits and underscores are
also accepted within segments.

```mw
module docs::errors

fn requirePositive(value: int): int
    if value <= 0
        throw Error(
            code: "input.not_positive",
            message: "Expected a positive integer.",
            help: "Pass a value greater than zero.",
        )
    return value

pub fn describe(value: int): string
    try
        return string(requirePositive(value))
    catch err: Error
        return $"{err.code}: {err.message}"
```

`Error(...)` uses named resource-constructor arguments. A dynamic string can be
validated explicitly with `ErrorCode(text)` before it reaches `code`.

## Throwing And Catching

`throw expression` requires an `Error` value and stops the current control path.
A `try` block has one following `catch` block:

```text
try
    work()
catch err: Error
    recover(err)
```

The annotation may be omitted; the binding is still `Error`. A catch block may
return, continue normally, or rethrow with `throw err`.

Application throws and runtime faults classified as catchable become `Error`
values. Not every failure is catchable. Failure to attach or validate durable
data, detected corruption, and internal integrity failures terminate the
operation instead of entering application recovery code.

## Transactions

A mutating export owns exactly one explicit lexical `transaction` block. Every
durable write reachable from the export sits inside that block, and the export's
call graph joins it implicitly: a helper the export calls performs its writes
inside the owner's transaction but cannot open or finish one of its own. A
read-only export needs no transaction and observes one coherent snapshot for its
whole call.

```mw
module docs::transactions

resource Counter
    required value: int

store ^counters(name: string): Counter

pub fn bump(name: string)
    transaction
        const current = ^counters(name).value ?? 0
        ^counters(name).value = current + 1
```

The block stages durable writes; reads in the same transaction observe earlier
staged writes. When the block finishes, the transaction commits all staged
changes together. A required field left unset when a staged entry commits rolls
the transaction back with `run.required_missing` rather than committing a
partial entry — so several required fields may be populated across separate
statements in one transaction and validated together at commit.

The transaction ownership law is checked when the program image is verified: a
transaction is opened exactly once and committed on every path, every mutation
sits inside the region, and a transaction owner is never called. An image that
violates the law is rejected with `image.flow` before it can run.

## Indeterminate Commit

A pre-commit fault or a confirmed abort rolls the whole transaction back. A
commit that does not confirm is *indeterminate*: the store handle is poisoned,
every later operation fails with `run.commit`, and the process must exit and
reopen the store. On reopen a recorded commit witness classifies the store as
complete-new (the commit landed) or complete-old (it did not); the result is
reported, never retried. Local variable assignments are ordinary state and are
not rewound by durable rollback.

## Host Effects

Rollback cannot undo output or external writes. The following are rejected
inside transactions before the effect occurs:

- `print`;
- `std::log::info`, `std::log::warn`, and `std::log::error`;
- `std::io::writeText` and `std::io::writeBytes`.

Host capability reads, including clock, environment, context, and file reads,
may run inside a transaction. Their returned values are ordinary values, but
the external state they observe is not controlled by the durable transaction.

When an external write must follow a durable change, commit an ordinary durable
work record first and perform the external effect after the transaction.

## Transaction Scope

A transaction governs Marrow durable writes only. It is not a process lock and
does not make an external service part of the commit. Conflict and store errors
that escape the block cause rollback under the same rule as an explicit throw.
