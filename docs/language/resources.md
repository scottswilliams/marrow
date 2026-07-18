# Resources

A resource declaration defines a typed hierarchical value. The same declaration
is used for local resource values and for entries attached to a durable store.

## Fields

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
```

A scalar member has `name: Type` form. Fields are sparse by default: the member
may be absent. `required` makes the containing resource invalid while that
member is absent.

Because a sparse field already models absence — an unset field reads `absent` —
declare a field `Option<T>` only when a stored `none` must be distinguishable from
the field being unset. That three-state field (`absent`, present `none`, present
`some`) is read by proving presence and then matching the stored `Option`; see
[Option and Result](types-and-values.md#option-and-result).

The `details` block is an unkeyed group. Its members extend the containing
resource hierarchy. A required descendant of an unkeyed group is a required
member of the containing resource. The read of that descendant is nevertheless
optional while any ancestor resource path may be absent.

## Keyed Layers

A member with parameters is a keyed child layer:

```text
tags(pos: int): string

notes(noteId: string)
    required text: string
    createdAt: instant
```

`tags` is a keyed scalar leaf. `notes` is a keyed resource-entry layer. A keyed
entry is subject to its own required-member validation when that entry exists.
No entry is required merely because the layer is declared.

Several key columns are allowed:

```text
cells(row: int, column: int): decimal
```

Keys must be supported ordered scalar types. Optional, decimal, resource,
collection, and entry-identity types are not accepted as resource key columns.
Key parameter names share the member namespace at their layer.

A one-column `int` leaf such as `tags(pos: int): string` has positional
behavior: positive integer positions, 1-based append, and holes. Other key
shapes are ordinary ordered keyed layers.

## Groups And Branches

An unkeyed block (`details` above) is a **group**: a static field-path namespace
inside a resource. A block with key parameters (`notes(noteId: string)` above) is
a **branch**: a keyed subtree, a distinct durable graph node with its own key
tuple, nested under its containing resource. A group or branch may itself hold
fields, groups, and branches.

When a resource backs a store, its group and branch declarations are part of the
durable graph. Each group has its own durable identity (a `group` identity), and
each branch has its own placement identity, one identity per key column, and one
per stored field — anchored at a group- or branch-qualified path
(`Book.details.pages`, `Book.notes.noteId`, `Book.notes.text`). Groups and
branches contribute to the [durable-contract
identity](durable-places.md#durable-identity) exactly as roots do.

A `branch` keyed by one or more columns and holding only scalar fields is part of the
executable durable graph, and such branches may nest to any depth: their whole entries
are created, read, replaced, and erased through the key-path address
`^root[key…].branch[bkey…]…` (see [Durable places](durable-places.md#keyed-branches)).
A top-level field may also hold a widened value — a dense `struct`/record, a closed
`enum`, or an `Option`/`Result` — stored inline in its field cell and read whole or by
field; `branch` fields stay scalar-only.
A root-level `group` of scalar or widened leaves is part of the executable durable graph:
its leaves join the containing entry's materialized value, and the group is read,
replaced, and erased whole through `^root[key…].group`, with each leaf read and rewritten
through `^root[key…].group.leaf` (see
[Durable places](durable-places.md#groups)).
Other durable shapes are not yet executable: a resource declaring a nominal-typed field,
or a group nested in a branch or in another group, declares and verifies its complete
durable identity, but an operation over it is the typed `check.unsupported` rejection
rather than a silent drop, until its lane lands. A keyed scalar leaf such as
`tags(pos: int): string` is likewise not yet part of the executable durable graph.

## Local Resource Values

A resource type names an ordinary by-value value. A constructor supplies named
members; a `const` or `var` annotation may name the resource type; and a resource
value is passed to a function parameter and returned from a function, sharing the
record representation.

```mw
module docs::resources::values

resource Book {
    required title: string
    required author: string
    subtitle: string
}

pub fn small(): Book {
    return Book(
        title: "Small Gods",
        author: "Terry Pratchett",
        subtitle: "A Discworld novel",
    )
}

pub fn drafted(title: string, author: string): Book {
    var book: Book = Book(title: title, author: author)
    book.subtitle = "draft"
    return book
}

pub fn describe(book: Book): string {
    return book.subtitle ?? book.title
}

