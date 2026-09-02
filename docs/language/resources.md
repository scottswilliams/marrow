# Resources

A resource declares the shape of a hierarchical value: its fields, its groups,
and its keyed branches. One declaration serves both a local value and the
entries of a durable store.

## Fields

A field is written `name: Type`. A field is sparse unless it is marked
`required`:

```mw
module docs::resources

resource Book {
    required title: string
    required author: string
    subtitle: string

    details {
        pages: int
        language: string
    }

    notes[noteId: string] {
        required text: string
        createdAt: instant
    }
}

store ^books[id: int]: Book

pub fn add(id: int, title: string, author: string) {
    transaction {
        ^books[id].title = title
        ^books[id].author = author
    }
}

pub fn describe(id: int): string {
    if const book = ^books[id] {
        return book.subtitle ?? book.title
    }
    return "(absent)"
}

test "describe falls back to the title" {
    add(1, "Small Gods", "Terry Pratchett")
    assert describe(1) == "Small Gods"
    assert describe(2) == "(absent)"
}
```

`title` and `author` are required; `subtitle` is sparse. `add` writes the two
required fields and no subtitle. `describe` reads the whole entry with
`if const` and falls back with `??`, because a sparse read is a `string?`
([optionals](types-and-values.md#optionals)).

A required field is present in every valid value. A constructor names each
required field, and a missing one is a `check.type` error. What a required
field means at commit is in [Writing](durable-places.md#writing). A sparse field
may be absent, and reading one yields `T?`. A sparse field already models absence, so
declare a field `Option<T>` only when a stored `none` must differ from an unset
field ([Option and Result](types-and-values.md#option-and-result)).

The types a field holds are listed in
[what a field holds](durable-places.md#what-a-field-holds). A `///` comment may
precede the resource and each member; it carries no meaning to the compiler.

## Members

A block without a key list is a group. A block with a bracketed key list is a
branch. In the declaration above, `details` is a group: a named layer of the
resource that travels with the value. `notes` is a branch: a keyed family of
entries beneath each `Book`, addressed one entry at a time as
`^books[id].notes[noteId]`.

A required field inside a group is a required field of the resource, so a
constructor that omits `details` when it holds a required leaf is a `check.type`
error. A required field inside a branch applies to each entry of the branch, and
declaring the branch creates no entry. A branch may take several key
components, `loans[borrower: string, day: date]`, and is then addressed by the
whole tuple in order. A key is an `int`, `string`, `bool`, `bytes`, `date`, or
`instant` ([Keys](durable-places.md#keys)). Within one layer, key names and
field names share one namespace.

A branch holds scalar fields and may hold further branches
([Keyed branches](durable-places.md#keyed-branches)). Today, a group sits
directly under the resource, and its leaves are scalars when the resource backs
a store. A group inside a group or a branch, and a keyed scalar leaf such
as `tags[pos: int]: string`, are future work ([status](../status.md)).

## Local values

A resource is an ordinary value. Build one with its constructor, bind it with
`const` or `var`, pass it to a function, and return it:

```mw
module docs::resources::values

resource Book {
    required title: string
    required author: string
    subtitle: string
}

pub fn drafted(title: string, author: string): Book {
    var book: Book = Book(title: title, author: author)
    book.subtitle = "draft"
    return book
}

pub fn describe(book: Book): string {
    return book.subtitle ?? book.title
}

pub fn lookup(found: bool): Book? {
    if found {
        return drafted("Small Gods", "Terry Pratchett")
    }
    return absent
}

test "values copy" {
    var a = drafted("a", "x")
    var b = a
    unset b.subtitle
    assert describe(a) == "draft"
    assert describe(b) == "a"
    assert lookup(true)?.title ?? "(absent)" == "Small Gods"
    assert lookup(false)?.title ?? "(absent)" == "(absent)"
}
```

`drafted` builds a `Book` from its required fields, then sets a sparse field on
the `var`. `describe` takes a `Book` by value. `lookup` returns `Book?`, and the
test reads through it with `?.` and `??`. `var b = a` takes a copy, so
`unset b.subtitle` clears the copy and leaves `a` as it was.

A `var` starts from a constructor and sets or clears sparse fields afterward.
Every resource value is copied on assignment, on a call, and on return, so a
callee that changes its parameter leaves the caller's value unchanged.

A binding or a return type may be `Book?`, proven present with `if const`. A
parameter is a bare value, so `book: Book?` reports `check.unsupported`, like
any optional parameter. A resource is not accepted as a type argument today:
`Option<Book>` and `List<Book>` report `check.unsupported`.

A resource name is used bare from any module of the project and takes no
`pub`. It shares one namespace with struct, enum, and built-in names
(`check.name_conflict`).

## Group values

A group is part of the value it belongs to. Its leaves are read and assigned
through the group name, and the whole group is assigned and copied as one unit.
`Book.details(pages: 384)` builds a group value for the constructor:

```mw
module docs::resources::groups

resource Book {
    required title: string
    required author: string

    details {
        pages: int
        language: string
    }
}

pub fn constructedPages(): int {
    const book = Book(
        title: "Small Gods",
        author: "Terry Pratchett",
        details: Book.details(pages: 384, language: "en"),
    )
    return book.details.pages ?? 0
}

test "groups copy" {
    assert constructedPages() == 384
    var a = Book(title: "a", author: "x")
    a.details.pages = 7
    var b = Book(title: "b", author: "y")
    b.details = a.details
    unset a.details.pages
    assert b.details.pages ?? 0 == 7
}
```

`constructedPages` supplies the group in the constructor. The test sets one
leaf of a group the constructor left vacant, assigns the group into another
value, and clears the source leaf; `b` still holds `7`.

A group leaf follows the field rules. An omitted group whose leaves are all
sparse is present with every leaf absent; a group with a required leaf is
supplied in the constructor. A group has no type name of its own:
`Book.details(...)` builds one, and no binding or parameter is annotated with
it.

## What a value carries

A resource value carries its fields and its groups. Keyed children stay
addressed by key: a `Book` read into a binding, passed, returned, or built by a
constructor holds no `notes`, and `Book(title: "t", notes: ...)` is a
`check.type` error.

The same boundary holds for a durable entry. Reading `^books[id]` yields the
fields and groups as a `Book?`; its notes stay at `^books[id].notes[noteId]`,
reached one entry at a time or through a
[bounded traversal](traversal-and-indexes.md#bounded-durable-traversal). There
is no whole-family read, replace, or delete.

Whole assignment stores exactly the fields the value carries. Assigning a `Book`
to `^books[id]` rewrites the entry's fields, drops every sparse field and group
leaf the value omits, and leaves the `notes` entries in place. To change one
field, assign it at its own path ([Writing](durable-places.md#writing)).
