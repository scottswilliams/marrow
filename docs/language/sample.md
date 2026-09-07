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
        ^books[id] = Book(title: title, author: author, shelf: shelf, currentVersion: 1)
        ^books[id].versions[1] = Book.versions(title: title, shelf: shelf, changedAt: changedAt)
    }
}

pub fn moveToShelf(id: Id(^books), shelf: string, changedAt: instant): bool {
    transaction {
        place book = ^books[id]
        if const current = book {
            const version: int = current.currentVersion + 1
            book.shelf = shelf
            book.currentVersion = version
            book.versions[version] = Book.versions(title: current.title, shelf: shelf, changedAt: changedAt)
            return true
        }
        return false
    }
}

pub fn addNote(id: Id(^books), noteId: string, text: string): bool {
    transaction {
        if not exists(^books[id]) {
            return false
        }
        ^books[id].notes[noteId] = Book.notes(text: text)
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

`add` takes the identity as an [`Id(^books)`](types-and-values.md#entry-identity); the caller chooses it, and `Id(^books, 1)` spells the first one. The block writes the book and its first version as whole entries, and the writes commit together when the block ends. Each constructor names every required field, so a present entry is complete from its first commit; a constructor that omits one is a `check.type` error ([writing](durable-places.md#writing)).

`moveToShelf` binds `place book = ^books[id]` and reads before it writes. `if const current = book` proves the entry present and binds a copy, so `current.currentVersion` and `current.title` are bare values. Inside that block the shelf, the new version number, and the history entry are written through the proved place in one [transaction](errors-and-transactions.md#transactions), and `return true` commits it. For an absent book, the block returns `false` and writes nothing. After a move, `^books[id].versions[1].shelf` still reads the old shelf: the history keeps every version.

`addNote` checks `exists(^books[id])` before writing under the book, and writes the note as a whole branch entry. `add` has already committed by then, so the entry is present and the note is written. For an absent book, `addNote` returns `false`.

`remove` deletes the book's own fields. Its `notes` and `versions` stay at their own addresses until a program deletes them there ([deleting](durable-places.md#deleting)). After `remove`, `label` reports the book absent, `shelfCount` no longer counts it, and `^books[id].notes["n1"].text` still reads its value.

`shelfCount` [walks the index](traversal-and-indexes.md#reading-an-index). `^books.byShelf[shelf]` yields the identity of each book on that shelf, at most 100 of them. The `on more` arm runs when a 101st exists. The index follows every write to `shelf`: after `moveToShelf(id, "classics", at)`, the book counts under `classics` and no longer under `fiction`.

`label` reads outside any transaction. A read needs no transaction and sees the last committed state. `$"{id}: {title}"` renders the identity without its root, so `label(Id(^books, 1))` returns `Id(1): Small Gods`, and an absent book renders as `Id(1): (absent)`.

`marrow check --demand .` lists the durable places each export reads and writes:

```text
shelf.sample.add reads ^books and ^books.versions; writes ^books and ^books.versions
shelf.sample.addNote reads ^books and ^books.notes; writes ^books.notes
shelf.sample.label reads ^books.title
shelf.sample.moveToShelf reads ^books and ^books.versions; writes ^books.currentVersion, ^books.shelf, and ^books.versions
shelf.sample.remove writes ^books
shelf.sample.shelfCount reads ^books.byShelf
```

A whole-entry write is listed as a read and a write of its family, so `add`, `addNote`, and `remove` demand whole entries. `moveToShelf` names the two fields it updates and the `versions` family it creates an entry in, and `shelfCount` touches only the index.
