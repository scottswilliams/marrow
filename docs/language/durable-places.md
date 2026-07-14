# Durable Places

A durable place begins with `^` and a declared store root. The prefix marks
saved lifetime in the language. It does not expose a storage key or backend
representation.

## Store Declarations

A keyed store attaches a resource type to typed identity columns:

```mw
module docs::durable

resource Book
    required title: string
    required author: string
    subtitle: string
    aliases(kind: string): string
    tags(pos: int): string

store ^books(id: int): Book

pub fn create(title: string, author: string): Id(^books)
    const id = nextId(^books)
    transaction
        ^books(id).title = title
        ^books(id).author = author
    return id

pub fn title(id: Id(^books)): string?
    return ^books(id).title
```

A singleton root omits keys:

```text
resource Settings
    required locale: string

store ^settings: Settings
```

Store roots are project-wide in the current language. Modules use the declared
root shape directly; function visibility does not change root access.

## Entry Identities

`Id(^books)` is the entry identity type for `^books`. It is distinct from every
other store identity type, including roots with the same raw key types.

`Id(^books, 42)` constructs an identity from explicit raw key values. For a
composite root, all key components are supplied to one constructor:

```text
Id(^enrollments, "student-1", "course-9")
```

Construction validates the declared arity and types. It does not read the
store, create an entry, or prove presence. A durable root call accepts either
one matching identity value or its explicit raw key components at this
boundary.

## `nextId`

`nextId(^root)` is defined only for a root with one `int` identity column. It
returns one greater than the greatest currently stored key. An empty root uses
`0` as the starting maximum and therefore returns `1`.

The call does not reserve or create the result. Two calls made before either
candidate is written may return the same identity. Code that needs allocation
and creation together performs them in one transaction and still handles
conflicts imposed by its execution environment.

## Presence And Reads

`exists(path)` reports whether the exact node is present. Reading a place that
may not exist yields `T?`. The resource and each ancestor must be known present
before a required descendant is a bare `T`.

```mw
module docs::durable_presence

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn show(id: Id(^books))
    if const book = ^books(id)
        print(book.title)

    if const subtitle = ^books(id).subtitle
        print(subtitle)
```

A whole resource binding makes its required fields bare. An `exists` guard
narrows the exact place it tests; testing only a resource ancestor does not
rewrite later descendant expressions. A whole resource read materializes scalar
fields and unkeyed groups. Keyed children remain separately addressed layers.
Reading a keyed or positional leaf at one key is optional because that position
may be absent.

Store roots and keyed child layers have ordered typed keys. Iteration and
neighbor operations use that order and visit only present entries; see
[Traversal and indexes](traversal-and-indexes.md).

## Field Assignment

Assigning one field changes that field and preserves sibling fields, groups, and
keyed children:

```text
^books(id).subtitle = "A Discworld novel"
```

A sparse field and a non-positional keyed leaf are **present-or-clear** places.
They accept `T`, `T?`, or `absent`. A present value is stored; an absent value
deletes that leaf. Required fields and positional sequence elements do not
accept optional or absent assignment.

```mw
module docs::durable_clear

resource Book
    required title: string
    subtitle: string
    aliases(kind: string): string

store ^books(id: int): Book

pub fn copySparse(from: Id(^books), to: Id(^books))
    if exists(^books(to))
        const subtitle: string? = ^books(from).subtitle
        ^books(to).subtitle = subtitle

        const shortName: string? = ^books(from).aliases("short")
        ^books(to).aliases("short") = shortName
```

The same clearing behavior applies when an optional local contains `absent`;
the syntax need not use the literal directly.

## Required-Member Validation

Creating a resource entry must establish every required member. Outside a
transaction, validation occurs at the end of the individual durable write.
Field-by-field creation of a resource with several required fields therefore
uses a transaction. Inside nested transactions, validation is deferred until
the outer transaction commits.

Required descendants of unkeyed groups are checked as requirements of the
containing entry. Required fields of a keyed child resource are checked when
that keyed entry is created or replaced.

## Whole Resource Assignment

Assignment to the resource address is exact replacement:

```mw
module docs::durable_replace

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn replace(id: Id(^books), title: string)
    const replacement = Book(title: title)
    ^books(id) = replacement
```

The assignment removes omitted sparse fields and unkeyed groups. It also
removes every keyed child below the replaced entry, because keyed children are
not members of the materialized `Book` value. Index entries derived from the
old data are updated as part of the same managed write.

Whole assignment to a keyed resource child likewise replaces that complete
child entry and removes its omitted descendants. Use field assignment when
sibling data must remain.

## Deletion

`delete place` removes the addressed subtree:

```text
delete ^books(id).subtitle
delete ^books(id).aliases("short")
delete ^books(id)
```

Deleting an absent sparse place is a no-op. Deleting a required field or a
group containing required descendants is rejected. Deleting a complete store
entry removes its fields, children, and maintained index entries.

A transaction stages deletion with other durable writes and either commits or
rolls them back together.

Source declarations such as `rename` and `retire` are defined in
[Evolution](evolution.md). The evolution command workflow returns with its
refounding lane.
