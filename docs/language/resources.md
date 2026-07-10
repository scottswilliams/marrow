# Resources

A resource declaration defines a typed hierarchical value. The same declaration
is used for local resource values and for entries attached to a durable store.

## Fields

```mw
module docs::resources

resource Book
    required title: string
    required author: string
    subtitle: string

    details
        pages: int
        language: string

    tags(pos: int): string

    notes(noteId: string)
        required text: string
        createdAt: instant

fn draft(title: string, author: string): Book
    return Book(title: title, author: author)

pub fn display(): string
    const book = draft("Small Gods", "Terry Pratchett")
    return book.subtitle ?? book.title
```

A scalar member has `name: Type` form. Fields are sparse by default: the member
may be absent. `required` makes the containing resource invalid while that
member is absent.

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
sequence, and entry-identity types are not accepted as resource key columns.
Key parameter names share the member namespace at their layer.

A one-column `int` leaf such as `tags(pos: int): string` has sequence behavior:
positive integer positions, 1-based append, and holes. Other key shapes are
ordinary ordered keyed layers.

## Local Resource Values

A constructor supplies named members:

```text
Book(
    title: "Small Gods",
    author: "Terry Pratchett",
    subtitle: "A Discworld novel",
)
```

All required fields must be present before the value is used at a boundary that
requires a valid complete resource. A local `var book: Book` may instead be
built incrementally by member assignment and then returned, passed, or written.

Sparse local member reads have type `T?`. Required members are bare once the
containing local resource is valid and present.

Resource values are copied by value. Assigning one local resource to another
does not create a reference to the first binding.

## Materialization Boundary

Unkeyed fields and groups are part of a materialized resource value. Keyed
children are addressed collections and are not included when a resource is read
into a local binding, passed to a function, returned, or constructed.

This distinction applies to durable entries as well. Reading `^books(id)` can
materialize its fields and unkeyed groups as `Book`; it cannot package
`^books(id).tags` or `^books(id).notes` into that value. Traverse keyed children
at their paths.

Whole assignment still replaces the complete durable entry, including keyed
children. Consequently, assigning a materialized resource back to the same
entry deletes keyed children that are not represented in the value. See
[Durable places](durable-places.md#whole-resource-assignment).

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