pub fn independentCopy(): string {
    var a = Book(title: "a", author: "x")
    var b = a
    b.subtitle = "changed"
    return a.subtitle ?? "a-untouched"
}
```

All required fields must be present before the value is used at a boundary that
requires a valid complete resource. A local `var book: Book` may instead be
built incrementally by member assignment and then returned, passed, or written.

Sparse local member reads have type `T?`. Required members are bare once the
containing local resource is valid and present.

Resource values are copied by value. Passing a resource to a parameter and
returning one both copy: a callee that mutates its own binding leaves the
caller's value unchanged, and assigning one local resource to another does not
create a reference to the first binding.

An optional resource value (`Book?`) is not yet composed: a resource type is
outside the value-argument family the `Option` template accepts, so an
optional-resource binding, parameter, or return is a `check.unsupported`
rejection until its lane lands.

## Group Values

An unkeyed group is part of the materialized resource value as a nested value.
Its leaves are read and assigned through the group name, and the whole group is a
value unit read, assigned, and copied whole. The qualified constructor
`Resource.group(field: value, …)` builds a group value, symmetric with the branch
entry constructor, and it is supplied to the resource constructor as a named
argument.

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

pub fn pagesAfterSet(): int {
    var book = Book(title: "Small Gods", author: "Terry Pratchett")
    book.details.pages = 384
    return book.details.pages ?? 0
}

pub fn constructedPages(): int {
    const book = Book(
        title: "Small Gods",
        author: "Terry Pratchett",
        details: Book.details(pages: 384, language: "en"),
    )
    return book.details.pages ?? 0
}

pub fn copiedGroup(): int {
    var a = Book(title: "a", author: "x")
    a.details.pages = 7
    var b = Book(title: "b", author: "y")
    b.details = a.details
    return b.details.pages ?? 0
}
```

A group leaf follows the same presence rule as a top-level member: a sparse leaf
reads `T?` and a `required` leaf is bare once the containing resource is valid and
present. Assigning a leaf sets it present; `unset book.details.pages` clears a
sparse leaf back to absent. A group leaf assignment reads its containing group,
updates the leaf, and writes the group back, so the group is not aliased. A group
value is copied by value: assigning one group into another carries its leaves
without aliasing the source.

An omitted group whose leaves are all sparse defaults to present with vacant
leaves. A group with a `required` leaf must be supplied, because a required
descendant of an unkeyed group is a required member of the containing resource;
omitting it is the same completeness rejection as an omitted required field.

A group value has no type annotation of its own: it is the round-trip unit produced
and consumed at construction, assignment, and member access, not a named type a
binding or parameter may declare. A group nested inside another group is not yet
part of the materialized value.

## Materialization Boundary

Unkeyed fields and groups are part of a materialized resource value. Keyed
children are addressed collections and are not included when a resource is read
into a local binding, passed to a function, returned, or constructed.

This distinction applies to durable entries as well. Reading `^books[id]`
materializes its top-level fields and its root-level groups as `Book`; it can never
package `^books[id].tags` or `^books[id].notes` into that value. Traverse keyed children
at their paths.

A keyed family is navigated, never materialized as a whole: the language spells no
construct that reads, replaces, merges, or clears a whole `branch` family in one
operation. Change a family by updating each entry at its own key-path in place, or,
when a local working copy is needed, by a bounded traversal that copies the entries
and an explicit per-key write-back that reconciles each one. There is no
family-level merge or subtree-replace.

Whole assignment replaces the entry's own payload — its marker and stored fields —
and leaves its keyed children in place. Assigning a materialized resource back to the
same entry therefore rewrites the fields exactly (dropping any omitted sparse field)
without disturbing the entry's keyed `branch` descendants. See
[Durable places](durable-places.md#whole-resource-assignment).

This exact-replacement rule is a footgun for the read-modify-write habit:
assigning a *partially* constructed value — one built from only a few of the
entry's fields — erases every sparse field the constructed value omits, not only
the fields being changed. This applies to group leaves as well: a whole value that
supplies a group with fewer leaves, or defaults an omitted all-sparse group to its
vacant form, replaces the group and drops every leaf the assigned value omits. To
change a subset of members without disturbing the rest, assign each at its own path
(`book.subtitle = …`, `book.details.pages = …`) rather than whole-assigning a
partial value. The round trip is safe only when the value
written back carries every field that must survive, which a whole-entry read
into a local guarantees. There is no checker lint: a partial constructor in
whole-assignment position is indistinguishable from a deliberate replacement.

## Resource Names

Resources belong to modules and may be named through the module path where the
project type environment is available. They do not use `pub`. A resource name
may not collide with another declaration or built-in in the same source
namespace.

Documentation comments beginning with `;;` may precede resource declarations
and members. They do not affect type, path, presence, or runtime value.

## Project Requirement

A project containing a resource declaration currently requires a native store
configuration during project checking, even if a particular resource is used
only as a local value. This is a current project-checking requirement rather
than a semantic difference between local and durable resource values.
