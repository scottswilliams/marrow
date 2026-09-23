# Traversal and indexes

`for` walks four things: an integer range, a local list or map, a durable root
or branch, and an index. A durable walk states its bound at the loop head and
says what happens when more entries remain.

A root, a branch addressed by its ancestor keys, and a `from` key:

```mw
module docs::traversal::walk

resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

const pageSize = 100

pub fn noteTotal(): Result<int, string> {
    var total = 0
    for id in ^books at most pageSize {
        for pos in ^books[id].notes at most 100 {
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
    for pos in ^books[id].notes at most 2 from first {
        text += ^books[id].notes[pos].text ?? ""
    } on more {
        text += "..."
    }
    return text
}

pub fn addBooksWithNotes() {
    transaction {
        ^books[1] = Book(title: "Small Gods")
        ^books[1].notes[1] = Book.notes(text: "a")
        ^books[1].notes[2] = Book.notes(text: "b")
        ^books[1].notes[3] = Book.notes(text: "c")
        ^books[2] = Book(title: "Pyramids")
    }
}

test "walks" {
    addBooksWithNotes()
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

struct VisitResult {
    visited: int
    absentAtTwo: bool
}

pub fn changeWhileWalking(): VisitResult {
    transaction {
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
        return VisitResult(visited: visited, absentAtTwo: absentAtTwo)
    }
}

test "the visited keys are frozen before the body runs" {
    const result = changeWhileWalking()
    assert result.visited == 2
    assert result.absentAtTwo
}
```

`for id in ^books at most pageSize` visits at most `pageSize` books, the
module constant `100`. `id` is the key of each entry. The inner loop walks
`^books[id].notes`. In `notesFrom`, `from first` starts at that position;
this branch can be traversed even when the book entry is absent. Each
`on more` block handles reaching the bound.

## Bounded durable traversal

A durable `for` head names a root or a keyed branch, a bound, an optional
starting key, and an `on more` block:

```text
for k in <base> at most N [from f] {
    statements
} on more {
    statements
}
```

