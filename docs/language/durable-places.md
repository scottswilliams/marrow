# Durable places

A durable place is a location whose value outlives the program. It is written
with `^` and read, assigned, and deleted like a local value.

## Declaring a store

Declare the shape, then the store:

```mw
module docs::durable::shelf

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn put(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn title(id: int): string? {
    return ^books[id].title
}

test "put then read" {
    put(1, "Small Gods")
    assert title(1) ?? "" == "Small Gods"
    assert title(2) ?? "none" == "none"
}
```

`resource Book` is an ordinary value shape. `store ^books[id: int]: Book` gives
it a durable root keyed by an `int`. `^books[id]` is one entry. `^books[id].title`
is one field of that entry.

`put` writes inside a `transaction`. When the block ends, its writes commit
together. `title` reads without one. The read yields `string?` because the entry
may be absent, and the test supplies a default with `??`. Every test starts in a
fresh store ([tests](tests.md#durable-tests)).

A store root is visible from every module of the project by its name; `pub`
applies to functions only ([visibility](modules-and-functions.md#visibility)).

## Keys

A key is an `int`, `string`, `bool`, `bytes`, `date`, or `instant`. `duration`
and optional types are not keys. Nominal source types are not durable identity
keys and report `check.unsupported`.

A root may take several key components. `store ^copies[isbn: string, number: int]:
Copy` names one entry by the whole tuple, `^copies[isbn, number]`, in
declaration order. Every read and write supplies one operand per component. A
key tuple has at most 8 components. `for` iterates one key component, so a
composite-keyed layer is addressed and not iterated
([traversal](traversal-and-indexes.md#bounded-durable-traversal)).

A project declares as many roots as it needs, each with its own name. One
transaction may write across several roots, and the writes commit together. Two
roots may name the same resource. Each then holds its own entries, and a write
through one is invisible through the other.

An entry identity stands in for the whole key. `^books[Id(^books, 1)]` names the
same entry as `^books[1]`, so an identity found through an index is a read or
write address ([entry identity](types-and-values.md#entry-identity)).

## What a field holds

A field holds a scalar, a `struct`, an `enum`, an `Option`, or a `Result`. It
holds no list, map, resource, place, or function; many values under one entry
go in a [keyed branch](#keyed-branches). A stored value nests at most 32 levels
([limits](execution-limits.md#limits)).

## Reading

A durable read yields `T?`, because the entry or the field may be absent
([optionals](types-and-values.md#optionals)):

```mw
module docs::durable::reading

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn titleOrNone(id: int): string {
    if const book = ^books[id] {
        return book.title
    }
    return "none"
}

test "an absent entry reads absent" {
    ^books[1] = Book(title: "Small Gods")
    assert exists(^books[1])
    assert not exists(^books[2])
    assert ^books[1].subtitle ?? "none" == "none"
    assert titleOrNone(2) == "none"
}
```

`^books[1].subtitle` is absent: the entry is present and the field is not.
`titleOrNone` binds the whole entry with `if const`. Inside the block
`book.title` is a plain `string`, because a present entry has every required
field. `exists` answers presence with a `bool` and narrows nothing; a read after
an `exists` guard is still optional.

`exists(^books)` is true when some entry of `^books` has fields of its own, and
`exists(^books[id].notes)` asks the same of a branch.

The test writes `^books[1]` with a bare statement. A test body owns no
transaction: it touches durable data directly, or it drives exports that do,
never both ([tests](tests.md#durable-tests)).

## Writing

A write sits inside a `transaction` block owned by the exported function. The
block's writes commit together when it ends, and a `return` inside the block
commits them ([transactions](errors-and-transactions.md#transactions)).

Assigning one field changes that field and leaves the others as they are:

```mw
module docs::durable::fields

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn create(id: int, title: string) {
    transaction {
        ^books[id].title = title
    }
}

pub fn retitle(id: int, title: string): string? {
    transaction {
        ^books[id].title = title
        return ^books[id].title
    }
}

pub fn present(id: int): bool {
    return exists(^books[id])
}

test "a field write creates the entry" {
    create(1, "Small Gods")
    assert present(1)
    assert retitle(1, "Pyramids") ?? "" == "Pyramids"
}
```

`create` writes one field of an entry that does not exist yet. The write is
staged, and at commit the entry is created because every required field is
present. A required field left unset at commit rolls the whole block back with
`run.required_missing`. `retitle` reads the field it just wrote: inside the
block, a read sees the writes staged before it.

A sparse field may stay unset, and `delete` clears it ([deleting](#deleting)).
A required field is present at every commit.

Assigning a whole entry replaces its fields exactly:

```mw
module docs::durable::replace

resource Book {
    required title: string
    subtitle: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

pub fn replace(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

test "replacement and delete leave the branch in place" {
    ^books[1] = Book(title: "Small Gods", subtitle: "A novel")
    ^books[1].notes[1] = Book.notes(text: "signed")
    ^books[1] = Book(title: "Pyramids")
    assert ^books[1].subtitle ?? "none" == "none"
    assert ^books[1].notes[1].text ?? "" == "signed"
    delete ^books[1]
    assert not exists(^books[1])
    assert ^books[1].notes[1].text ?? "" == "signed"
}
```

`Book(title: "Pyramids")` names no `subtitle`, so the replacement drops it.
`Book.notes(text: "signed")` builds one entry of the `notes` branch. The note
under `notes[1]` stays: a keyed branch is its own node, and a whole-entry
assignment touches only the entry's own fields. A constructor that omits a
required field is a `check.type` error. The last three lines belong to
[deleting](#deleting).

## Named places

`place` binds an entry address to a name:

```mw
module docs::durable::named

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn setSubtitle(id: int, subtitle: string): bool {
    transaction {
        place book = ^books[id]
        if not exists(book) {
            return false
        }
        book.subtitle = subtitle
    }
    return true
}

pub fn put(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn subtitleOf(id: int): string? {
    place book = ^books[id]
    return book.subtitle
}

test "a place writes one field" {
    put(1, "Small Gods")
    assert setSubtitle(1, "A novel")
    assert not setSubtitle(2, "A novel")
    assert subtitleOf(1) ?? "" == "A novel"
}
```

The right-hand side is a whole entry address, `^root[key]`. The key is evaluated
once, at the binding, and every later use of the name goes to that one address.
A place proves nothing. `exists(book)` proves presence, and the guard returns
before any write when the entry is absent. `book.subtitle = subtitle` then writes
one field and reads nothing else.

A branch beneath the entry is addressed through the name, so `book.notes[pos]`
reads and writes the branch entry that `^books[id].notes[pos]` names.

A place is a constant, and its bare name is not a value: read a field through
it, bind the whole entry with `if const`, or test it with `exists`. A field
address or another place on the right-hand side is a `check.type` error.

Two bindings to the same entry keep separate proofs. Deleting through one leaves
the other's proof stale: a sparse field written through it leaves the entry short
of a required field, so the block rolls back with `run.required_missing`. Prove
presence, delete, and write through one binding.

## Groups

A group is a named set of fields inside the entry:

```mw
module docs::durable::group

resource Book {
    required title: string

    details {
        pages: int
        language: string
    }
}

store ^books[id: int]: Book

pub fn pages(id: int): int? {
    return ^books[id].details.pages
}

test "a group is one value of the entry" {
    ^books[1] = Book(title: "Small Gods", details: Book.details(pages: 381, language: "en"))
    ^books[1].details.pages = 400
    assert ^books[1].details.language ?? "" == "en"
    ^books[1].details = Book.details(language: "de")
    assert ^books[1].details.pages ?? 0 == 0
    assert ^books[1].title ?? "" == "Small Gods"
}
```

`details` is part of the entry: it is present when the entry is present, and it
is addressed by the entry's key. `^books[id].details.pages` reads one leaf and
yields `int?`. The test writes one leaf and keeps `language`, then assigns the
whole group exactly, so the omitted `pages` is dropped. `title` is untouched
either way. A group leaf write over an absent entry writes nothing, while a
field write stages the field and creates the entry at commit.

## Keyed branches

A branch is a keyed family of entries inside an entry:

```mw
module docs::durable::branch

resource Book {
    required title: string

    notes[pos: int] {
        required text: string
        pinned: bool

        tags[tag: string] {
            required weight: int
        }
    }
}

store ^books[id: int]: Book

pub fn noteText(id: int, pos: int): string? {
    return ^books[id].notes[pos].text
}

test "a branch entry is its own node" {
    ^books[1].notes[1].tags["gift"] = Book.notes.tags(weight: 2)
    assert ^books[1].notes[1].tags["gift"].weight ?? 0 == 2
    assert not exists(^books[1].notes[1])
    assert not exists(^books[1])
    ^books[1].notes[1] = Book.notes(text: "signed")
    assert exists(^books[1].notes[1])
    assert ^books[1].notes[1].tags["gift"].weight ?? 0 == 2
}
```

`^books[id].notes[pos]` extends the entry's key with the branch key, and
`.tags[tag]` extends it again, one level down per branch. A branch holds scalar
fields and further branches, nested at most 16 levels. Every operation on an
entry applies to a branch entry at its own address.

The test writes a tag under a note and a book that do not exist. The write is
admitted. Each ancestor is then descendant-only: it has keyed descendants and no
fields of its own, so `exists` is false for it. Giving the note its own fields
later leaves the tag in place.

## Deleting

`delete` removes a sparse field or an entry's own fields. `delete
^books[id].subtitle` clears the field, and clearing a field that is already
absent does nothing. Deleting a required field is a `check.type` error. `delete
^books[id]` removes the entry's own fields, and `exists(^books[id])` turns
false. A note under the entry stays, because a branch entry is its own node.

Removing an entry with everything beneath it is a bounded traversal that deletes
each node:

```mw
module docs::durable::purge

resource Book {
    required title: string

    notes[pos: int] {
        required text: string

        tags[tag: string] {
            required weight: int
        }
    }
}

store ^books[id: int]: Book

pub fn seed(id: int) {
    transaction {
        ^books[id] = Book(title: "Small Gods")
        ^books[id].notes[1] = Book.notes(text: "signed")
        ^books[id].notes[1].tags["gift"] = Book.notes.tags(weight: 2)
    }
}

pub fn purge(id: int) {
    transaction {
        for pos, note in ^books[id].notes at most 1000 {
            for tag, entry in note.tags at most 1000 {
                delete entry
            } on more {}
            delete note
        } on more {}
        delete ^books[id]
    }
}

pub fn hasNotes(id: int): bool {
    return exists(^books[id].notes)
}

test "purge empties the subtree" {
    seed(1)
    assert hasNotes(1)
    purge(1)
    assert not hasNotes(1)
}
```

The inner loop deletes each tag through the entry binding its loop head
declares, the outer loop deletes each note, and the last statement deletes the
book. Each `for` head states its bound, so one transaction removes as much as
its bounds admit and `on more` observes the rest
([traversal](traversal-and-indexes.md#bounded-durable-traversal)). There is no
whole-subtree delete.

## Access demand

Every export has a demand: the durable places it reads and writes, through every
function it calls. `marrow check --demand .` prints it for the first example:

```text
docs.durable.shelf.put reads ^books; writes ^books
docs.durable.shelf.title reads ^books.title
```

A whole-entry write is listed as a read and a write. Demand describes the access
a program requires; it grants nothing
([`marrow check`](../tools/cli.md)).

## Durable identity

Every durable declaration, from the root down to each field, has an identity: a
128-bit id minted once and recorded in `.marrow/ids`. The first storeless
`marrow run <export>` mints the ids a project lacks, and the file is committed
with the source so every checkout reuses them. `marrow check` and `marrow test`
on an unminted project report `check.durable_identity`:

```text
src/docs/durable/shelf.mw:8:7: check.durable_identity: durable identity for root `books` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)
```

Identity follows the id. A declaration keeps its identity through reordering and
respelling of the source around it. A renamed declaration gets a fresh id, and
the old entry stays in the ledger. The ids and the shape of the whole graph form
the program's durable contract, which a store on disk compares with its own
before it runs the program
([identity ledger](../tools/projects.md#identity-ledger),
[changing the program](../operations/README.md#changing-the-program)).

Today, keyed roots, their groups, and their branches read and write end to end.
A singleton root such as `store ^settings: Settings`, a root whose resource holds
a nominal field, and a group inside another group or a branch are future work
([status](../status.md#not-yet-available)).
