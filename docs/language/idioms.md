# Idioms

Marrow code is arranged so that each consequence is visible on the line where
it happens. This page shows the marks that carry those consequences and the
conventional shape of a file, a guard prelude, a counter, and an example test.

## Marks

Every act with a result beyond local computation is spelled with one of these
marks, and nothing else is:

| Mark | Meaning | Search pattern |
|---|---|---|
| `^` | a [durable place](durable-places.md) | `\^` |
| `place` | a name bound to one entry address ([named places](durable-places.md#named-places)) | `\bplace ` |
| `transaction {` | a block whose writes commit together ([transactions](errors-and-transactions.md#transactions)) | `transaction {` |
| `at most` / `on more` | a [bounded durable traversal](traversal-and-indexes.md#bounded-durable-traversal) and its overflow block | `at most` |
| `checked` / `on` | [checked arithmetic](control-flow.md#checked-arithmetic) and its fault arms | `\bchecked ` |
| `try` | propagation of a `Result` failure ([prefix `try`](control-flow.md#prefix-try)) | `\btry ` |
| `require` | origination of a `Result` failure ([require guards](control-flow.md#require-guards)) | `\brequire ` |
| `delete` | removal of a durable field or entry ([deleting](durable-places.md#deleting)) | `\bdelete\b` |
| `$"` | an interpolated string ([literals](source-and-syntax.md)) | `\$"` |
| `while` | the one loop with no bound of its own ([while](control-flow.md)) | `\bwhile\b` |
| `unreachable(` | an invariant the program declares ([divergence](control-flow.md#divergence)) | `unreachable\(` |
| `todo(` | a path left unwritten | `todo\(` |
| `pub` | an exported function ([visibility](modules-and-functions.md#visibility)) | `\bpub ` |

Each construct has one spelling and no synonym, so a text search for its
pattern finds every use in a module. `\^` lists every point where durable data
enters the code. `transaction {` lists every commit. `at most` lists every
durable walk and `\bwhile\b` every loop without one. `\brequire ` and
`\btry ` together list every failure exit that is not a spelled `return`. A
pattern may also hit a comment or a string; those are discounted by reading,
and a real use is never missed.

Two marks rest on rules other pages state. A read or write through a `place`
carries no `^` on its own line, but the name was bound from a `^` address on
the binding line. `delete` is durable only: a local sparse field is cleared
with `unset` ([local values](resources.md#local-values)), so every `delete`
that compiles removes durable data.

There is no mark for the absence of consequence. An export whose call graph
never touches `^` is storeless, and `marrow check` lists it as such
([access demand](durable-places.md#access-demand)). A construct added to the
language keeps one spelling and keeps this list complete
([general-purpose language](../future/general-purpose-language.md)).

The marks in one module:

```mw
module docs::idioms::marks

resource Book {
    required title: string
    shelf: string
}

store ^books[id: int]: Book

pub fn shelve(id: int, shelf: string): Result<string, string> {
    transaction {
        place book = ^books[id]
        if not exists(book) {
            return err($"no book {id}")
        }
        book.shelf = shelf
        return ok(shelf)
    }
}

pub fn onShelf(shelf: string): int {
    var n = 0
    for id in ^books at most 100 {
        if ^books[id].shelf ?? "" == shelf {
            n += 1
        }
    } on more {
        return n
    }
    return n
}
```

`shelve` has one `transaction {` and one `place`; the write `book.shelf = shelf`
carries no `^` because `book` was bound from `^books[id]` two lines above.
`onShelf` has one `at most` and one `on more`. Searching this file for `\^`
finds every durable touch: the store declaration, the `place` binding, and the
two reads in `onShelf`.

## File skeleton

A module is ordered from what data is, to where it lives, to what the program
does: resource and type declarations come first, then `store` roots, then
functions. A function's `test` blocks sit next to it.

```mw
module docs::idioms::skeleton

// shape

resource Book {
    required title: string
    loans: int
}

// places

store ^books[id: int]: Book

// acts

pub fn recordLoan(id: Id(^books)) {
    transaction {
        if exists(^books[id]) {
            ^books[id].loans = (^books[id].loans ?? 0) + 1
        }
    }
}

pub fn loansOf(id: Id(^books)): int {
    return ^books[id].loans ?? 0
}

test "example: loansOf" {
    assert loansOf(Id(^books, 1)) == 0
}
```

A reader meets each name before its use: the shape, the root declared over it,
then the functions that read and write that root. A test beside its function
makes the two one unit to read and to move.

## Guard prelude

A `pub fn` opens with its preconditions, one per line, before the happy path.
A boolean precondition is a `require`. A presence check is a `const` with a
diverging `else`. Inside the function's own `transaction` block, a guard is a
plain `if` with a `return`, because that return commits.

```mw
module docs::idioms::lookup

resource Patient {
    required name: string
    wardCode: string
}

store ^patients[pid: int]: Patient

pub fn wardOf(pid: Id(^patients), wards: Map<string, string>): Result<string, string> {
    require exists(^patients[pid]) else "unknown patient"
    const name = ^patients[pid].name else {
        return err("patient has no name")
    }
    const code = ^patients[pid].wardCode else {
        return err($"{name} has no ward")
    }
    const ward = wards[code] ?? "(unassigned)"
    return ok(ward)
}
```

Local and durable presence take the same shape. A sparse durable read and a
local optional both take a diverging `else` that binds the present value past
the guard; a map lookup falls back with `??`. Past each guard the value is in
scope and present.

A mutating export puts its guards at the top of its block and its writes
below them, as `shelve` does above. Each guard returns before the first write,
so a rejected call commits nothing
([guards inside a block](errors-and-transactions.md#guards-inside-a-block)).
`require` and `try` do not stand in a function that owns a block, ahead of the
block or inside it; the spelled `if` is the guard form there.

## Validation chain

A validator combines the three guard forms. Let-else reads the subject and
rejects absence, `try` joins a shared guard, and `require` states each boolean
precondition on its own line. Guard order is rejection order: the first
failing line names the failure the caller sees.

```mw
module docs::idioms::validation

resource Book {
    required title: string
    required revision: int
    shelf: string
}

store ^books[id: int]: Book

fn revisionMatches(actual: int, expected: int): Result<bool, string> {
    require actual == expected else $"stale revision {actual}"
    return ok(true)
}

pub fn validateMove(id: int, expected: int, shelf: string): Result<bool, string> {
    const book = ^books[id] else {
        return err("unknown book")
    }
    try revisionMatches(book.revision, expected)
    const current = book.shelf ?? ""
    require not isEmpty(shelf) else "shelf is empty"
    require shelf != current else "already on that shelf"
    return ok(true)
}

test "example: validateMove" {
    ^books[1] = Book(title: "Small Gods", revision: 3)
    const accepted: Result<bool, string> = ok(true)
    const stale: Result<bool, string> = err("stale revision 3")
    assert validateMove(1, 3, "fantasy") == accepted
    assert validateMove(1, 2, "fantasy") == stale
}
```

Searching the module for `\brequire ` lists its three preconditions, and
`\btry ` the one point that forwards a shared guard's failure. The validator
opens no `transaction`; the export that calls it does.

## Counter allocation

A caller supplies its own entry keys; there is no `nextId` built-in
([presence and identity](builtins.md#presence-and-identity)). A program that
needs a fresh, increasing key mints one from a durable counter it owns. The
counter is an ordinary keyed root, one entry per sequence name.

```mw
module docs::idioms::allocation

resource Counter {
    required value: int
}

store ^idseq[name: string]: Counter

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn createBook(title: string): Id(^books) {
    transaction {
        place seq = ^idseq["book"]
        const next = (seq.value ?? 0) + 1
        seq.value = next
        const bid = Id(^books, next)
        ^books[bid] = Book(title: title)
        return bid
    }
}

pub fn titleOf(id: Id(^books)): string? {
    return ^books[id].title
}

test "example: createBook" {
    const first = createBook("Small Gods")
    const second = createBook("Pyramids")
    assert titleOf(first) ?? "" == "Small Gods"
    assert titleOf(second) ?? "" == "Pyramids"
}
```

`seq.value ?? 0` supplies the first value when the counter entry is absent, so
no separate initialization is needed. The increment and the create share one
block, so they commit or roll back together: a key is never advanced without
its entry. `Id(^books, next)` constructs the [entry identity](types-and-values.md#entry-identity)
from the allocated key without reading the store, and the returned `Id(^books)`
is the key of a later read. The program keeps the counter in step with
`^books`; the language only holds the two writes to one block.

## Named steps

Name each step. When a computation would nest more than two calls deep, each
stage is bound to a `const` and read by the next line, so the transform reads
from top to bottom.

```mw
module docs::idioms::named_steps

pub fn slug(raw: string): string {
    const trimmed = trim(raw)
    const words = split(trimmed, " ")
    const joined = join(words, "-")
    return joined
}

pub fn total(xs: List<int>): int {
    var sum = 0
    for x in xs {
        sum += x
    }
    return sum
}

test "example: slug and total" {
    assert slug(" small gods ") == "small-gods"
    assert total(List(10, 20, 12)) == 42
}
```

`slug` names each stage instead of writing `join(split(trim(raw), " "), "-")`.
`total` reduces a list with an accumulator loop: a running binding updated
once per element. Closures, and with them `fold`, `map`, and `filter`, are
future work ([general-purpose language](../future/general-purpose-language.md)).

A list starts from its constructor, `List(10, 20, 12)`, and `append` returns
the grown list: `xs = append(xs, extra)`. A map starts empty with `Map()` and
is filled with `m[k] = v`.

## Building text

A fixed message assembled from parts is one interpolated string. Text built up
across steps is `+` accumulation into a `var`.

```mw
module docs::idioms::text

pub fn label(name: string, open: int): string {
    return $"{name}: {open} open"
}

pub fn report(items: List<string>): string {
    var body = ""
    for item in items {
        const line = "- " + item + "\n"
        body += line
    }
    return body
}

test "example: label and report" {
    assert label("inbox", 3) == "inbox: 3 open"
    assert report(List("a", "b")) == "- a\n- b\n"
}
```

`label` keeps the layout of the result visible in the source. `report` grows
the body once per iteration. A hole may hold a scalar, an enum member, or an
entry identity; it renders as `string(...)` does
([conversion and output](builtins.md#conversion-and-output)).

## Checked arithmetic as a signature

Integer arithmetic that is expected to survive a fault is written `checked`,
and each fault it can raise takes a named diverging arm. The operation and its
recovery sit together; the expression's value is the non-faulting result.

```mw
module docs::idioms::arithmetic

pub fn perDayCents(totalCents: int, days: int): int {
    return checked totalCents / days
        on out_of_range {
            return 0
        } on zero_divisor {
            return 0
        }
}

pub fn sumCents(a: int, b: int): int? {
    const total: int = checked a + b
        on out_of_range {
            return absent
        }
    return total
}

test "example: perDayCents and sumCents" {
    assert perDayCents(700, 7) == 100
    assert perDayCents(700, 0) == 0
    assert sumCents(1, 2) ?? 0 == 3
    assert sumCents(maxInt, 1) ?? 0 == 0
}
```

Plain `a + b` faults on overflow and stops the invocation. `checked` is the
visible signal that this arithmetic handles a fault locally, and the arm names
which one. The arms each operation takes are listed under
[checked arithmetic](control-flow.md#checked-arithmetic).

## Example tests beside functions

A function's worked example is a neighboring test titled `example: <name>`,
so the example runs with the rest of the suite.

```mw
module docs::idioms::example_test

pub fn fieldCount(line: string): int {
    return length(split(line, ","))
}

test "example: fieldCount" {
    assert fieldCount("a,b,c") == 3
}
```

`example:` is only a title. `marrow test` runs the block like any other
[test](tests.md), and the title tells a reader where the function's
demonstration lives.
