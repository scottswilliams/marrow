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

`if const name = place` guards a saved value read, binding `name` in the then
block only when `place` is present. An optional `: type` annotation follows the
name, as on `const` and `var`, and names the saved read's type:

```mw
if const pages: int = ^books(id).pages
    print(pages)
```

## Short-Circuit Logic

`and` and `or` short-circuit from left to right:

```mw
if exists(^books(id)) and not exists(^books(id).loanedTo)
    loan(id, borrower)
```

The right side is not evaluated if the left side decides the result.

The absence-default `??` is lazy in the same way: its right-hand default is
evaluated only when the left read is absent, so a default expression with
effects does not run for a present value.

```mw
const shelf = ^books(id).shelf ?? defaultShelf()   ; defaultShelf() runs only when shelf is absent
```

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

A range loop never runs forever. For `int`, the step's sign sets the direction: a
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
materializing it (binding or assigning such a saved collection into a local is a
check error — iterate it directly or build a local collection):

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

Use two loop variables when code needs each address and value together:

```mw
for id, book in ^books
    print($"{id}: {book.title}")

for pos, tag in ^books(id).tags
    print($"{pos}: {tag}")
```

A composite keyed layer is a chain of single-key sub-layers (see
[Resources and Saved Data](resources-and-storage.md)), so a loop over it binds one
key column. The single variable is the outer column; descend the layer at that key
to reach the inner column:

```mw
for row in ^grids(id).cells
    for col, value in ^grids(id).cells(row)
        print($"({row},{col}) = {value}")
```

A value-reading loop head over a composite layer that still has more than one
column to fill pairs a key with a value that is itself a sub-layer, so it is
rejected at compile time. This covers the bare two-name form
(`for row, col in ^grids(id).cells`) and the `values(...)` and `entries(...)`
wrappers (`for v in values(^grids(id).cells)`,
`for row, v in entries(^grids(id).cells)`). Descend one column at a time instead;
`keys(...)` and `count(...)`, which read only the next key column, remain valid.

A saved path that names a single stored value — a fully-keyed leaf
(`^grids(id).cells(row, col)`), a scalar field (`^books(id).title`), or a whole
record (`^books(id)`) — is not an iterable. A `for` loop over one is a compile-time
error, since there is no key to stream.

`entries(...)` is the explicit two-name loop-head form for the same address/value
walk. It is not a collection value that can be assigned, returned, or passed to
single-variable loops:

```mw
for id, book in entries(^books)
    print($"{id}: {book.title}")

for id, book in reversed(entries(^books))
    print($"{id}: {book.title}")
```

Iteration helpers are not partially applied. `keys(...)`, `values(...)`, and
`entries(...)` must receive their iterable at the read site; a helper name alone
is not an iterator value.

Use `values(...)` when code needs only values:

```mw
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

Local keyed trees use the same loop shapes. For `var scores(player: string): int`,
`for player in scores` and `for player in keys(scores)` bind `string` keys,
`for player, score in scores` and `for player, score in entries(scores)` bind
key/value pairs, and `for score in values(scores)` binds values. `reversed(...)`
preserves the selected shape in descending key order: direct local keyed-tree
loops stay key-only or key/value by loop head, `reversed(values(scores))` yields
values, and `reversed(entries(scores))` yields pairs.

A local keyed tree's key columns follow the same key-type contract as a saved
keyed layer: each key must be an orderable scalar. An identity, an enum, a
resource, a sequence, or a `decimal` key is rejected at check, on a local keyed
`var` and a keyed function parameter alike.

A local sequence is a 1-based integer-keyed tree, so it follows the same shapes
as any other keyed collection — identical to a saved sequence. For
`var xs: sequence[int]`, `for pos in xs` and `for pos in keys(xs)` bind the
1-based `int` position, `for pos, x in xs` and `for pos, x in entries(xs)` bind
position/value pairs, and `for x in values(xs)` binds element values. The same
holds for any sequence-typed value, including one a function returns. `reversed`
walks descending positions, with `reversed(values(xs))` yielding the values in
reverse.

`while` loops use a boolean condition:

```mw
while loanCount < limit
    loanCount = loanCount + 1
```

Unlike a range loop, a `while` loop is unbounded: it runs until its condition is
false, and Marrow imposes no step or fuel limit, so `while true` with no exit
runs forever. Iteration is intentionally unbounded; ensuring a loop terminates is
the program author's responsibility. Recursion and source nesting are capped only
to fail closed against stack overflow, not as a work budget — see the
[cost model](cost-model.md).

A loop over a saved layer must not change that layer's key set while traversing
it. Deleting, appending, writing a whole keyed entry, or writing a field at a key
that is not the loop's own key all risk inserting or removing a sibling
mid-traversal and are rejected. Writing a field of the current entry — at the loop
key — is fine. To rewrite the key set, build a local sequence of the keys first,
then iterate that local and mutate the layer:

```mw
var ids: sequence[Id(^books)]
for id in keys(^books)
    append(ids, id)
for id in values(ids)
    delete ^books(id)
```

`keys(^books)` is a stream over saved data, not a value: it can be iterated in
place or counted, but never bound to a local, passed by value, or otherwise
materialized. The loop above copies each key into the local `ids`, so the
mutation traverses the snapshot rather than the live layer.

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

`code` and `message` are always present, so `err.code` and `err.message` read
directly. `help` and `data` are sparse, so a bare `err.help` or `err.data` is a
compile error; resolve each at the read site like any maybe-present read:

```mw
catch err: Error
    print($"{err.message} ({err.help ?? "no help"})")
    if exists(err.data)
        print("error carries data")
```

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
