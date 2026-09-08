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
key tuple has at most 8 components, each with its own name; a component that
repeats another is a `check.name_conflict` at that component. A root's key
names are the store's own and may coincide with a field of the resource. `for`
iterates one key component, so a composite-keyed layer is addressed and not
iterated ([traversal](traversal-and-indexes.md#bounded-durable-traversal)).

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

An untested durable read yields `T?`, because the entry or field may be absent
([optionals](types-and-values.md#optionals)). A required field or required group
leaf read through a [proved named place](#named-places) has its declared type;
sparse reads remain optional:

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
field. `exists` answers presence with a `bool`. An explicit guard over a named
place establishes a presence proof; a guard over an inline path does not change
the types of later reads.

`exists(^books)` is true when `^books` has a present immediate entry, including
an entry whose fields are all sparse and unset. `exists(^books[id].notes)` asks
the same of that branch, whether or not the book entry is present. Each family
test uses one bounded scan and faults on encountered own payload without its
entry marker ([traversal](traversal-and-indexes.md#bounded-durable-traversal)).

The test writes `^books[1]` with a bare statement. A test body owns no
transaction: it touches durable data directly, or it drives exports that do,
never both ([tests](tests.md#durable-tests)).

## Writing

A write sits inside a `transaction` block owned by the exported function. The
block's writes commit together when it ends, and a `return` inside the block
commits them ([transactions](errors-and-transactions.md#transactions)).

An entry is written whole, and one field of a present entry is written through
a proved place:

```mw
module docs::durable::fields

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn create(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn retitle(id: int, title: string): string? {
    transaction {
        place m = ^books[id]
        if exists(m) {
            m.title = title
        }
        return m.title
    }
}

pub fn present(id: int): bool {
    return exists(^books[id])
}

test "a field write updates a present entry" {
    create(1, "Small Gods")
    assert present(1)
    assert retitle(1, "Pyramids") ?? "" == "Pyramids"
    assert retitle(2, "Pyramids") ?? "none" == "none"
    assert not present(2)
}
```

`create` assigns a whole entry. The constructor names every required field, so
the entry is complete from its first commit, and a present entry is always
complete. `retitle` binds `place m = ^books[id]`, proves the entry present
with `exists(m)`, and writes one field through `m` inside that block. A field
write updates a present entry and never creates one: `retitle(2, "Pyramids")`
writes nothing, and `present(2)` stays false. The read of `m.title` after the
write sees the write staged before it and is `string?` because the guard's block
has ended
([named places](#named-places)).

A sparse field may stay unset, and `delete` clears it ([deleting](#deleting)).
A required field is present whenever its entry is.

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

pub fn titleOf(id: int): string {
    place book = ^books[id]
    if not exists(book) { return "none" }
    const title: string = book.title
    return title
}

test "a place writes one field" {
    put(1, "Small Gods")
    assert setSubtitle(1, "A novel")
    assert not setSubtitle(2, "A novel")
    assert subtitleOf(1) ?? "" == "A novel"
    assert titleOf(1) == "Small Gods"
    assert titleOf(2) == "none"
}
```

The right-hand side is a whole entry address, `^root[key]`. The key is evaluated
once, at the binding, and every later use of the name goes to that one address.
`exists(book)` proves presence, and the guard returns before any write when the
entry is absent. `book.subtitle = subtitle` then writes one field and reads
nothing else.

A field write, a whole-group write, or a group-leaf write goes through a place,
or a [traversal pin](traversal-and-indexes.md#bounded-durable-traversal), that
a presence proof covers. The proof forms are:

- the block of `if exists(p)`;
- the block of `if const x = p`;
- the rest of the block after `const x = p else { … }`;
- the rest of the block after `if not exists(p) { … }` when that block returns
  or throws, as in `setSubtitle`;
- the rest of the block after a whole-entry assignment `p = Book(…)`.

A negative guard whose block falls through proves nothing. A write with no
proof, including every inline `^books[id].subtitle = subtitle`, is
`check.requires_presence` at the write. A function binds one place per entry
and proves and writes through that name.

Within the proof's scope, a required field or required group leaf read through
that name has its declared type. This includes a required struct, enum,
`Option`, or `Result` field where that field shape is supported. Read a struct
field into a local value before projecting its members. Sparse fields remain
optional. Whole-entry and whole-group value reads remain optional.

An untested place supports optional reads. If a required read's proof is
invalidated within its lexical scope, the read is `check.requires_presence`,
even where an optional value would fit. It needs a fresh guard or whole-entry
assignment. After an inner proof block ends, an outer untested place supports
optional reads again. Values already copied from a place remain ordinary values.

A proof lasts until its block ends or an entry in the same family is erased,
directly or through a call. A family is one root and one branch path, so the
erase may use any binding or key: `delete book`, `delete other` over the same
root, or `delete ^books[k]`. Parent and child entry families are independent.
Complete replacement, field and group updates, and deletion of sparse leaves
preserve entry presence. The proof establishes no stable field value or sparse
field presence.

A write evaluates its right-hand side before consuming the proof. A helper
that erases the family invalidates the proof even when it recreates the entry
or returns an ordinary error value; such a return does not roll back the
transaction. A completed whole-entry assignment through a named place
establishes a new proof after its right-hand side has been evaluated.

A protected read or write inside a `while` or `for` body entered after the proof was established
also requires that proof to survive the repeating region, including nested
bodies and a `while` condition. An entry erase in that region, directly or
through a call, can precede the use on the next iteration and invalidates
the proof. A proof established inside the body, such as `if exists(pin)` on
each iteration, starts a new lifetime. One-time loop inputs follow ordinary
evaluation order. After an erasing loop, an earlier proof cannot be reused.
A traversal pin requires an explicit guard on each iteration to read required
fields bare; traversal and index results establish no automatic presence proof.

A branch beneath the entry is addressed through the name, so `book.notes[pos]`
reads and writes the branch entry that `^books[id].notes[pos]` names. A place
over a branch entry, `place n = ^books[id].notes[pos]`, is proved and written
by the same forms, and its whole-entry assignment proves it for the rest of
the block.

A place is a constant, and its bare name is not a value: read a field through
it, bind the whole entry with `if const`, or test it with `exists`. A field
address or another place on the right-hand side is a `check.type` error.
`reads` and `writes` are reserved words and do not name a place.

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
    place b = ^books[1]
    b = Book(title: "Small Gods", details: Book.details(pages: 381, language: "en"))
    b.details.pages = 400
    assert b.details.language ?? "" == "en"
    b.details = Book.details(language: "de")
    assert b.details.pages ?? 0 == 0
    assert b.title == "Small Gods"
    delete b.details.language
    assert b.details.language ?? "none" == "none"
}
```

`details` is part of the entry: it is present exactly when the entry is, and it
is addressed by the entry's key. `^books[id].details.pages` reads one leaf and
yields `int?`. The test binds `place b = ^books[1]`, and the whole-entry
assignment through `b` proves it for the rest of the body, so the group writes
that follow need no further guard. The test writes one leaf and keeps
`language`, then assigns the whole group exactly, so the omitted `pages` is
dropped. `title` is untouched either way, and `delete b.details.language`
clears one sparse leaf. A group whose leaves are all sparse is cleared with
`delete b.details`; a group that declares a required leaf is erased only with
its entry ([deleting](#deleting)).

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
admitted: each branch entry has its own presence, independent of its ancestors.
`exists` remains false for the note and book. Creating the note later leaves
the tag in place. A program can address or traverse a branch beneath an absent
ancestor when it supplies that ancestor's keys.

## Deleting

`delete` is the one way to clear durable state, and it needs no presence
proof. `delete ^books[id].subtitle` clears a sparse field, `delete
^books[id].details.language` a sparse group leaf, `delete ^books[id].details`
a group whose leaves are all sparse, and `delete ^books[id]` the entry's own
payload, after which `exists(^books[id])` is false. Each form is also written
through a place, `delete book.subtitle`. Clearing a field that is already
absent does nothing. A note under the entry stays, because a branch entry is
its own node.

A durable field set takes a definite value of the field's type. Assigning
`absent`, or an operand of an optional type `T?`, is a `check.type` error
whose message names `delete`. Deleting a required field is a `check.type`
error, and so is `delete` of a group that declares a required leaf: a required
field is present whenever its entry is, so such a group is erased only with
its entry.

Deleting visited entries uses nested bounded traversals. These traversals visit
only present entry payloads, so they do not by themselves implement
complete subtree removal:

```mw
module docs::durable::visited

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
        ^books[id].notes[2].tags["gift"] = Book.notes.tags(weight: 3)
    }
}

pub fn removePresentEntries(id: int) {
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

pub fn tagWeight(id: int, pos: int): int? {
    return ^books[id].notes[pos].tags["gift"].weight
}

test "a descendant-only note is not visited" {
    seed(1)
    assert hasNotes(1)
    removePresentEntries(1)
    assert not hasNotes(1)
    assert tagWeight(1, 1) ?? 0 == 0
    assert tagWeight(1, 2) ?? 0 == 3
}
```

The outer loop visits note `1`, whose inner loop deletes its tag, then deletes
the note. Note `2` is absent, so the outer loop does not visit it and its tag
survives. The last statement deletes the book's own payload. `hasNotes`
then returns false even though the tag under note `2` remains.

The empty `on more` blocks also leave entries beyond either limit untouched;
this function does not report completion. A removal workflow must account for
those limits and for descendants whose ancestors are absent. Walking present
parents cannot discover those children's ancestor keys. The current language
has no whole-subtree delete or traversal that enumerates absent ancestors
([traversal](traversal-and-indexes.md#bounded-durable-traversal)).

## Access demand

Every export has a demand: the durable places it reads and writes, through every
function it calls. `marrow check --demand .` prints it for the first example:

```text
docs.durable.shelf.put reads ^books; writes ^books
docs.durable.shelf.title reads ^books.title
```

A whole-entry write is listed as a read and a write. `writes ^books` names
creation, replacement, and erase of an entry; `writes ^books.title` names an
update of a present entry. Demand describes the access a program requires; it
grants nothing ([`marrow check`](../tools/cli.md)).

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
