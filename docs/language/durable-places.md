# Durable Places

A durable place begins with `^` and a declared store root. The prefix marks
saved lifetime in the language. It does not expose a storage key or backend
representation. Every durable read and write named below reaches the store
through the compiler-checked path kernel; application code never receives a raw
key, an engine handle, or a transaction object.

## Store Declarations

A keyed store attaches a resource type to one typed identity column:

```mw
module docs::durable

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn put(id: int, title: string)
    transaction
        ^books(id) = Book(title: title)

pub fn title(id: int): string?
    return ^books(id).title
```

The identity column is a single key drawn from the closed orderable durable-key
scalar set: `int`, `string`, `bool`, `bytes`, `date`, or `instant` (a nominal
type over one of these is admitted through its base scalar). `duration` is a
span rather than an identity and is not a durable key. A resource declares
scalar fields that are `required` or sparse; a sparse field may be absent. The
store root is project-wide: any module uses the declared root shape directly,
and function visibility does not change root access.

## Durable Identity

A program's durable graph — its roots, each root's key scalar, and each root
record's ordered stored field profile (name, scalar type, and `required` flag) —
has a stable 32-byte **durable-contract identity**. The compiler derives it from
the resolved graph and records it in the program image; the independent verifier
rebuilds the descriptor from the image tables, recomputes the identity, and
rejects any image whose recorded identity does not match its graph. The identity
changes on every semantic change to the graph (a renamed root or field, a
changed key type, a field made required, a field added or removed) and is stable
across source spelling and declaration order that leave the graph unchanged.
Operation sites — the individual read and write points over the graph — are not
part of the identity, so adding or removing one leaves it stable.

The identity is scoped to the local project. Its canonical form reserves a
leading package-lineage byte, so a durable graph contributed by a dependency
package later carries a distinct lineage without changing the identity of a
local graph.

Durable writes are grouped by an explicit transaction owned by the exporting
function; see [Errors and transactions](errors-and-transactions.md).

## Presence And Reads

`exists(path)` reports whether the addressed entry is present. Reading a place
that may not exist yields `T?`, because the entry at that key may be absent:

```mw
module docs::durable_presence

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn present(id: int): bool
    return exists(^books(id))

pub fn subtitle(id: int): string?
    return ^books(id).subtitle

pub fn titleOrNone(id: int): string
    if const book = ^books(id)
        return book.title
    return "none"
```

Binding a whole resource with `if const` proves the entry present, so its
required fields read as bare `T` through that binding. Reading a single field of
a keyed entry directly is optional, because that key may be absent.

Store roots have ordered typed keys. Iteration visits only present entries in
key order; see [Traversal and indexes](traversal-and-indexes.md).

## Field Assignment

Assigning one field changes that field and preserves the entry's other fields:

```mw
module docs::durable_field

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn setSubtitle(id: int, subtitle: string)
    transaction
        ^books(id).subtitle = subtitle

pub fn clearSubtitle(id: int)
    transaction
        const cleared: string? = absent
        ^books(id).subtitle = cleared
```

A sparse field is a **present-or-clear** place: it accepts a value, an optional
value, or `absent`. A present value is stored; an absent value clears the field.
A required field does not accept an optional or absent assignment.

## Whole Resource Assignment

Assignment to the entry address is exact replacement:

```mw
module docs::durable_replace

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn replace(id: int, title: string)
    transaction
        ^books(id) = Book(title: title)
```

The assignment stores every field named by the constructed value and removes any
omitted sparse field. A required field left unset when the entry commits rolls
the transaction back with `run.required_missing` rather than storing a partial
entry.

## Deletion

`delete place` removes the addressed place:

```mw
module docs::durable_delete

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn removeSubtitle(id: int)
    transaction
        delete ^books(id).subtitle

pub fn remove(id: int)
    transaction
        delete ^books(id)
```

Deleting a sparse field that is already absent is a no-op. Deleting a required
field is rejected. Deleting a whole entry removes the entry and its fields.
