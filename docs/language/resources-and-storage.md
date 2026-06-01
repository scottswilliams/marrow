# Resources And Saved Data

Resources are the center of Marrow `.mw`. A resource is a typed tree shape. The
same shape can be used for local values, local keyed trees, or saved data.

This page uses "saved data" for data marked with `^`. Saved data persists in
the project database. Local data has no `^` and exists only while code runs.

Ordinary application code declares resources for saved roots. Raw saved-tree
access is for import, export, data evolution, repair, and tools.

Saved path syntax is logical. Marrow decides how roots, keyed layers, fields,
and indexes are encoded for the selected backend. Code depends on the resource
shape, not on a backend key layout.

Encoded resource keys are distinct from structural names. A record key such as
`"byShelf"` does not collide with an index named `byShelf`.

Typed traversal keeps those segment kinds separate: `^books` yields `Book`
elements, `keys(^books)` yields resource identities, and
`^books.byShelf(...)` yields identities from that index branch.

## Resource Trees

```mw
resource Book
    required title: string
    required author: string
    shelf: string
```

Use the resource locally:

```mw
var draft: Book
draft.title = "Small Gods"
draft.author = "Terry Pratchett"
```

Attach the same shape to saved data:

```mw
resource Book at ^books(id: int)
    required title: string
    required author: string
    shelf: string
```

Then `^books(id)` is a saved `Book`, where `id` is a `Book::Id`.

Saved resources are not hidden blobs. The book above is stored as fields and
layers under `^books(id)`, so tools can inspect the same tree that code reads.

A resource identity can exist with no populated fields. Existence is an explicit
structural fact: `exists(^books(id))` is true once the node exists, independent of
any field value or child. There is no need to model a required field just to record
that a resource exists.

A saved root has one managed resource owner. If `Book` owns `^books`, another
resource cannot claim `^books` with a different shape. Use nested layers,
indexes, or a separate root instead.

A saved resource declaration may omit identity keys when the root itself is
the resource:

```mw
resource Settings at ^settings
    theme: string
    required maxLoans: int
```

## Tree Layers

Indentation in a resource declaration mirrors the saved/local tree:

```mw
resource Patient at ^patients(id: string)
    name
        first: string
        last: string

    visits(date: date)
        note: string
        provider: string
```

A plain nested block groups fields. A keyed nested block creates repeatable
children under that layer.

Unkeyed groups are structural. Required fields inside an unkeyed group are
required for the containing resource. Keyed layers apply required-field checks
only to entries that exist.

Use child layers for data owned by the parent and normally reached through the
parent. Use a separate saved resource when the child has its own identity,
lifecycle, or important lookup paths.

## Identity Keys

Keys in the `at ^root(...)` clause identify the saved resource:

```mw
resource Book at ^books(id: int)
    required title: string

const id = Book::Id(17)
^books(id).title = "Small Gods"
```

Identity is owned by the store. The canonical identity type is `Id(^books)`: the
store plus its key. When a resource has only that one store, the store auto-exports
an ergonomic alias named after the resource, so everyday code writes `Book::Id`.
That alias is store-derived sugar, not identity owned by the resource. If one
resource has several stores, each store names a distinct alias (for example
`DraftBook::Id` and `PublishedBook::Id`). Ordinary typed code passes the identity
value, not the raw key.

Composite identities work the same way:

```mw
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string
    enrolledAt: instant
```

The enrollment is identified by both `studentId` and `courseId`.

Identity keys live in the saved path. They are not ordinary stored fields.
If the resource also stores the same business values as fields, those fields
use separate field names.
Key names are part of the managed layer namespace. A resource keyed by `id`
does not also declare a field or child layer named `id`.

Ordinary typed code addresses a managed root through the store's identity alias:

```mw
const id = Enrollment::Id(
    studentId: "student-1",
    courseId: "course-9",
)

^enrollments(id).status = "active"
```

The runtime lowers that identity into the declared key segments before it
reaches the backend.

Use identity keys when changing a key means "this is a different record."
Identity keys do not change in place. Changing identity means creating a new
record and explicitly transforming or deleting any data that should not remain
under the old identity.

## Element Documentation

Resource elements may have documentation comments:

```mw
resource Book at ^books(id: int)
    ;; Display title shown in search and shelf views.
    title: string
```

Documentation comments feed generated docs, editor hover, inspect output, and
LSP help. They do not change the saved path, the field name, the runtime value,
or the type of the element. In the example above, code still reads and writes
`title`:

