# Traversal and indexes

`for` walks four things: an integer range, a local list or map, a durable root
or branch, and an index. A durable walk states its bound at the loop head and
says what happens when more entries remain.

A root, a branch beneath a pin, a branch beneath a named place, and a `from`
key:

```mw
module docs::traversal::walk

resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

pub fn noteTotal(): Result<int, string> {
    var total = 0
    for id, book in ^books at most 100 {
        for pos in book.notes at most 100 {
            total += 1
        } on more {
            return err("more than 100 notes")
        }
    } on more {
        return err("more than 100 books")
    }
    return ok(total)
}

pub fn notesFrom(id: int, first: int): string {
    var text = ""
    place book = ^books[id]
    for pos in book.notes at most 2 from first {
        text += book.notes[pos].text ?? ""
    } on more {
        text += "..."
    }
    return text
}

test "walks" {
    ^books[1] = Book(title: "Small Gods")
    ^books[1].notes[1] = Book.notes(text: "a")
    ^books[1].notes[2] = Book.notes(text: "b")
    ^books[1].notes[3] = Book.notes(text: "c")
    ^books[2] = Book(title: "Pyramids")
    match noteTotal() {
        ok(total) => {
            assert total == 3
        }
        err(reason) => {
            unreachable("two books hold three notes")
        }
    }
    assert notesFrom(1, 2) == "bc"
    assert notesFrom(1, 1) == "ab..."
}

test "the visited keys are frozen before the body runs" {
    ^books[1] = Book(title: "a")
    ^books[2] = Book(title: "b")
    var visited = 0
    var absentAtTwo = false
    for id in ^books at most 10 {
        visited += 1
        if id == 1 {
            delete ^books[2]
            ^books[3] = Book(title: "c")
        }
        if id == 2 {
            absentAtTwo = not exists(^books[id])
        }
    } on more {
        unreachable("no more")
    }
    assert visited == 2
    assert absentAtTwo
}
```

