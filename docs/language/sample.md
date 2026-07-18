# Reference Sample

This shelf module combines resources, durable paths, typed identities,
transactions, keyed children, presence reads, and index traversal in one
checkable program.

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
    if const currentVersion = ^books[id].currentVersion {
        if const title = ^books[id].title {
            transaction {
                const version: int = currentVersion + 1
                ^books[id].shelf = shelf
                ^books[id].currentVersion = version
                ^books[id].versions[version].title = title
                ^books[id].versions[version].shelf = shelf
                ^books[id].versions[version].changedAt = changedAt
            }
            return true
        }
    }
    return false
}

pub fn addNote(id: Id(^books), noteId: string, text: string): bool {
    if not exists(^books[id]) { return false }

    transaction {
        ^books[id].notes[noteId].text = text
    }
    return true
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
    } on more return found
    return found
}

pub fn label(id: Id(^books)): string {
    if const title = ^books[id].title {
        return $"{id}: {title}"
    }
    return $"{id}: (absent)"
}
```

The caller supplies the integer identity as an `Id(^books)`, and `add` writes
the entry and its first history version in one transaction. Every durable write
sits in a transaction; a durable field read outside a transaction is a presence
read, not a write. The durable write is what makes a key present, so a later
`exists(^books[id])` reports it.

The example also shows required and sparse fields, keyed resource children
(`notes` and `versions`), guarded reads with `if const`, exact path deletion,
string interpolation, and ordered traversal through `^books.byShelf[...]` with an
explicit `at most` bound and an `on more` arm. Each write that changes `shelf`
updates the declared index as part of the same durable operation.
