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

Range loops use range syntax:

```mw
for i in 1..10
    write($"{i}")      ; 1 through 9

for i in 1..=10
    write($"{i}")      ; 1 through 10
```

`..` excludes the end; `..=` includes it. A range exists only as a loop iterable,
not as a value.

The two endpoints must be the same steppable type: `int`, `decimal`, `date`, or
`instant`. The loop variable binds to that type, so the body is fully type-checked.
A non-steppable endpoint (string, bool, enum) is a check error.

A `by` step sets the increment. For `int` and `decimal` endpoints the step is a
number of the same type; for `date` and `instant` endpoints it is a duration:

```mw
for i in 0..10 by 2          ; 0, 2, 4, 6, 8
for x in 0.0..1.0 by 0.25    ; 0.0, 0.25, 0.50, 0.75
for d in start..=end by 1.day
for t in startInstant..endInstant by 1.hour
```

When `by` is omitted, `int` defaults to a step of `1` and `date` to one calendar
day. `decimal` and `instant` have no safe default, so a range over either requires
an explicit `by` step.

A `date` steps in whole calendar days using calendar arithmetic, so it crosses
month and leap-day boundaries correctly; the date step must be a whole number of
days. An `instant` steps by its duration in UTC.

A loop never runs forever. For `int` and `decimal` the step's sign sets the
direction: a positive step ascends, a negative one descends. A step pointing away
from the end iterates zero times rather than looping endlessly — `10..1 by -1`
counts down, while `1..10 by -1` and `10..1` (default `+1`) both run zero times. When
the endpoints and step are all literals and the direction is provably empty, that
dead loop is a check error; a wrong direction from a variable is simply an empty
loop. A zero step never progresses and is rejected.

`date` and `instant` ranges ascend only: a duration is never negative, so the step
must be a positive duration and descending temporal ranges are not yet supported. A
negated duration step (`by -1.day`) is a check error.

Collection loops walk elements. A store, index, or keyed child layer is a durable
iterable; a `for` loop over one streams lazily rather than materializing it:

```mw
for book in ^books
    write(book.title)

for tag in ^books(id).tags
    write(tag)
```

A collection element is the useful value the collection stores or selects. For a
primary resource root, the element is the resource. For a sequence, the element
is the item at a populated position. For a non-unique index branch, the element
is the identity stored in that lookup branch:

```mw
for id in ^books.byShelf("fiction")
    write($"book {id}: {^books(id).title}")
```

Use two loop variables when code needs both the address and the element:

```mw
for id, book in ^books
    write($"{id}: {book.title}")

for pos, tag in ^books(id).tags
    write($"{pos}: {tag}")
```

Use `keys(...)` when code only needs addresses:

```mw
for id in keys(^books)
    write($"{id}")

for pos in keys(^books(id).tags)
    write($"{pos}")
```

Saved-layer iteration walks child keys in stored order, streaming them lazily
rather than materializing the layer. Element and two-variable loops also read the
values they yield; `keys(...)` reads only the addresses.

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

## Transactions

Transactions affect saved data, so their detailed rules live in
[Resources and Saved Data](resources-and-storage.md). A transaction is a block
statement:

```mw
transaction
    ^books(id).loanedTo = borrower
```
