# Reference Sample

This small shelf project exercises the main Marrow language/database surface.
It is the sample shape used by docs, conformance tests, and tool output.

```mw
module shelf::sample

resource Book
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string
    tags(pos: int): string

    notes(noteId: string)
        text: string

    versions(version: int)
        required title: string
        required shelf: string
        required changedAt: instant

store ^books(id: int): Book
    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string, changedAt: instant): Id(^books)
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    book.currentVersion = 1

    const id: Id(^books) = nextId(^books)

    transaction
        ^books(id) = book
        ^books(id).versions(1).title = title
        ^books(id).versions(1).shelf = shelf
        ^books(id).versions(1).changedAt = changedAt

    return id

pub fn moveToShelf(id: Id(^books), shelf: string, changedAt: instant)
    if not exists(^books(id))
        return

    if const currentVersion = ^books(id).currentVersion
        if const title = ^books(id).title
            transaction
                const version: int = currentVersion + 1
                ^books(id).shelf = shelf
                ^books(id).currentVersion = version
                ^books(id).versions(version).title = title
                ^books(id).versions(version).shelf = shelf
                ^books(id).versions(version).changedAt = changedAt

pub fn addNote(id: Id(^books), noteId: string, text: string): bool
    if not exists(^books(id))
        return false

    ^books(id).notes(noteId).text = text
    return true

pub fn addTag(id: Id(^books), tag: string): int
    if not exists(^books(id))
        return 0

    return append(^books(id).tags, tag)

pub fn remove(id: Id(^books))
    delete ^books(id)

pub fn printShelf(shelf: string)
    for id in ^books.byShelf(shelf)
        if const title = ^books(id).title
            print($"{id}: {title}")

pub fn main()
    const now: instant = std::clock::now()
    const id = add(
        title: "Small Gods",
        author: "Terry Pratchett",
        shelf: "fiction",
        changedAt: now,
    )
    append(^books(id).tags, "favorite")
    printShelf("fiction")
```

The sample covers:

- a runnable public entrypoint;
- identity allocation with `nextId`;
- required fields and sparse fields;
- keyed child layers such as `notes(noteId)`;
- sequence append with `append`;
- child key values that cannot collide with declared index names;
- explicit history entry creation;
- transaction-built history entries with required fields;
- managed assignment and `delete`;
- declared index traversal through `^books.byShelf(...)`;
- a transaction that changes primary data and generated index entries.