`for id, book in ^books at most 100` visits at most 100 books. `id` is the key
of each entry and `book` is a pin, an address for the entry at that key.
`for pos in book.notes` walks the notes beneath the pinned book. In
`notesFrom`, a named [place](durable-places.md#named-places) is the base
instead, and `from first` starts the walk at that position. Each `on more`
block says what the function does when the bound is reached.

## Bounded durable traversal

A durable `for` head names a root or a keyed branch, a bound, an optional
starting key, and an `on more` block:

```text
for k[, p] in <base> at most N [from f] {
    statements
} on more {
    statements
}
```

The base is a root such as `^books`, a branch beneath one entry such as
`^books[id].notes`, or a branch beneath a place or a pin such as `book.notes`.
`k` binds each key in ascending [key order](types-and-values.md#key-types).
The body reads the entry through the key or the pin. `N` is a positive integer
literal of at most 65,536. A durable `for` without `at most` or without
`on more` is a `check.type` error.

The loop freezes the first `N` keys before the body runs, then runs the body
once per frozen key. `on more` runs when an `(N + 1)`th key exists and every
body ran to completion. A `break`, a `return`, or a fault leaves the loop
without running it. `from f` starts the frozen set at `f`, inclusive.

A pin `p` is a [place](durable-places.md#named-places) over the entry at the
current key, scoped to the body. It reads nothing on its own. A read through
the pin is optional like any durable read, and a write through it sits inside
a `transaction`. `exists(p)` reports whether the entry is still present; it
changes no later read or write.

Writes in the body do not change the frozen set. An entry created in the body
is not visited. An entry erased by an earlier iteration keeps its frozen key,
and a read through that key finds nothing, as the second test above shows.

`N` bounds the frozen keys and body executions, not the total navigation work.
An address that holds only branch descendants and no entry payload is not
visited. Navigation skips each such address with one seek past its descendants,
but may skip any number of them before finding a present entry or the end.
The current engine-call count is proportional to `N + 1 + d`, where `d` is the
number of descendant-only entries skipped. A family presence test,
`exists(^books)` or `exists(^books[id].notes)`, has the same limitation. This
navigation work is not bounded by the invocation's instruction budget
([status](../status.md#bounds-and-platform)).

The frozen keys are held as one list and count against the collection limit,
so a walk over wide keys can reach `run.collection_limit` before `N` keys.

`for` iterates one key component. A composite-keyed root or branch, such as
`store ^cells[x: int, y: int]: Cell`, is addressed by its whole tuple and is
not iterated; a `for` head over it is a `check.unsupported` error. Give every
layer a program needs to walk its own single-key branch.

A place names one entry, so `for k in b` over a place is a `check.type` error.
Walk a branch beneath the place, `for k in b.notes`, or walk the root.

## Ranges

A `for` head over an integer range binds one name to each integer in ascending
order. `..` excludes the end and `..=` includes it. Both ends are `int`
expressions, evaluated once. `by step` advances by a positive integer literal
each iteration:

```text
for i in 1..=n {
    sum += i
}

for value in 0..10 by 2 {
    count += 1
}
```

The first loop runs `n` times. The second runs five times, for `0`, `2`, `4`,
`6`, and `8`. A range whose start is past its end, such as `5..3` or `5..=4`,
runs zero times. A range that reaches `maxInt` ends the loop. A range takes no
`at most`; its length is fixed by its ends. `by 0`, a negative step, and a
computed step are `check.type` errors. A range covers integers only.

## Local collections

A `for` head over a local list or map walks every element;
[control flow](control-flow.md) states the binding forms. A local collection
takes no `at most`; its length is already known.

## Index declarations

A keyed root declares an index inside its `store` block. An index is an
ordered path to the root's entries by one or more of their fields; it holds no
data of its own and has no write operation. A non-unique index ends with the
root's key. A `unique` index leaves the key out and admits one entry per
value:

```mw
module docs::traversal::indexes

resource Book {
    required title: string
    shelf: string
    isbn: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

struct ShelfCount {
    count: int
    truncated: bool
}

pub fn add(id: int, title: string, shelf: string, isbn: string) {
    transaction {
        ^books[id] = Book(title: title, shelf: shelf, isbn: isbn)
    }
}

pub fn countOnShelf(shelf: string): ShelfCount {
    var count = 0
    for bookId in ^books.byShelf[shelf] at most 100 {
        count += 1
    } on more {
        return ShelfCount(count: count, truncated: true)
    }
    return ShelfCount(count: count, truncated: false)
}

pub fn titleByIsbn(isbn: string): string? {
    if const found = ^books.byIsbn[isbn] {
        return ^books[found].title
    }
    return absent
}

pub fn isbnTaken(isbn: string): bool {
    return exists(^books.byIsbn[isbn])
}

pub fn moveByIsbn(isbn: string, shelf: string): bool {
    transaction {
        if const found = ^books.byIsbn[isbn] {
            ^books[found].shelf = shelf
            return true
        }
        return false
    }
}

test "indexes" {
    add(1, "Small Gods", "top", "111")
    add(2, "Pyramids", "top", "222")
    add(3, "Mort", "low", "333")
    assert countOnShelf("top").count == 2
    assert titleByIsbn("333") ?? "" == "Mort"
    assert isbnTaken("222")
    assert not isbnTaken("999")
    assert moveByIsbn("333", "top")
    assert countOnShelf("top").count == 3
    assert countOnShelf("low").count == 0
}
```

`byShelf[shelf, id]` orders books by shelf, then by key, so two books on one
shelf stay distinct. `byIsbn[isbn] unique` maps each ISBN to one book. `add`
writes the entry once; both indexes follow. `moveByIsbn` changes `shelf`, and
the last two assertions show `byShelf` moved with it.

Each component names one key of the root or one top-level field of the
resource, and no component repeats. A component is an `int`, `string`, `bool`,
`bytes`, `date`, or `instant` field
([key types](types-and-values.md#key-types)). A field inside a group or a
branch is not a component. A non-unique index ends with every key of the root
in declaration order and puts no key first. A `unique` index may omit the
keys. An index name shares the root's namespace with its keys and its fields.
A root declares at most 8 indexes. A singleton root declares no index. Each of
these rules is a `check.type` error at the declaration.

The compiler maintains every index. A field assignment, a field clear, a
whole-entry replacement, and a `delete` each keep the affected indexes in step
with the entry; no source operation writes an index. A commit that would put
two entries under one `unique` value faults with `run.unique_index` and rolls
the whole transaction back:

```text
ERROR duplicate isbn (run.unique_index at 20:9)
0 passed, 0 failed, 1 errored (1/1 selected)
```

Each index has its own line in the
[identity ledger](../tools/projects.md#identity-ledger),
`index books.byShelf`, minted with the root's other identities. Today,
renaming an index mints a new identity. Rename and retirement that keep an
index's identity are future work ([status](../status.md)).

## Reading an index

A program reads an index through its root, `^books.byShelf`. The read shape
follows the index kind.

A non-unique index is walked with a bounded `for` head. The brackets hold
every field component, and the loop variable binds the
[entry identity](types-and-values.md#entry-identity) `Id(^books)` of each
entry, in ascending order of the index:

```text
for bookId in ^books.byShelf[shelf] at most 100 {
    count += 1
} on more {
    return ShelfCount(count: count, truncated: true)
}
```

`^books[bookId]` reads the entry the identity names. The walk freezes its
identities and runs `on more` exactly as a root walk does. The root's key is
one component, and the walk takes no `from` and no pin; each of those forms is
a `check.unsupported` error.

A `unique` index is read with brackets holding the whole value,
`^books.byIsbn[isbn]`. The result is `Id(^books)?`: the one matching entry's
identity, or absent. `if const found = ^books.byIsbn[isbn]` binds the identity
when it is present, and `^books[found].title` reads through it.

`exists(^books.byIsbn[isbn])` answers presence alone and yields a `bool`
([presence and identity](builtins.md#presence-and-identity)). A non-unique
index has no `exists`; the `for` head is its only read, and `exists` over it
is a `check.type` error.

A found identity is an address. Inside a `transaction`,
`^books[found].shelf = shelf` writes one field of the entry the lookup found,
and `^books[found] = Book(...)` replaces it, exactly as a key in brackets
would.
