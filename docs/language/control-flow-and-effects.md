# Control Flow And Errors

Marrow control flow is structured and indentation-based.

## Conditionals

```mw
if status == "open"
    print("open")
else if status == "loaned"
    print("loaned")
else
    print("other")
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
    print($"{i}")      ; 1 through 9

for i in 1..=10
    print($"{i}")      ; 1 through 10
```

`..` excludes the end; `..=` includes it. A range exists only as a loop iterable,
not as a value.

The two endpoints must be the same steppable type: `int`, `date`, or `instant`.
The loop variable binds to that type, so the body is fully type-checked. A
non-steppable endpoint (decimal, string, bool, enum) is a check error.

A `by` step sets the increment. For `int` endpoints the step is an `int`; for
`date` and `instant` endpoints it is a duration:

```mw
for i in 0..10 by 2          ; 0, 2, 4, 6, 8
for d in start..=end by 1.day
for t in startInstant..endInstant by 1.hour
```

When `by` is omitted, `int` defaults to a step of `1` and `date` to one calendar
day. `instant` has no safe default, so a range over it requires an explicit `by`
step.

Use an `int` range when decimal work needs fixed increments:

```mw
for i in 0..4
    var x: decimal = decimal(i) * 0.25
```

For a variable decimal step, multiply the integer loop index by the step:

```mw
fn sample(count: int, step: decimal)
    for i in 0..count
        var x: decimal = decimal(i) * step
```

A `date` steps in whole calendar days using calendar arithmetic, so it crosses
month and leap-day boundaries correctly; the date step must be a whole number of
days. An `instant` steps by its duration in UTC.

A loop never runs forever. For `int`, the step's sign sets the direction: a
positive step ascends, a negative one descends (`10..1 by -1` counts down). A
step pointing away from the end iterates zero times rather than looping
endlessly. When the endpoints and step are all literals, a provably wrong
direction — `1..10 by -1`, or `10..1` with the default `+1` — is a dead loop and
a check error; a wrong direction from a variable is simply an empty loop. A zero
step never progresses and is rejected.

`date` and `instant` ranges ascend only: the step must be a positive duration,
and descending temporal ranges are not yet supported. A negated duration step
(`by -1.day`) is a check error.

Collection loops walk durable iterables. A store, index, or keyed child layer
is a durable iterable; a `for` loop over one streams lazily rather than
materializing it:

```mw
for id in ^books
    if const title = ^books(id).title
        print(title)

for pos in ^books(id).tags
    if const tag = ^books(id).tags(pos)
        print(tag)
```

A single loop variable is the durable key or identity being streamed. For a
primary store root, it is the store identity. For a keyed child layer, it is the
child key at a populated position. For a non-unique index branch, it is the
identity stored in that lookup branch:

```mw
for id in ^books.byShelf("fiction")
    if const title = ^books(id).title
        print($"book {id}: {title}")
```

Use two loop variables, `entries(...)`, or `values(...)` when code needs values:

```mw
for id, book in ^books
    print($"{id}: {book.title}")

for pos, tag in ^books(id).tags
    print($"{pos}: {tag}")

for book in values(^books)
    print(book.title)
```

Use `keys(...)` when code wants to make address-only traversal explicit:

```mw
for id in keys(^books)
    print($"{id}")

for pos in keys(^books(id).tags)
    print($"{pos}")
```

Value and two-variable loops also read the values they yield; `keys(...)` reads
only the addresses.

`while` loops use a boolean condition:

```mw
while loanCount < limit
    loanCount = loanCount + 1
```

## Exiting Nested Loops

`break` exits the innermost loop. `continue` skips to the next iteration of the
innermost loop. To exit nested loops, extract the loop into a function and
`return`:

```mw
fn findWanted(): Id(^books)
    for shelf in ^books.byShelf
        for id in ^books.byShelf(shelf)
            if wanted(id)
                return id
    throw Error(code: "book.not_found", message: "No matching book.")

const id = findWanted()
```

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

Catch errors with `try` / `catch`:

```mw
try
    loan(id, borrower)
catch err: Error
    print($"loan failed: {err.message}")
```

A `try` statement requires a `catch` clause. `catch err: Error` binds a typed
error value. If the type annotation is omitted, `Error` is used. Applications
can store errors in their own saved resources when they want persistent audit or
diagnostics; those saved resources model persistent fields concretely.

Use catch-cleanup-rethrow when cleanup must run after a caught error:

```mw
try
    loan(id, borrower)
catch err: Error
    cleanupLoanAttempt(id)
    throw err
```

## Transactions

Transactions affect saved data, so their detailed rules live in
[Resources and Saved Data](resources-and-storage.md). A transaction is a block
statement:

```mw
transaction
    ^books(id).loanedTo = borrower
```
