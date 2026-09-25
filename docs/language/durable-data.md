# Durable data

A durable address is a location whose value outlives the program. It is written
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

A store root is visible from every module of the project that declares it by its
name, and is not addressable from a project that depends on that one; `pub`
applies to functions only ([visibility](modules-and-functions.md#visibility),
[dependencies](modules-and-functions.md#dependencies)).

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
holds no list, map, resource, reference, or function; many values under one entry
go in a [keyed branch](#keyed-branches). A stored value nests at most 32 levels
([limits](execution-limits.md#limits)).

## Reading

A direct durable read yields `T?`, because the entry or field may be absent
([optionals](types-and-values.md#optionals)). A required field or required group
leaf read through a [checked entry reference](#entry-references) has its declared type;
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

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

fn present(id: int): bool {
    return exists(^books[id])
}

fn subtitle(id: int): string? {
    return ^books[id].subtitle
}

test "an absent entry reads absent" {
    add(1, "Small Gods")
    assert present(1)
    assert not present(2)
    assert subtitle(1) ?? "none" == "none"
    assert titleOrNone(2) == "none"
}
```

`^books[1].subtitle` is absent: the entry is present and the field is not.
`titleOrNone` binds the whole entry with `if const`. Inside the block
`book.title` is a plain `string`, because a present entry has every required
field. `exists` answers presence with a `bool`. A checked entry reference
establishes presence for reads and writes through its name; a guard over an
inline path does not change the types of later reads.

`exists(^books)` is true when `^books` has a present immediate entry, including
an entry whose fields are all sparse and unset. `exists(^books[id].notes)` asks
the same of that branch, whether or not the book entry is present. Each family
test uses one bounded scan, and a stored entry whose presence record is missing
faults ([traversal](traversal-and-indexes.md#bounded-durable-traversal)).

The test calls `add` to commit its setup, then calls readers to observe it. A
test body owns no transaction of its own ([tests](tests.md#durable-tests)).

## Writing

A write sits inside a `transaction` block owned by the exported function. The
block's writes commit together when it ends, and a `return` inside the block
commits them ([transactions](errors-and-transactions.md#transactions)).

An entry is written whole, and one field of a present entry is written through
a checked entry reference:

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
        ref m = ^books[id] else {
            return absent
        }
        m.title = title
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
complete. `retitle` binds `ref m = ^books[id] else { return absent }` and
writes one field through `m`. A field write updates a present entry and never
creates one: `retitle(2, "Pyramids")` writes nothing, and `present(2)` stays
false. The required read `m.title` has type `string`, observes the staged
write, and lifts to the function's optional return type
([entry references](#entry-references)).

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

pub fn addSignedBook() {
    transaction {
        ^books[1] = Book(title: "Small Gods", subtitle: "A novel")
        ^books[1].notes[1] = Book.notes(text: "signed")
    }
}

pub fn erase(id: int) {
    transaction {
        delete ^books[id]
    }
}

fn subtitle(id: int): string? {
    return ^books[id].subtitle
}

fn noteText(id: int, pos: int): string? {
    return ^books[id].notes[pos].text
}

fn present(id: int): bool {
    return exists(^books[id])
}

test "replacement and delete leave the branch in place" {
    addSignedBook()
    replace(1, "Pyramids")
    assert subtitle(1) ?? "none" == "none"
    assert noteText(1, 1) ?? "" == "signed"
    erase(1)
    assert not present(1)
    assert noteText(1, 1) ?? "" == "signed"
}
```

`Book(title: "Pyramids")` names no `subtitle`, so the replacement drops it.
`Book.notes(text: "signed")` builds one entry of the `notes` branch. The note
under `notes[1]` stays: a keyed branch is its own node, and a whole-entry
assignment touches only the entry's own fields. A constructor that omits a
required field is a `check.type` error. The last three lines belong to
[deleting](#deleting).

## Entry references

`ref` captures an entry address and checks its presence. Its `else` block
handles absence and must leave the current path:

```mw
module docs::durable::references

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn setSubtitle(id: int, subtitle: string): bool {
    transaction {
        ref book = ^books[id] else {
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
    return ^books[id].subtitle
}

pub fn titleOf(id: int): Result<string, string> {
    ref book = ^books[id] else {
        return err("missing book")
    }
    return ok(book.title)
}

test "a reference writes one field" {
    put(1, "Small Gods")
    assert setSubtitle(1, "A novel")
    assert not setSubtitle(2, "A novel")
    assert subtitleOf(1) ?? "" == "A novel"
    const title = try titleOf(1)
    assert title == "Small Gods"
    const missing: Result<string, string> = err("missing book")
    assert titleOf(2) == missing
}
```

The right-hand side is a whole root or branch entry address, such as
`^books[id]` or `^books[id].notes[pos]`. All keys are evaluated once, from
left to right, before the presence check. Every later use of the name
addresses that captured entry. The binding reads presence, without copying
the entry's fields. It establishes no fact about ancestor entries: a present
note can be bound beneath an absent book.

The `else` block is mandatory. Every path through it must return, break,
continue, or reach `unreachable`. The new name is in scope after the binding,
not inside its own `else` block. A field or group address, or another
reference, is not a valid right-hand side.

Field, whole-group, and group-leaf writes require a checked entry reference
and a transaction. Inline field writes such as `^books[id].subtitle = subtitle`
are `check.requires_presence` errors. Whole-entry assignment to a direct path
creates or replaces an entry; it needs no reference. Optional reads use direct
paths as in `subtitleOf`. To capture a key for several direct operations,
bind that key with `const`.

Required fields and required group leaves read through a live reference have
their declared types. Sparse fields remain optional. Whole-entry and
whole-group value reads remain optional; `const copy = book else { … }`
copies a present entry value. Copied values are detached from durable state.
Read a struct field into a local value before projecting its members.

An entry reference is a local address binding, not a value that can be passed,
returned, or stored. Whole-entry assignment through its name replaces the
payload at the captured address; it does not retarget the reference. Branches
beneath the entry can be addressed or traversed through the name.

Presence lasts until the block ends or an entry in the same family is erased,
directly or through a call. A family is one root and one branch path, so an
erase through another key or reference in that family also ends the proof.
Parent and child families are independent. Complete replacement, field and
group updates, and deletion of sparse leaves preserve entry presence. Presence
does not establish a stable field value or the presence of a sparse field.

A required read or field write after an invalidating erase is
`check.requires_presence`, even if an optional value would fit. Check presence
again with a new `ref` binding or an explicit guard on the existing reference.
A completed whole-entry assignment through the reference also establishes a
new proof. A write evaluates its right-hand side before consuming its proof;
a helper that erases the family invalidates it even if the helper recreates
the entry or returns an ordinary error value. Such a return does not roll back
the transaction.

A protected read or write inside a loop entered after its proof was
established also requires that proof to survive the repeating region,
including nested bodies and a `while` condition. An erase in that region can
precede the use on the next iteration. Bind a reference inside the body when
each iteration needs its own presence check. Traversal freezes keys, and
neither traversal nor an index result proves entry presence. After an erasing
loop, an earlier proof cannot be reused.

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

struct DetailChanges {
    languageAfterPageWrite: string
    pagesAfterReplacement: int
    titleAfterReplacement: string
    languageAfterDelete: string
}

pub fn changeDetails(): DetailChanges {
    transaction {
        ^books[1] = Book(title: "Small Gods", details: Book.details(pages: 381, language: "en"))
        ref b = ^books[1] else {
            unreachable("the entry was just created")
        }
        b.details.pages = 400
        const language = b.details.language ?? ""
        b.details = Book.details(language: "de")
        const remainingPages = b.details.pages ?? 0
        const title = b.title
        delete b.details.language
        return DetailChanges(
            languageAfterPageWrite: language,
            pagesAfterReplacement: remainingPages,
            titleAfterReplacement: title,
            languageAfterDelete: b.details.language ?? "none",
        )
    }
}

test "a group is one value of the entry" {
    const changes = changeDetails()
    assert changes.languageAfterPageWrite == "en"
    assert changes.pagesAfterReplacement == 0
    assert changes.titleAfterReplacement == "Small Gods"
    assert changes.languageAfterDelete == "none"
}
```

`details` is part of the entry: it is present exactly when the entry is, and it
is addressed by the entry's key. `^books[id].details.pages` reads one leaf and
yields `int?`. `changeDetails` creates the entry and binds a checked reference
`b`, so the group writes that follow need no further guard. The function writes one leaf and keeps
`language`, then assigns the whole group exactly, so the omitted `pages` is
dropped. `title` is untouched either way, and `delete b.details.language`
clears one sparse leaf. The returned value captures each intermediate observation
for the test's assertions. A group whose leaves are all sparse is cleared with
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

pub fn addTag() {
    transaction {
        ^books[1].notes[1].tags["gift"] = Book.notes.tags(weight: 2)
    }
}

pub fn addNote() {
    transaction {
        ^books[1].notes[1] = Book.notes(text: "signed")
    }
}

fn tagWeight(): int? {
    return ^books[1].notes[1].tags["gift"].weight
}

fn notePresent(): bool {
    return exists(^books[1].notes[1])
}

fn bookPresent(): bool {
    return exists(^books[1])
}

test "a branch entry is its own node" {
    addTag()
    assert tagWeight() ?? 0 == 2
    assert not notePresent()
    assert not bookPresent()
    addNote()
    assert notePresent()
    assert tagWeight() ?? 0 == 2
}
```

`^books[id].notes[pos]` extends the entry's key with the branch key, and
`.tags[tag]` extends it again, one level down per branch. A branch holds scalar
fields and further branches, nested at most 16 levels. Every operation on an
entry applies to a branch entry at its own address.

`addTag` writes a tag under a note and a book that do not exist. The write is
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
through an entry reference, `delete book.subtitle`. Clearing a field that is already
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
        for pos in ^books[id].notes at most 1000 {
            for tag in ^books[id].notes[pos].tags at most 1000 {
                delete ^books[id].notes[pos].tags[tag]
            } on more {}
            delete ^books[id].notes[pos]
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

Every export has a demand: the durable paths it reads and writes, through every
function it calls. `marrow check .` prints it for the first example:

```text
2 exports across 1 module

docs.durable.shelf: 2 exports
  put
    reads ^books
    writes ^books
  title
    reads ^books.title
```

A whole-entry write is listed as a read and a write. `writes ^books` names
creation, replacement, and erase of an entry; `writes ^books.title` names an
update of a present entry. Demand describes the access a program requires; it
grants nothing ([`marrow check`](../tools/cli.md)).

## Durable identity

Every durable declaration, from the root down to each field, has an identity: a
128-bit id minted once and recorded in `.marrow/ids`. The first storeless
`marrow run <export>` or `marrow test` mints the ids a project lacks, and the
file is committed with the source so every checkout reuses them. `marrow check`
writes nothing; on an unminted project it reports one `check.durable_identity`
diagnostic per store root, naming every missing declaration:

```text
src/docs/durable/shelf.mw:8:7: check.durable_identity: 6 durable identities of this store root are missing from .marrow/ids (application `.`, root `books`, product `Book`, key `books.id`, field `Book.title`, field `Book.subtitle`); `marrow run` or `marrow test` mints them (commit the updated .marrow/ids)
```

Identity follows the id. A declaration keeps its identity through reordering and
respelling of the source around it. A renamed declaration gets a fresh id, and
the old entry stays in the ledger. The ids and the shape of the whole graph form
the program's durable contract, which a store on disk compares with its own
before it runs the program
([identity ledger](../tools/projects.md#identity-ledger),
[changing the program](../operations/README.md#changing-the-program)).

A stored `struct` value, and the payload of a stored `enum` member (including
`Option` and `Result`), is held in its field's single cell, and its fields are
read by position. The fields of a stored struct or payload have no ids of their
own: their declared names and order are part of the durable contract. Reordering,
renaming, adding, removing, or retyping such a field is a representation change,
because existing cells would otherwise be read with a different meaning. A store
on disk refuses the edited program with `store.contract_changed`, and
[`marrow apply`](../tools/cli.md#marrow-apply) refuses it with
`store.apply_unsupported` and reason `stored_value`. No stored value is
converted.

```mw
module docs::durable::positional

struct Pos {
    x: int
    y: int
}

resource Marker {
    required at: Pos
}

store ^markers[id: int]: Marker

pub fn place(id: int, x: int, y: int) {
    transaction {
        ^markers[id] = Marker(at: Pos(x: x, y: y))
    }
}

pub fn xOf(id: int): int {
    if const marker = ^markers[id] {
        return marker.at.x
    }
    return -1
}

test "a stored struct reads back by field name" {
    place(1, 7, 2)
    assert xOf(1) == 7
}
```

Declaring `Pos` as `y: int` followed by `x: int` leaves every use in the source
valid, but a cell written as `Pos(x: 7, y: 2)` would then read `x` as 2. After
`place(1, 7, 2)` has written a store, `marrow run docs.durable.positional.xOf
--store <store> -- 1` with the reordered `Pos` refuses before it reads an entry
and prints this line on standard error:

```text
store.contract_changed: the supplied image differs in the durable contract from the binding required by this operation; the current binding was not changed
```

A resource's own fields, its group fields and its branch fields are stored under
their ids. Their order is part of the contract as well, so an attached store
refuses a reorder, but `marrow apply` accepts one: each existing value stays
under its id.

Today, keyed roots, their groups, and their branches read and write end to end.
A singleton root such as `store ^settings: Settings` and a group inside another
group or a branch are future work
([status](../status.md#not-yet-available)).
Binding a resource containing a nominal value to a store reports
`check.unsupported`, including nested and sparse fields and unused bindings
([nominal ints](types-and-values.md#aliases-and-nominal-ints)).