```mw
^books(id).title = "Small Gods"
```

Source stable-id annotations are not part of v0.1. Durable element identity is
catalog work; for this release, data evolution treats source spelling and
explicit maintenance code as the contract.

Adding a sparse element is a source change. Adding a required element requires
explicit data-evolution work that populates existing saved resources before code
depends on the field. Adding an index requires a backfill or rebuild of the
generated index tree when matching base data already exists.

## Indexes

Use an index when a value is only an alternate lookup path:

```mw
resource Book at ^books(id: int)
    title: string
    shelf: string

    index byShelf(shelf, id)
```

Changing `shelf` moves the same book to a different lookup path. It does not
create a different book. The index remains inspectable saved data, and user
code uses the declared index instead of writing separate maintenance code.
Index entries lead back to resource identities; the primary resource remains
the place to read fields.

Declared indexes are direct members of keyed saved resources. A singleton
saved resource has no generated identity for an index entry to point to.
Nested-layer indexing is modeled as a separate resource when it needs a
first-class lookup path.

Generated index entries are populated paths. Non-unique indexes use generated
marker values at identity lookup paths. Unique indexes store the resource
identity at the lookup path.
Typed code reads non-unique index identities through direct iteration or
`keys(...)`. It reads a unique index identity from the lookup path. Generated
marker values are visible only through raw inspection.

Index arguments may name identity keys or top-level fields only. Fields nested
through unkeyed groups are rejected, whether written as a dotted path or as a
bare leaf name, and indexes do not walk keyed child layers. A non-unique index
ends with all resource identity keys in declaration order so each entry is
distinct:

```mw
for id in ^books.byShelf("fiction")
    write($"book {id}: {^books(id).title}")
```

Indexes may be unique:

```mw
resource Book at ^books(id: int)
    isbn: string
    index byIsbn(isbn) unique
```

A unique index can omit the identity key because each populated lookup path
points to one resource identity.

```mw
const id: Book::Id = ^books.byIsbn(isbn)
```

For a composite resource identity, a non-unique index includes all identity
key names. Typed traversal reconstructs the generated identity value instead
of exposing a tuple of raw key components.

An index entry exists only when every indexed value is populated. Sparse
records with absent indexed fields do not create placeholder entries. Unique
sparse indexes reject conflicts among populated keys; absence is not a unique
value.

Indexes describe current resource lookup paths. They do not automatically index
every history entry. If historical data needs its own lookup path, model it as a
resource or add an explicit data-evolution transform.

Unique index conflicts reject the write without committing saved data. If the
conflict error escapes a transaction, the transaction rolls back. If code
catches the error inside the transaction, the failed write has no effect and
the transaction can continue.

Identity keys, declared fields, keyed layers, and index names share the
resource namespace. A resource cannot use the same name for a field and an
index, or for an identity key and a field.

Managed resource elements use declared identifiers. Quoted path segments are for
existing raw data, import, export, data evolution, and repair; they do not create
undeclared fields in a managed resource.

Ordinary code may read declared index trees. Ordinary code does not write them;
repair and derived rebuild are explicit data-evolution tooling work.

## Lookup And Query

Marrow reads saved data with paths, traversal, and declared indexes.

Use the primary saved root when identity is known:

```mw
const title = ^books(id).title
```

Stores, indexes, and keyed child layers are durable iterables. Iterate one with an
ordinary `for` loop; it streams lazily rather than materializing the collection.
Use an index when the access pattern matters:

```mw
for id in ^books.byShelf("fiction")
    print(^books(id).title)
```

Full-store traversal is explicit by iterating the store root, and streams the same
way:

```mw
for id in ^books
    print(^books(id).title)
```

Materialization is explicit: `for id in ^books` streams, while `collect(^books)`
builds an in-memory list and may warn. The concern is not large loops but hidden
ones — an access that hides a scan with no matching index is what the checker
flags.

Marrow does not add a separate storage query language. If code needs a new lookup
path, add an index to the resource and rebuild the generated tree when existing
data should appear through it.

## Managed Writes

Assignment to a typed saved resource is a managed write:

```mw
^books(id).shelf = "favorites"
```

If `shelf` participates in an index, Marrow handles the full update:

1. validate the new value,
2. read the old indexed value,
3. write the field,
4. remove the old index entry,
5. add the new index entry.

