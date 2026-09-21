# A durable program, read through

This library catalog uses one root for books and one for a counter. Each book
also owns a keyed branch of notes. The complete program below shows the two
compositions this page owns: one transaction spanning both roots, and bounded
traversal nested from a root into a branch.

```mw
module docs::walkthrough::catalog

resource Book {
    required title: string
    required shelf: string

    notes[seq: int] {
        required text: string
        required at: instant
    }
}

resource Tally {
    required count: int
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
}

store ^tallies[name: string]: Tally

pub fn add(id: int, title: string, shelf: string, at: instant): bool {
    transaction {
        if exists(^books[id]) {
            return false
        }
        place catalogued = ^tallies["catalogued"]
        catalogued = Tally(count: (catalogued.count ?? 0) + 1)
        ^books[id] = Book(title: title, shelf: shelf)
        ^books[id].notes[1] = Book.notes(text: "catalogued", at: at)
    }
    return true
}

pub fn catalogued(): int {
    return ^tallies["catalogued"].count ?? 0
}

pub fn noteCount(): int {
    var total = 0
    for id, book in ^books at most 4096 {
        if exists(book) {
            for seq, entry in ^books[id].notes at most 4096 {
                if const note = entry {
                    total += 1
                }
            } on more {
                return -1
            }
        }
    } on more {
        return -1
    }
    return total
}

pub fn countOnShelf(shelf: string): int {
    var total = 0
    for bookId in ^books.byShelf[shelf] at most 4096 {
        if exists(^books[bookId]) {
            total += 1
        }
    } on more {
        return -1
    }
    return total
}

test "books and counters commit together" {
    assert add(1, "Small Gods", "fiction", instant("2026-07-18T09:00:00Z"))
    assert not add(1, "Pyramids", "fiction", instant("2026-07-18T10:00:00Z"))
    assert catalogued() == 1
    assert noteCount() == 1
    assert countOnShelf("fiction") == 1
}
```

## One transaction over two roots

`add` writes the counter, the book, and the book's first note in one
`transaction`. The duplicate-key guard returns before those writes. The three
writes therefore commit together for a new id, while a duplicate changes
neither root. [Errors and transactions](language/errors-and-transactions.md)
defines the commit and return rules.

## Nested bounded traversal

`noteCount` walks `^books`, then the `notes` branch under each present book. The
outer and inner loops each state an independent bound and handle `on more`; this
example returns `-1` when either walk has more entries. [Bounded durable
traversal](language/traversal-and-indexes.md#bounded-durable-traversal) defines
the ordering, pins, bounds, and continuation form.

`countOnShelf` walks the `byShelf` index instead of the whole root. Each result
is a root-local book identity, which addresses the corresponding `^books`
entry. [Reading an index](language/traversal-and-indexes.md#reading-an-index)
defines index lookup and traversal.

## Reference rules

- [Durable places](language/durable-places.md) defines roots, reads, presence
  proofs, writes, replacement, and deletion.
- [Traversal and indexes](language/traversal-and-indexes.md) defines bounded
  root, branch, and index walks.
- [Errors and transactions](language/errors-and-transactions.md) defines commit,
  rollback, and interrupted outcomes.
- [Tests](language/tests.md#durable-tests) defines the fresh store used by the
  test block.
