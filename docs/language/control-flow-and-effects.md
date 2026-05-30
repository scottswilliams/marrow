# Control Flow And Errors

Marrow control flow is structured and indentation-based.

## Conditionals

```mw
if status == "open"
    write("open")
else if status == "loaned"
    write("loaned")
else
    write("other")
```

Conditions must be `bool`.

## Short-Circuit Logic

`and` and `or` short-circuit from left to right:

```mw
if exists(^books(id)) and not exists(^books(id).loanedTo)
    loan(id, borrower)
```

The right side is not evaluated if the left side decides the result.

## Loops

Range loops use common modern range syntax:

```mw
for i in 1..10
    write($"{i}")      ; 1 through 9

for i in 1..=10
    write($"{i}")      ; 1 through 10
```

Ranges use `int` endpoints.

Tree loops iterate one layer at a time:

```mw
for id in ^books
    write($"book {id}")

for id in keys(^books.byShelf("fiction"))
    write($"book {id}: {^books(id).title}")
```

`values` and `entries` make other traversal shapes explicit:

```mw
for book in values(^books)
    write(book.title)

for id, book in entries(^books)
    write($"{id}: {book.title}")
```

Use `keys(...)` when a loop only needs identities. `values(...)` and
`entries(...)` materialize the values or resources they yield.

`while` loops use a boolean condition:

```mw
while loanCount < limit
    loanCount = loanCount + 1
```

## Loop Labels

Labels let `break` and `continue` target an outer loop:

```mw
outer: for shelf in ^books.byShelf
    for id in ^books.byShelf(shelf)
        if wanted(id)
            break outer
```

`break` exits a loop. `continue` skips to the next iteration.

## Errors

Errors are builtin resource-shaped values. `Error` is not a managed saved
resource; its `data` field is the dynamic boundary used by tools and hosts.

```mw
resource Error
    required code: ErrorCode
    required message: string
    help: string
    data: unknown
```

Raise an error with `throw`:

```mw
throw Error(
    code: "book.absent",
    message: $"Book {id} does not exist.",
    help: "Check the id or create the book before loaning it.",
)
```

`throw` requires an `Error` value.

Catch errors with `try` / `catch` / `finally`:

```mw
try
    loan(id, borrower)
catch err: Error
    write($"loan failed: {err.message}")
finally
    write("attempt finished")
```

`catch err: Error` binds a typed error value. If the type annotation is
omitted, `Error` is used. Applications can store errors in their own saved
resources when they want persistent audit or diagnostics; those saved
resources model persistent fields concretely.

`finally` runs before the `try` statement exits, whether the block succeeds,
throws, returns, breaks, or continues. If `finally` throws, that error leaves
the statement.
`finally` is cleanup code; it cannot `return`, `break`, or `continue`.

## Transactions And Locks

Transactions and locks affect saved data, so their detailed rules live in
[Resources and Saved Data](resources-and-storage.md). They are block
statements:

```mw
lock ^books(id)
    transaction
        ^books(id).loanedTo = borrower
```
