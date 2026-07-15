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

Each identity column is drawn from the closed orderable durable-key scalar set:
`int`, `string`, `bool`, `bytes`, `date`, or `instant` (a nominal type over one
of these is admitted through its base scalar). `duration` is a span rather than
an identity and is not a durable key. A resource declares fields that are
`required` or sparse; a sparse field may be absent. The store root is
project-wide: any module uses the declared root shape directly, and function
visibility does not change root access.

### Durable Field Values

A stored field holds a value from the closed acyclic durable value set:

| Field value | Notes |
|---|---|
| a scalar | `int`, `bool`, `string`, `bytes`, `date`, `instant`, `duration` |
| a nominal scalar | stored as its base scalar |
| a dense `struct` | its leaves stored inline; the struct's leaves are structure, not separately named durable declarations |
| a closed `enum` | a user `enum`, `Option`, or `Result`; each variant is a member |
| an `Option` of any of the above | `Option[T]` where `T` is itself an admitted value |

The value graph must be acyclic (a struct or enum may not contain itself,
directly or transitively). A collection is not stored directly in a field — a
large collection belongs under a keyed `branch` — and a field does not store a
nested sparse resource, a place, a function, or a handle.

A `store` root is either a *singleton* or a *keyed tuple*. A singleton root
omits the key list and holds a single entry:

```mw
module docs::durable_singleton

resource Settings
    required locale: string

store ^settings: Settings
```

A keyed tuple has one or more ordered key columns (up to eight); a composite key
identifies each entry by the whole tuple in column order:

```mw
module docs::durable_composite

resource Enrollment
    required grade: int

store ^enrollments(student: string, course: string): Enrollment
```

