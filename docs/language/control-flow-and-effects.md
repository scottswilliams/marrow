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

Collection loops walk durable iterables. A store, index branch, or keyed child
layer is a durable iterable; a `for` loop over one streams lazily rather than
materializing it (binding or assigning such a saved collection into a local is a
check error — iterate it directly or build a local collection).

The loop head is the one iteration construct:

```text
for name ("," name)* in ("reversed")? iterable ("by" step)?
```

The bound names are key-first: the first name always binds an address, and each
further name descends one layer to the value at that address. `reversed` is a
traversal-direction keyword in the head slot; `by` sets a range step. There is no
wrapper builtin between `in` and the iterable — the head's name count is the whole
story.

A single loop variable binds the durable key or identity being streamed. For a
primary store root, it is the store identity. For a keyed child layer, it is the
child key at a populated position. For a non-unique index branch, it is the
identity stored in that lookup branch:

```mw
for id in ^books
    if const title = ^books(id).title
        print(title)

for pos in ^books(id).tags
    if const tag = ^books(id).tags(pos)
        print(tag)

for id in ^books.byShelf("fiction")
    if const title = ^books(id).title
        print($"book {id}: {title}")
```

The value is one descent away: read `^books(id).title` in the body, or bind it in
the head. There is no value-only saved loop — iterating values without their
addresses over saved data is a projection, not a navigation step, so the two-name
head is the honest spelling.

A bare non-unique index root — the index named with no lookup key
(`^books.byShelf`) — streams every stored identity across all its branches in
index order, not the distinct leading key values; with two loop variables it pairs
each identity with its record. There is no form that enumerates an index's
distinct leading values; deriving them means streaming the identities and
deduplicating the leading key in code.

A second loop variable binds the value reached at each address:

```mw
for id, book in ^books
    print($"{id}: {book.title}")

for pos, tag in ^books(id).tags
    print($"{pos}: {tag}")
```

A composite keyed layer is a chain of single-key sub-layers (see
[Resources and Saved Data](resources-and-storage.md)). A loop over an N-column
layer binds either the outer column alone (one name) or every key column
outermost-first plus the leaf value (N+1 names):

```mw
for row in ^grids(id).cells                 ; outer column only
    for col, value in ^grids(id).cells(row)
        print($"({row},{col}) = {value}")

for row, col, value in ^grids(id).cells     ; both columns and the leaf
    print($"({row},{col}) = {value}")
```

An intermediate arity — a name count that is neither 1 nor N+1 — is a compile
error (`check.loop_head_arity`) carrying the layer's column count. Over the
two-column `cells`, `for row, col in ^grids(id).cells` is rejected: two names name
both keys but leave no leaf to bind. Bind the outer column and descend, or bind
all columns and the value.

A saved path that names a single stored value — a fully-keyed leaf
(`^grids(id).cells(row, col)`), a scalar field (`^books(id).title`), or a whole
record (`^books(id)`) — is not an iterable. A `for` loop over one is a compile-time
error, since there is no key to stream.

`reversed` before the iterable walks it in reverse key order:

```mw
for id in reversed ^books
    if const title = ^books(id).title
        print(title)

for pos, tag in reversed ^books(id).tags
    print($"{pos}: {tag}")
```

Over a saved layer the reversal streams stored keys from high to low — a true
reverse, not a copy of the forward result flipped after the fact; an early `break`
stops the scan. A composite identity reverses at every key level. `reversed` is a
keyword only in this head slot, between `in` and the iterable; everywhere else it
is an ordinary identifier. A range has no `reversed` form — spell a descending
range with its endpoints and a negative `by` step (`for i in 10..1 by -1`).

Local keyed trees use the same head, key-first. For
`var scores(player: string): int`, `for player in scores` binds the `string` key
and `for player, score in scores` binds key and value.

A local keyed tree's key columns follow the same key-type contract as a saved
keyed layer: each key must be an orderable scalar. An identity, an enum, a
resource, a sequence, or a `decimal` key is rejected at check, on a local keyed
`var` and a keyed function parameter alike.

A local sequence is a 1-based integer-keyed tree, so it follows the same head
shapes. For `var xs: sequence[int]`, `for pos in xs` binds the 1-based `int`
position and `for pos, x in xs` binds position and element. The same holds for any
sequence-typed value, including one a function returns. `reversed xs` walks
descending positions.

The names in one loop head share one scope, so repeating a name (`for a, a in x`)
is rejected under `check.duplicate_declaration`.

`keys(...)` and `values(...)` are value-position builtins over local collections:
they materialize a local sequence of addresses or elements (see
[Builtins](builtins.md)). They are not loop heads. `for k in keys(xs)` is rejected
(`check.loop_head_view_call`) because the head already binds addresses key-first;
bind the sequence first when a materialized copy is what the code wants. They are
also rejected over any saved path — iterate saved data with `for ... in`.

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
key — is fine. To rewrite the key set, copy the keys into a local sequence first,
then iterate that local and mutate the layer:

```mw
var ids: sequence[Id(^books)]
for id in ^books
    append(ids, id)
for pos, id in ids
    delete ^books(id)
```

Iterating `^books` streams saved identities lazily and never materializes them as
a value. The loop above copies each identity into the local `ids`, so the
mutation traverses the snapshot rather than the live layer.

## Exiting Nested Loops

`break` exits the innermost loop. `continue` skips to the next iteration of the
innermost loop. To exit nested loops, extract the loop into a function and
`return`:

```mw
fn findWanted(): Id(^books)
    for id in ^books
        for pos in ^books(id).tags
            if const tag = ^books(id).tags(pos)
                if tag == "wanted"
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
