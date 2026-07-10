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

## Transaction Blocks

```mw
module docs::transactions

resource Account
    required balance: int

store ^accounts(id: int): Account

pub fn transfer(from: Id(^accounts), to: Id(^accounts), amount: int): bool
    if const fromBalance = ^accounts(from).balance
        if const toBalance = ^accounts(to).balance
            transaction
                ^accounts(from).balance = fromBalance - amount
                ^accounts(to).balance = toBalance + amount
            return true
    return false
```

The block stages durable writes. Reads in the same transaction observe earlier
staged writes. When the block finishes without an escaping error, the outermost
transaction commits all staged data and maintained index changes together.

If an error escapes a transaction block, its durable changes roll back. A
normal `return`, `break`, or `continue` commits before transferring control.
Local variable assignments are ordinary interpreter state and are not rewound
by durable rollback.

## Nested Transactions

A nested transaction joins its enclosing transaction. It does not commit
independently. Successful inner work becomes durable only when the outermost
block commits.

An error that escapes a nested transaction aborts the joined outer transaction
and propagates to a handler outside that outermost boundary. Catching an error
inside the transaction before it escapes is ordinary control flow; the failed
operation has no effect, and the transaction may continue when the fault is
recoverable.

Required-member validation for staged resource creation is performed at the
outer commit. This permits several required fields to be populated across
separate statements in one transaction.

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
