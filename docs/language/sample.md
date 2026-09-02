# A library catalog

This module keeps a small catalog: books on shelves, notes on each book, and a history of every shelf move. It is one complete file that uses most of the language together.

```mw
module shelf::sample

resource Book {
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string

    notes[noteId: string] {
        required text: string
    }

    versions[version: int] {
        required title: string
        required shelf: string
        required changedAt: instant
    }
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
}

pub fn add(id: Id(^books), title: string, author: string, shelf: string, changedAt: instant) {
    transaction {
        ^books[id].title = title
        ^books[id].author = author
        ^books[id].shelf = shelf
        ^books[id].currentVersion = 1
        ^books[id].versions[1].title = title
        ^books[id].versions[1].shelf = shelf
        ^books[id].versions[1].changedAt = changedAt
    }
}

pub fn moveToShelf(id: Id(^books), shelf: string, changedAt: instant): bool {
    transaction {
        if const currentVersion = ^books[id].currentVersion {
            if const title = ^books[id].title {
                const version: int = currentVersion + 1
                ^books[id].shelf = shelf
                ^books[id].currentVersion = version
                ^books[id].versions[version].title = title
                ^books[id].versions[version].shelf = shelf
                ^books[id].versions[version].changedAt = changedAt
                return true
            }
        }
        return false
    }
}

pub fn addNote(id: Id(^books), noteId: string, text: string): bool {
    transaction {
        if not exists(^books[id]) {
            return false
        }
        ^books[id].notes[noteId].text = text
        return true
    }
}

pub fn remove(id: Id(^books)) {
    transaction {
        delete ^books[id]
    }
}

pub fn shelfCount(shelf: string): int {
    var found: int = 0
    for id in ^books.byShelf[shelf] at most 100 {
        found += 1
    } on more {
        return found
    }
    return found
}

pub fn label(id: Id(^books)): string {
    if const title = ^books[id].title {
        return $"{id}: {title}"
    }
    return $"{id}: (absent)"
}
```

`resource Book` is the shape of one book. `title`, `author`, `shelf`, and `currentVersion` are [required](resources.md#fields), so every stored book has them. `loanedTo` is sparse: it is absent until a program assigns it.

`notes` and `versions` are [keyed branches](durable-places.md#keyed-branches). Each book carries its own notes keyed by a `string` and its own history keyed by a version number. `^books[id].versions[2].shelf` is one field of one version of one book.

`store ^books[id: int]: Book` gives the shape a durable root keyed by an `int`. `index byShelf[shelf, id]` adds a second path to the same entries, ordered by shelf and then by identity.

`add` takes the identity as an [`Id(^books)`](types-and-values.md#entry-identity); the caller chooses it, and `Id(^books, 1)` spells the first one. The block writes the book and its first version, and the writes commit together when the block ends. `title` is required, so a block that leaves it unset rolls back with `run.required_missing`.

`moveToShelf` reads before it writes. `if const` proves `currentVersion` and `title` present and binds them. The new version number, the shelf, and the history entry are written in one [transaction](errors-and-transactions.md#transactions), and `return true` commits it. If either field is absent, the block returns `false` and writes nothing. After a move, `^books[id].versions[1].shelf` still reads the old shelf: the history keeps every version.

`addNote` checks `exists(^books[id])` before writing under the book. `add` has already committed by then, so the entry is present and the note is written. For an absent book, `addNote` returns `false`.

`remove` deletes the book's own fields. Its `notes` and `versions` stay at their own addresses until a program deletes them there ([deleting](durable-places.md#deleting)). After `remove`, `label` reports the book absent, `shelfCount` no longer counts it, and `^books[id].notes["n1"].text` still reads its value.

`shelfCount` [walks the index](traversal-and-indexes.md#reading-an-index). `^books.byShelf[shelf]` yields the identity of each book on that shelf, at most 100 of them. The `on more` arm runs when a 101st exists. The index follows every write to `shelf`: after `moveToShelf(id, "classics", at)`, the book counts under `classics` and no longer under `fiction`.

`label` reads outside any transaction. A read needs no transaction and sees the last committed state. `$"{id}: {title}"` renders the identity without its root, so `label(Id(^books, 1))` returns `Id(1): Small Gods`, and an absent book renders as `Id(1): (absent)`.

`marrow check --demand .` lists the durable places each export reads and writes:

```text
shelf.sample.add writes ^books.author, ^books.currentVersion, ^books.shelf, ^books.title, ^books.versions.changedAt, ^books.versions.shelf, and ^books.versions.title
shelf.sample.addNote reads ^books; writes ^books.notes.text
shelf.sample.label reads ^books.title
shelf.sample.moveToShelf reads ^books.currentVersion and ^books.title; writes ^books.currentVersion, ^books.shelf, ^books.versions.changedAt, ^books.versions.shelf, and ^books.versions.title
shelf.sample.remove writes ^books
shelf.sample.shelfCount reads ^books.byShelf
```

`remove` demands the whole entry. Every other export names the exact fields it touches, and `shelfCount` touches only the index.