Each root — singleton, single-column, or composite — is a distinct durable graph
node with its own complete identity (its placement, its stored product, one
identity per key column, and one per stored field; see
[Durable Identity](#durable-identity)).

A stored resource may also declare static `group` namespaces and keyed `branch`
placements (see [Resources](resources.md#groups-and-branches)). These are part of
the durable graph and complete their identity like a root.

Durable declarations compile, verify, and complete their identity, but durable
execution is currently in the trough: the CLI no longer opens a store, so a
durable export does not run from `marrow run` (it reports the typed
`cli.durable_unsupported` outcome). The operations described below are the current
durable *language* — they are checked and their identity is complete — and their
execution returns as the ephemeral-memory preview and later the persistent
companion path; see [Project status](../status.md).

Within that checked language, the *flat scalar* single-column keyed root is the
form whose operations the compiler fully lowers: a root with one key column, no
groups or branches, and only plain scalar fields, whose entries are read and
written through the operations below. A singleton root, a composite-key root, a
root whose resource declares a group or a branch, or a root whose resource
declares a widened field (a nominal scalar, struct, enum, or `Option` value)
declares and verifies its full identity, but its read and write operations are
not yet lowered — an operation over one is the typed `check.unsupported` rejection
rather than a silent drop, until the wider durable runtime lands. (Declaring such
a store is no longer a `check.type` on the resource, as it was before durable
field values widened; the store is identity-complete, only its operations are
deferred.)

The compiler emits an **operation site** for every node of the whole durable graph
— a whole-payload site for each keyed placement (the store root and every nested
`branch`) and a field-leaf site for each stored field (top-level, group-scoped, or
branch-scoped) — and the verifier seals each one by resolving its concrete address
against the graph it independently reconstructs. A site on the flat executable root
seals as executable; every other site — over a nested placement, a group-scoped or
widened field, or a non-flat root — seals with a complete identity but parks, so its
concrete address is checked and recorded while its execution waits for the wider
kernel. The site table holds one entry per graph node regardless of how many
operations reference it, and appending a sparse field adds one field-leaf site
without disturbing any existing site.

## Durable Identity

Every durable declaration — the application, a store root, each of its key
columns, the stored resource, each stored field, each static `group` namespace,
each keyed `branch` placement together with its own key columns and fields, and
each closed `enum` reachable through a durable field (its sum identity and one
member identity per variant) — has its own durable
identity: an opaque 128-bit id minted once from OS entropy and recorded in the
project's committed identity ledger, `marrow.ids` (see
[Projects](../tools/projects.md#the-identity-ledger)). The ledger is machine
written and machine read; developers never edit, copy, or cite ids. In ordinary
development it is invisible: the first `marrow run` over a fresh durable
declaration mints its identities, and the file is committed with the source so
every later build, clone, and checkout reuses them. A durable declaration with
no ledger identity does not compile — the typed `check.durable_identity`
diagnostic names the missing identity.

In the ledger model a retired identity is recorded as a tombstone and is never
reused, so removing a declaration and re-adding its name yields a fresh
identity. The only mint that exists today, `marrow run`, is additive-only: it
mints a row for each anchor that lacks one and never tombstones. So under the
current mint a rename mints a fresh identity for the new name and leaves the old
row live and orphaned, and deleting a declaration then re-adding the same path
readopts the old identity. This is harmless in the current trough — no
persistent store is reachable, so no stored data is bound to a stale or readopted
identity. The accepted apply action (future) is what classifies a rename as an
anchor move, records a genuine removal as a tombstone, and surfaces an orphaned
row.

A program's whole durable graph additionally carries a stable 32-byte
**durable-contract identity**, computed over the graph's ledger ids and shape:
each root's ordered key tuple — scalar and identity per column — and its
resource's ordered **member tree**. The member tree is the resource's stored
fields (the `required` flag and the stored value shape per field) interleaved
with its static `group` namespaces (each an identity and its own member tree) and
its keyed `branch` placements (each an identity, an ordered key tuple, and its own
member tree). A field's value shape records its scalar, dense struct leaves, or
closed-enum members; a durable-reachable enum contributes its sum identity and
one member identity per variant, so appending an enum member (which preserves the
existing members' identities and order) is a distinct evolution from renaming or
re-typing one. A nested struct's leaves are structure, not separate durable
declarations, so they mint no identities of their own. Key-column and member
order are part of the identity. The compiler derives it from
the resolved
graph and records it in the program image; the independent verifier rebuilds
the descriptor from the image tables, recomputes the identity, and rejects any
image whose recorded identity does not match its graph. Because the descriptor
carries ledger ids rather than names, an anchor move preserves the durable
identity — the ledger anchor moves, the id stays — while every semantic change (a
changed key type, a field made required, a field added or removed, or a
delete-then-re-add onto a fresh id) changes it. These are ledger-model properties
the conformance tests pin. Spelling and declaration order that leave the graph
unchanged leave the identity unchanged. A rename becomes an anchor move only under
the accepted apply action (future); the additive-only `run` mint instead leaves a
renamed declaration's old row live, as described above. Operation sites — the
individual read and write points over the graph — are not part of the identity,
so adding or removing one leaves it stable.

The identity is scoped to the local project. Its canonical form reserves a
leading package-lineage byte, so a durable graph contributed by a dependency
package later carries a distinct lineage without changing the identity of a
local graph. A storeless project needs no ledger and no identity.

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

## Named Places

A `place` binding names one concrete durable entry address inside a function:

```mw
module docs::durable_place

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn retitle(id: int, title: string)
    transaction
        place book = ^books(id)
        book = Book(title: title)
        book.subtitle = "revised"

pub fn subtitleOf(id: int): string?
    place book = ^books(id)
    return book.subtitle
```

The right-hand side is a whole durable entry address `^root(key...)`. The key
tuple is evaluated **exactly once**, at the binding. Every later operation through
the name — a field read `book.subtitle`, a field assignment `book.subtitle = v`, a
whole-entry replacement `book = Book(...)`, an `exists(book)` test, a
`delete book`, or an `if const c = book` guard — resolves through that one
evaluated address without re-running the key operand. Two reads of `book` therefore
address the same entry even if the key expression contains a function call or a
checked computation, which runs once.

A place binding is a `const`: it is not re-assignable and does not shadow an
existing name. Writing `book = Book(...)` replaces the durable entry the place
addresses; it does not rebind the place to a different address. A place names a
durable designation rather than a fetched value, so its bare name is not itself a
value: read a field with `place.field`, bind the whole entry with `if const`, or
test presence with `exists`. Binding a place to a non-durable value, to a field
address (`^books(id).title`), or to another place is rejected.

A place binds an address the same way an inline `^root(key...)` operation does, so a
place over a store shape whose operations are not yet lowered (a singleton,
composite-key, group- or branch-bearing, or widened-field root) reports the same
not-yet-executable result as the inline form.

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