That managed write is internally coherent or it reports a typed capability or
storage error before success is visible. Ordinary app code does not call a
special `set(...)` function for indexed fields. Untyped writes that bypass a
managed resource root are rejected unless code runs in an explicit maintenance
mode.

Group several field writes under one root with an `edit` block. Like a field
write it preserves omitted fields and children; unlike a whole-record `=` it does
not clear them:

```mw
edit ^books(id)
    shelf = "fiction"
    subtitle = "A novel"
```

Field writes update existing resources. To create a resource or keyed entry
with required fields, assign a whole tree value or use a transaction that
builds it field by field and leaves it valid before commit. Outside an
explicit transaction, a field write that would create a resource or entry with
missing required fields is rejected. Inside a transaction, newly created
resources and entries are validated before commit.

## History Layers

History is an ordinary keyed child layer inside a resource. It is useful when
only some fields are historical and other fields stay current:

```mw
resource Policy at ^policies(policyId: string)
    status: string
    currentVersion: int

    versions(version: int)
        title: string
        body: string
        approvedAt: instant
```

This keeps current fields at:

```text
^policies(policyId).status
^policies(policyId).currentVersion
```

And historical fields at:

```text
^policies(policyId).versions(version).title
^policies(policyId).versions(version).body
```

Multiple history layers can advance independently:

```mw
names(version: int)
    first: string
    last: string

addresses(version: int)
    line1: string
    city: string
```

The rule of thumb:

- Identity keys define the record.
- Index keys find the record another way.
- History keys select a historical state inside the record.

Writing a current field does not automatically create a new version. Code
writes history entries deliberately.

## Sequences And Keyed Trees

Marrow supports sequences and sparse keyed trees. Both are trees.

The canonical sequence shape is an integer-keyed layer:

```mw
tags(pos: int): string
```

Marrow also accepts `sequence[T]` as sugar for the same 1-based keyed tree:

```mw
tags: sequence[string]
```

`sequence[T]` is built-in type syntax, not a user-defined generic type.

Appending to a sequence writes after the highest populated positive integer
key. It does not fill holes left by delete, failed work, or explicit keyed
writes. Sequence keys are stable storage positions; use an ordered local value
when code needs dense, gap-free positions.

Sequence helpers use positive integer positions. Use an integer-keyed tree
when zero or negative keys are part of the data model.

Keyed trees are for named or sparse layers:

```mw
scores(playerId: string): int
```

Use sequences when integer order is the important access pattern. Use keyed
trees when the keys have meaning, may be sparse, or are iterated in sorted key
order.

Iteration over any layer — forward (`for`, `keys`, `values`, `entries`), reverse
(`reversed`), or by stored neighbor (`next`, `prev`) — visits only stored entries,
in key order, and skips holes. A gap left by a delete, by failed work, or by
sparse keys is passed over, never visited as an empty position. This stored-only,
gap-skipping, key-ordered walk is the storage guarantee the `reversed`,
`next`, and `prev` helpers in the builtins reference rest on.

## Reading And Writing

Read and write fields directly:

```mw
const title: string = ^books(id).title
^books(id).shelf = "fiction"
```

Read or write whole local resources:

```mw
var book: Book = ^books(id)
book.shelf = "favorites"
^books(id) = book
```

A whole-resource read materializes the resource's fields — its top-level scalars
and any unkeyed nested groups — into a local value. It does not pull in keyed
child layers such as history, sequences, or keyed trees; those are read, written,
and traversed through their saved paths (for example `^books(id).versions(v)`).
A whole read is useful for small records and construction; read or traverse the
child layers you need directly.

Whole-resource assignment is exact. It replaces the saved resource for that
identity, clearing every field, unkeyed group, and keyed child layer omitted from
the assigned value. To preserve children while updating current state, use a field
write or an edit block instead of `=`.

The compiler checks resource fields before runtime. Runtime reads from saved
data also validate bytes before returning typed values.

If saved bytes do not match the resource schema, typed reads raise a typed
error. Raw inspection can still show the stored bytes for repair.

## Sparse And Required Elements

An unpopulated element is not a value. It is absent from the tree. That is
different from an empty string, zero, or false.

```mw
subtitle: string
```

Rules:

- A maybe-present element must be resolved at the read. An unresolved read of a
  maybe-present place is a compile error that names the place and its resolutions;
  it never raises a runtime fault and never returns a stored null. Resolve it with
  `place ?? fallback`, `const x = place else <diverge>` (return, throw, break, or
  continue), an `if let x = place` or `if exists(place)` branch, or optional
  chaining `a?.b?.c` that ends in one of those.
