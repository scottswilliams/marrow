# Reference Sample

This small shelf project exercises the main Marrow language/database surface.
It is the sample shape used by docs, conformance tests, and tool output.

```mw
module shelf::sample

resource Book at ^books(id: int)
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

    index byShelf(shelf, id)

pub fn add(title: string, author: string, shelf: string, changedAt: instant): Book::Id
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf
    book.currentVersion = 1

    let id: Book::Id = nextId(^books)

    transaction
        ^books(id) = book
        ^books(id).versions(1).title = title
        ^books(id).versions(1).shelf = shelf
        ^books(id).versions(1).changedAt = changedAt

    return id

pub fn moveToShelf(id: Book::Id, shelf: string, changedAt: instant)
    transaction
        let version: int = ^books(id).currentVersion + 1
        ^books(id).shelf = shelf
        ^books(id).currentVersion = version
        ^books(id).versions(version).title = ^books(id).title
        ^books(id).versions(version).shelf = shelf
        ^books(id).versions(version).changedAt = changedAt

pub fn addNote(id: Book::Id, noteId: string, text: string): bool
    if not exists(^books(id))
        return false

    ^books(id).notes(noteId).text = text
    return true

pub fn addTag(id: Book::Id, tag: string): int
    if not exists(^books(id))
        return 0

    return append(^books(id).tags, tag)

pub fn copyTags(from: Book::Id, to: Book::Id): bool
    if not exists(^books(from)) or not exists(^books(to))
        return false

    merge ^books(to).tags = ^books(from).tags
    return true

pub fn remove(id: Book::Id)
    delete ^books(id)

pub fn printShelf(shelf: string)
    for id in keys(^books.byShelf(shelf))
        print($"{id}: {^books(id).title}")

pub fn main()
    let now: instant = std::clock::now()
    let id = add(
        title: "Small Gods",
        author: "Terry Pratchett",
        shelf: "fiction",
        changedAt: now,
    )
    addTag(id, "favorite")
    printShelf("fiction")
```

The sample covers:

- a runnable public entrypoint;
- identity allocation with `nextId`;
- required fields and sparse fields;
- keyed child layers such as `notes(noteId)`;
- sequence append with `append`;
- child key values that cannot collide with generated index names;
- explicit history entry creation;
- transaction-built history entries with required fields;
- managed assignment, `merge`, and `delete`;
- generated index traversal through `^books.byShelf(...)`;
- a transaction that updates primary data and generated index entries.