The base is a root such as `^books`, a branch beneath one entry such as
`^books[id].notes`, or a branch beneath an entry reference such as `book.notes`.
`k` binds each key in ascending [key order](types-and-values.md#key-types).
The body addresses the entry using its key. A durable loop binds exactly one name. `N` is a positive integer
literal, or a module `const` of type `int`, of at most 65,536. A durable `for`
without `at most` or without `on more` is a `check.type` error.

The loop freezes the first `N` keys before the body runs, then runs the body
once per frozen key. `on more` runs when an `(N + 1)`th key exists and every
body ran to completion. A `break`, a `return`, or a fault leaves the loop
without running it. `from f` starts the frozen set at `f`, inclusive.

Decoded traversal and index keys must match their declared scalar types and
supported value ranges. A mismatched stored key raises `run.corruption`,
including a key inspected to decide `on more`. These operation checks cover
the decoded key components; [logical inspection](../operations/README.md#auditing-a-store)
checks the complete store.

A frozen key does not establish presence: an earlier iteration may have
erased that entry. To read required fields or update fields, bind a checked
[entry reference](durable-data.md#entry-references) in the body:

```mw
module docs::traversal::references

resource Book {
    required title: string
    shelf: string
}

store ^books[id: int]: Book

pub fn shelveAll(shelf: string): int {
    var moved = 0
    transaction {
        for id in ^books at most 100 {
            ref book = ^books[id] else {
                continue
            }
            book.shelf = shelf
            moved += 1
        } on more {
            return moved
        }
    }
    return moved
}

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn shelfOf(id: int): string? {
    return ^books[id].shelf
}

test "each iteration checks entry presence" {
    add(1, "Small Gods")
    add(2, "Pyramids")
    assert shelveAll("top") == 2
    assert shelfOf(2) ?? "" == "top"
}
```

`shelveAll` checks presence on each iteration and skips an absent entry.
The reference lives for that iteration. A protected read or write inside a
loop entered after its proof was established requires the proof to survive
that repeating region; an erase of the family invalidates it
([entry references](durable-data.md#entry-references)).

Writes in the body do not change the frozen set. An entry created in the body
is not visited. An entry erased by an earlier iteration keeps its frozen key,
and a read through that key finds nothing, as the second test above shows.

`N` bounds the frozen keys and body executions. Acquiring those keys and the
`more` result uses at most `N + 1` bounded scans in the addressed entry family.
A family presence test, `exists(^books)` or `exists(^books[id].notes)`, uses
one scan. Child families lie outside that range, so their populations add no
navigation steps. Absent ancestors are not visited; a branch beneath one is
still traversable when the program supplies its ancestor keys.

A stored entry whose presence record is missing raises `run.corruption`,
including during the extra step that decides `on more`. Once that step finds a
present entry, it records `more` and inspects no later one. Traversal checks the
structure and domain of what it walks; complete contents are checked by
[logical inspection](../operations/README.md#auditing-a-store).

The bound counts scan calls, not copied cells, backend time, or total memory.
Each call can return a page containing own payload and later entries even
though the step classifies only its first cell. Pages contain at most 64 cells
with a soft 1 MiB key/value byte target; an oversized first cell is returned
to make progress. Session setup, loop-body operations and commit work are
separate ([storage implementation](../implementation/storage.md#navigating-entries)).

The frozen keys are held as one list and count against the collection limit,
so a walk over wide keys can reach `run.collection_limit` before `N` keys.

`for` iterates one key component. A composite-keyed root or branch, such as
`store ^cells[x: int, y: int]: Cell`, is addressed by its whole tuple and is
not iterated; a `for` head over it is a `check.unsupported` error. Give every
layer a program needs to walk its own single-key branch.

An entry reference names one entry, so `for k in b` is a `check.type` error.
Walk a branch beneath it, `for k in b.notes`, or walk the root.

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
ordered path to the root's entries by one or more of their keys or top-level
fields. A program cannot write an index directly. A non-unique index ends with
the root's complete key; a `unique` index may project any permitted subset and
admits one entry per projected value:

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
    index byId[id] unique
    index all[id]
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

pub fn countAll(): ShelfCount {
    var count = 0
    for bookId in ^books.all at most 100 {
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
            ref m = ^books[found] else {
                return false
            }
            m.shelf = shelf
            return true
        }
        return false
    }
}

test "indexes" {
    add(1, "Small Gods", "top", "111")
    add(2, "Pyramids", "top", "222")
    add(3, "Mort", "low", "333")
    assert countAll().count == 3
    assert not countAll().truncated
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
writes the entry once; its indexes follow. `moveByIsbn` binds an entry reference over
the found identity, handles absence in its `else`, and changes `shelf`; the last
two assertions show `byShelf` moved with it.

Each component names one key of the root or one top-level field of the
resource, and no component repeats. A root's key names are the store's own and
a resource field may share one ([keys](durable-data.md#keys)); a component
whose spelling names both is refused, because it resolves neither. A component
has type `int`, `string`, `bool`, `bytes`, `date`, or `instant`
([key types](types-and-values.md#key-types)). A field inside a group or a
branch is not a component. A non-unique index ends with every key of the root
in declaration order, with no key before that final suffix. A `unique` index may omit the
keys. An index name is distinct from the root's key names and the resource's
field names. A root declares at most 8 indexes. A singleton root declares no index, and
operating on one is future work ([durable paths](durable-data.md#access-demand)).
Each of these rules is a `check.type` error at the declaration.

The runtime maintains an index inside the same transaction as the entry it
projects. An entry contributes an index value exactly when the entry and
all projected components are present. An empty entry or an entry with every
sparse field absent still contributes to an index that projects only keys.
Creation, field assignment or clearing, whole-entry replacement, and entry
deletion keep this correspondence. An unchanged projection requires no index
write. A mutation that would put two entries under one `unique` value faults
with `run.unique_index` and rolls the whole transaction back.

This maintenance assumes the existing entries and indexes agree. Logical
inspection reports missing or orphaned index cells; it does not repair them.

Each index has its own line in the
[identity ledger](../tools/projects.md#identity-ledger),
`index books.byShelf`, minted with the root's other identities. Today,
renaming an index mints a new identity. Rename and retirement that keep an
index's identity are future work ([status](../status.md)).

## Reading an index

A program reads an index through its root, `^books.byShelf`. The read shape
follows the index kind.

A non-unique index is walked with a bounded `for` head. When the index projects
fields, the brackets hold every field component. The loop variable binds the
[entry identity](types-and-values.md#entry-identity) `Id(^books)` of each
entry, in ascending order of the index:

```text
for bookId in ^books.byShelf[shelf] at most 100 {
    count += 1
} on more {
    return ShelfCount(count: count, truncated: true)
}
```

A non-unique index that projects only keys, such as `all[id]` above, takes no
brackets: `for bookId in ^books.all at most 100`. The complete `countAll` example
uses this form. Empty brackets remain a `parse.syntax` error.

`^books[bookId]` reads the entry the identity names. The walk freezes its
identities and runs `on more` exactly as a root walk does. The root's key is
one component, and the walk takes no `from`; violating either restriction is
a `check.unsupported` error ([known gaps](README.md#known-gaps)).

A `unique` index is read with brackets holding the whole value,
`^books.byIsbn[isbn]`. The result is `Id(^books)?`: the one matching entry's
identity, or absent. `if const found = ^books.byIsbn[isbn]` binds the identity
when it is present, and `^books[found].title` reads through it.

`exists(^books.byIsbn[isbn])` answers presence alone and yields a `bool`
([presence and identity](builtins.md#presence-and-identity)). A non-unique
index has no `exists`; the `for` head is its only read, and `exists` over it
is a `check.type` error.

A found identity is an address and supplies no automatic presence proof. Inside a `transaction`,
`ref m = ^books[found] else { return false }` checks presence, and `m.shelf = shelf`
writes one field of the entry the lookup found, and `^books[found] = Book(...)`
replaces it, exactly as a key in brackets would
([entry references](durable-data.md#entry-references)).