- `path ?? default` returns `default` for an unpopulated sparse path. It
  does not hide schema errors.
- `exists(path)` checks whether a value or child exists and narrows the path
  inside the guarded block.
- `delete path` removes the value and child tree at that path. Deleting an
  already absent sparse path or resource identity has no effect.
- `required` fields must be populated for a valid resource.
- A `required` field inside a keyed layer is required for entries that exist,
  not for every possible key.

```mw
if exists(^books(id).subtitle)
    write(^books(id).subtitle)

const subtitle: string = ^books(id).subtitle ?? ""
```

## Delete

`delete` removes the value and child tree at a path:

```mw
delete ^books(id).subtitle
delete ^books(id)
```

When `delete` targets a managed saved resource, Marrow also updates the
generated index entries for that resource.

Deleting a required field is rejected unless the surrounding keyed entry or
resource is being deleted, or code is running in explicit maintenance mode.

`merge` is a reserved word, not a v0.1 statement. For partial updates that keep
existing data, use field writes or an edit block rather than a whole-record `=`.

Deleting one resource identity is ordinary application work. Deleting a whole
managed root is maintenance work. Code must opt into maintenance mode. The
operation may still fail with a typed storage limit when the selected store
cannot delete that subtree safely. Delete does not follow identity values
stored in other resources. Cascading cleanup is ordinary application or
data-evolution code.

## Backup And Restore

Backups are portable saved-data archives. They contain ordered paths, values,
and a small manifest. They are not engine files.

Generated index trees are included as saved paths.

With matching or bundled source available, tools can show backup entries as
typed resources. Without source, tools can still restore or inspect the raw
tree.

Normal restore writes into an empty target. Non-empty restore modes are explicit
maintenance actions.

## Transactions

A transaction makes multiple saved writes commit or roll back together:

```mw
transaction
    ^books(id).shelf = "fiction"
    delete ^books(id).loanedTo
```

Most single-record managed writes do not need an explicit transaction in app
code. Use a transaction when a group of saved writes must stay coherent, such
as a record update plus an audit entry or several resources that must change
together.

Nested transactions are savepoints. An inner transaction can roll back its own
saved writes without committing the outer transaction. A successful inner
transaction becomes durable only when the outermost transaction commits.
If an inner transaction rolls back and the outer code catches the error, the
outer transaction can continue.

If a transaction block exits without an escaping error, it commits its saved
writes before leaving. That includes exit by `return`, `break`, or `continue`.
If an error escapes the block, saved writes from that transaction roll back,
including generated index writes. Local variable mutation is ordinary program
state and is not rewound by a transaction rollback. Output and host effects
already performed are not rewound by a saved-data rollback.

An error caught inside the transaction is ordinary control flow; rollback
happens only if an error still escapes the transaction block.

Reads inside a transaction see earlier saved writes from the same transaction.
Outside the transaction, changes become visible through normal Marrow reads as
committed saved data, or they roll back. Marrow does not require application
code to handle half-applied generated indexes.

ID allocation is allowed to leave gaps, including gaps left by failed or
rolled-back work. Treat IDs as opaque identifiers, not business counters.

## Locks

Source-level `lock` is not part of v0.1. Use transactions for saved-data
atomicity in ordinary `.mw` code. Backend and server layers may still use their
own writer coordination outside the source language.

## Managed Saved Trees

When a resource owns a saved root, writes under that root go through the
resource schema. Raw untyped writes to managed roots are rejected unless code
explicitly opts into maintenance mode.
Maintenance mode is selected by tools for data evolution, repair, restore, and
root-wide work; ordinary application code does not enter it by accident.

This protects managed indexes, history layers, and typed fields from
accidental corruption while still allowing deliberate maintenance functions.
Untyped writes to a managed root are rejected when the runtime can enforce the
boundary. Otherwise the project treats the writer as an explicit maintenance or
data-integrity risk.

## Passing Resource Places

Functions can accept resource values as normal inputs. Mutating the caller's
local resource must be explicit:

```mw
fn normalize(inout book: Book)
var draft: Book = ^books(id)
normalize(inout draft)
```

`inout` at the call site makes hidden writes visible. First-class storable
references to saved places are not part of the ordinary application model.
Saved paths are not valid `inout` arguments.
