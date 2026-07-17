# Reference Sample

This shelf module combines resources, durable paths, typed identities,
transactions, keyed children, positional leaves, and index traversal.

```mw
module shelf::sample

resource Book {
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string
    tags[pos: int]: string

    notes[noteId: string] {
        text: string
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

pub fn add(title: string, author: string, shelf: string, changedAt: instant): Id(^books) {
    const id: Id(^books) = nextId(^books)

    transaction {
        ^books[id].title = title
        ^books[id].author = author
        ^books[id].shelf = shelf
        ^books[id].currentVersion = 1
        ^books[id].versions[1].title = title
        ^books[id].versions[1].shelf = shelf
        ^books[id].versions[1].changedAt = changedAt
    }

    return id
}

pub fn moveToShelf(id: Id(^books), shelf: string, changedAt: instant) {
    if not exists(^books[id]) { return }

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
        }
    }
}

pub fn addNote(id: Id(^books), noteId: string, text: string): bool {
    if not exists(^books[id]) { return false }

    ^books[id].notes[noteId].text = text
    return true
}

pub fn addTag(id: Id(^books), tag: string): int {
    if not exists(^books[id]) { return 0 }

    return append(^books[id].tags, tag)
}

pub fn remove(id: Id(^books)) {
    delete ^books[id]
}

pub fn printShelf(shelf: string) {
    for id in ^books.byShelf[shelf] at most 100 {
        if const title = ^books[id].title {
            print($"{id}: {title}")
        }
    } on more {
        print("more shelved books remain")
    }
}

pub fn main() {
    const now: instant = instant("2026-07-15T12:00:00Z")
    const id = add(
        title: "Small Gods",
        author: "Terry Pratchett",
        shelf: "fiction",
        changedAt: now,
    )
    append(^books[id].tags, "favorite")
    printShelf("fiction")
}
```

The `add` function obtains an integer identity candidate with `nextId` and
writes the entry in the same transaction. `nextId` does not reserve its result;
the durable write is what makes that key present.

The example also shows required and sparse fields, keyed resource children,
1-based positional append, explicit history entries, exact path deletion, and
ordered traversal through `^books.byShelf[...]`. Each write that changes
`shelf` updates the declared index as part of the same durable operation.
