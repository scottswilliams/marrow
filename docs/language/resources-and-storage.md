# Resources And Saved Data

Resources are the center of Marrow `.mw`. A resource is a typed tree shape. The
same shape can be used for local values, local keyed trees, or saved data.

This page uses "saved data" for data marked with `^`. Saved data persists in
the project database. Local data has no `^` and exists only while code runs.

Ordinary application code declares stores for saved roots. Inspection, import,
export, data evolution, repair, and restore tools operate through checked
tree-cell facts rather than treating backend keys as source semantics.

Saved address syntax is logical. Marrow decides how roots, keyed layers, fields,
and indexes are stored for the selected backend. Code depends on the resource
shape, not on physical storage details.

Encoded record keys are distinct from structural names. A record key such as
`"byShelf"` does not collide with an index named `byShelf`.

Typed traversal keeps those segment kinds separate: `^books` streams book
identities, `values(^books)` streams `Book` values, and
`^books.byShelf(...)` streams identities from that index branch.

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
resource Book
    required title: string
    required author: string
    shelf: string

store ^books(id: int): Book
```

Then `^books(id)` is a saved `Book`, where `id` is the store identity
canonically modeled as `Id(^books)`.

Saved resources are not hidden blobs. The book above is stored as fields and
layers under `^books(id)`, so tools can inspect the same tree that code reads.

A store identity can exist with no populated fields. Existence is an explicit
structural fact: `exists(^books(id))` is true once the node exists, independent of
any field value or child. There is no need to model a required field just to record
that a resource exists.

A saved root has one managed store schema. If `^books` stores `Book`, another
store cannot claim `^books` with a different shape. Use nested layers, indexes,
or a separate root instead.

A store declaration may omit identity keys when the root itself is addressed
directly:

```mw
resource Settings
    theme: string
    required maxLoans: int

store ^settings: Settings
```

## Tree Layers

Indentation in a resource declaration mirrors the saved/local tree:

```mw
resource Patient
    name
        first: string
        last: string

    visits(date: date)
        note: string
        provider: string

store ^patients(id: string): Patient
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

Keys in the `store ^root(...)` declaration identify the saved resource:

```mw
resource Book
    required title: string

store ^books(id: int): Book

const id: Id(^books) = nextId(^books)
^books(id).title = "Small Gods"
```

Identity is owned by the store. The canonical identity type is `Id(^books)`: the
store plus its key. Ordinary typed code passes the identity value, not the raw
key.

Composite identities work the same way:

```mw
resource Enrollment
    status: string
    enrolledAt: instant

store ^enrollments(studentId: string, courseId: string): Enrollment
```

The enrollment is identified by both `studentId` and `courseId`.

Identity keys live in the store address. They are not ordinary stored fields.
If the resource also stores the same business values as fields, those fields
use separate field names.
Key names are part of the managed layer namespace. A store keyed by `id`
does not also declare a field or child layer named `id`.

Ordinary typed code addresses a managed root through the store identity:

```mw
const id: Id(^enrollments) = loadEnrollmentId("student-1", "course-9")

^enrollments(id).status = "active"
```

The runtime lowers that identity into the declared key segments before it
reaches the backend.

Use identity keys when changing a key means "this is a different record."
Identity keys do not change in place. Changing identity means creating a new
record and explicitly transforming or deleting any data that should not remain
under the old identity.

## Field Documentation

Resource fields may have documentation comments:

```mw
resource Book
    ;; Display title shown in search and shelf views.
    title: string

store ^books(id: int): Book
```

Documentation comments feed generated docs, editor hover, inspect output, and
LSP help. They do not change the saved address, the field name, the runtime
value, or the type of the field. In the example above, code still reads and writes
`title`:

```mw
^books(id).title = "Small Gods"
```

Durable identity is owned by an invisible catalog, not by source spelling. The
catalog is compiler and tooling infrastructure: there are no stable-id
annotations in source and no user-facing catalog command. Each resource, store,
field, keyed layer, index, and enum member gets a random opaque stable id,
recorded automatically the first time the project runs and advanced only by
`evolve apply`; `check` never writes it. Because identity lives in the catalog,
renaming a field in source does not change its durable identity or move stored
data — the rename carries identity forward through `evolve rename` or an alias
the accepted catalog already records (see Evolution below).

Adding a sparse field is a source change. Adding a required field requires
explicit data-evolution work that populates existing saved resources before code
depends on the field. Adding an index requires a backfill or rebuild of the
generated index tree when matching base data already exists.

## Indexes

Use an index when a value is only an alternate lookup path:

```mw
resource Book
    title: string
    shelf: string

store ^books(id: int): Book
    index byShelf(shelf, id)
```

Indexes are owned by stores. The concise `resource ... at ^books(...)` source
form is accepted as declaration sugar and desugars to the split resource plus
store form above.

Changing `shelf` moves the same book to a different lookup path. It does not
create a different book. The index remains inspectable saved data, and user
code uses the declared index instead of writing separate maintenance code.
Index entries lead back to store identities; the primary resource remains
the place to read fields.

Declared indexes require keyed stores. A singleton
saved root has no store identity for an index entry to point to.
Nested-layer indexing is modeled as a separate resource when it needs a
first-class lookup path.

Generated index entries are populated paths. Non-unique indexes use generated
marker values at identity lookup paths. Unique indexes store the store identity
at the lookup path.
Typed code reads non-unique index identities through direct iteration or
`keys(...)`. It reads a unique index identity from the lookup path. Generated
marker values are visible only through checked inspection tooling.

Index arguments may name store keys or top-level fields only. Fields nested
through unkeyed groups are rejected, whether written as a dotted path or as a
bare leaf name, and indexes do not walk keyed child layers. A non-unique index
ends with all store identity keys in declaration order so each entry is
distinct:

```mw
for id in ^books.byShelf("fiction")
    write($"book {id}: {^books(id).title}")
```

Indexes may be unique:

```mw
resource Book
    isbn: string

store ^books(id: int): Book
    index byIsbn(isbn) unique
```

A unique index can omit the identity key because each populated lookup path
points to one store identity.

```mw
const id: Id(^books) = ^books.byIsbn(isbn)
```

For a composite store identity, a non-unique index includes all identity
key names. Typed traversal reconstructs the store identity value instead
of exposing a tuple of raw key components.

An index entry exists only when every indexed value is populated. Sparse
records with absent indexed fields do not create placeholder entries. Unique
sparse indexes reject conflicts among populated keys; absence is not a unique
value.

Indexes describe current store lookup paths. They do not automatically index
every history entry. If historical data needs its own lookup path, model it as a
resource or add an explicit data-evolution transform.

Unique index conflicts reject the write without committing saved data. If the
conflict error escapes a transaction, the transaction rolls back. If code
catches the error inside the transaction, the failed write has no effect and
the transaction can continue.

Identity keys, declared fields, keyed layers, and shorthand index names share
the source namespace. A concise resource declaration cannot use the same name
for a field and an index, or for an identity key and a field.

Managed resource members use declared identifiers. Quoted field spelling is an
ordinary managed field access; it does not create undeclared fields or bypass
write planning in a managed resource.

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

Materialization stays in the tree model: `for id in ^books` streams identities,
and holding a result means building a local tree — a `sequence` or keyed layer,
the same shape you would save. There is no flat in-memory list and no
in-memory-versus-saved distinction; `^` is the only difference between a local
tree and a saved one. The concern is not large loops but hidden ones — an access
that hides traversal with no matching index is what the checker flags.

Marrow does not add a separate storage query language. If code needs a new lookup
path, add an index to the store and rebuild the generated tree when existing
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
managed store root are rejected; maintenance code still uses managed writes.

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
resource Policy
    status: string
    currentVersion: int

    versions(version: int)
        title: string
        body: string
        approvedAt: instant

store ^policies(policyId: string): Policy
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
and traversed through their saved addresses (for example `^books(id).versions(v)`).
A whole read is useful for small records and construction; read or traverse the
child layers you need directly.

Whole-resource assignment is exact. It replaces the saved resource for that
identity, clearing every field, unkeyed group, and keyed child layer omitted from
the assigned value. To preserve children while updating current state, write the
specific fields instead of using `=`.

The compiler checks resource fields before runtime. Runtime reads from saved
data also validate bytes before returning typed values.

If saved bytes do not match the resource schema, typed reads raise a typed
error. Checked inspection can still show the stored bytes for repair.

## Sparse And Required Fields

An unpopulated field is not a value. It is absent from the tree. That is
different from an empty string, zero, or false.

```mw
subtitle: string
```

Rules:

- A maybe-present field must be resolved at the read. An unresolved read of a
  maybe-present place is a compile error that names the place and its resolutions;
  it never raises a runtime fault and never returns a stored null. Resolve it with
  `place ?? fallback`, an `if exists(place)` branch, or optional chaining
  `a?.b?.c` that ends in one of those.
- `path ?? default` returns `default` for an unpopulated sparse path. It
  does not hide schema errors.
- `exists(path)` checks whether a value or child exists and narrows the path
  inside the guarded block.
- `delete path` removes the value and child tree at that path. Deleting an
  already absent sparse path or store identity has no effect.
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
generated index entries for that store.

Deleting a required field is rejected unless the surrounding keyed entry or
resource is being deleted, or code is running in explicit maintenance mode.

`merge` is a reserved word, not a v0.1 statement. For partial updates that keep
existing data, write specific fields rather than a whole-record `=`.

Deleting one store identity is ordinary application work. Deleting a whole
managed root is maintenance work. Code must opt into maintenance mode. The
operation may still fail with a typed storage limit when the selected store
cannot delete that subtree safely. Delete does not follow identity values
stored in other resources. Cascading cleanup is ordinary application or
data-evolution code.

## Backup And Restore

Typed backup and restore are commands (`marrow backup`, `marrow restore`).
Backups are not engine files: a backup carries a manifest with the source,
catalog, engine-profile, and value-codec facts, plus the canonical tree-cell
stream, so it restores under the Marrow storage contract rather than by copying
raw bytes.

Restore replays a backup into an empty store in one transaction and validates the
data against the schema before activating it; it never treats raw saved paths as
the production backup contract.

Non-empty restore modes are explicit maintenance actions.

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
state and is not rewound by a transaction rollback. Rollback-sensitive host
effects are rejected inside a transaction before they run: program output,
logging, and filesystem writes must happen outside the transaction. Host
capability reads, such as clock, environment, and filesystem reads, do not change
saved state and may run inside transactions.

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

When a store owns a saved root, writes under that root go through the
store schema. Raw untyped writes to managed roots are rejected.
Maintenance mode is selected by tools for data evolution, repair, restore, and
root-wide work; ordinary application code does not enter it by accident.

This protects managed indexes, history layers, and typed fields from
accidental corruption while still allowing deliberate maintenance functions
through managed writes. Otherwise the project treats the writer as an explicit
data-integrity risk.

## Evolution

Durable schema changes state their intent in an `evolve` block. A bare source
diff implies nothing about stored data: renaming a member in the resource alone
changes the on-disk path and orphans the data behind it. The `evolve` block names
what the change means for durable identity:

```mw
evolve
    rename Book.title -> Book.subtitle
    default Book.author = "unknown"
    retire ^books.byTitle
```

`rename old -> new` declares that the entity now spelled `new` is the durable
entity formerly spelled `old`, so its stable identity and stored data carry
forward and the old path is kept as an alias. A saved-data-backed rename is
rejected unless an `evolve rename` states this intent, or the accepted catalog
already records the alias; either way authorizes it, so identity is never silently
reassigned. `default` gives the value to backfill where a newly populated member
is absent. `retire` is destructive: it states intent to remove an entity and its
stored data. `transform` computes the new shape of an entity from the old through
a checked body.

A `default` value must be a constant the checker can evaluate when the change is
discharged: a literal such as `"unknown"`, `0`, or `true`. The same fill is written
into every record that lacks the member, so a value that varies per record is a
`transform`, not a `default`. A non-constant `default` is rejected with a diagnostic
pointing to `transform`.

A `transform` targets a top-level saved member and computes it per record from the
record's other members:

```mw
evolve
    transform Book.priceCents
        return old.price * 100
```

The target must be a top-level member of the resource; a nested member under a group
or keyed layer is rejected.

Inside the body, `old` is the record before this evolution, read-only and typed
against the current schema; `old.<member>` reads that member's value. The body is a
pure function of `old` only: it computes the target as a total function of `old` with
operators and pure helpers, and may not read or write any saved data (a `^` path),
perform host effects, open a transaction, or call a project function. A body that
reads saved data is rejected, because a transform sees one record at a time through
`old` and may not reach across records. Its result must type as the target member.

A transform reads *other* members, never the value it replaces: reading `old.<target>`
is rejected. It also may not read a member the same `evolve` block changes with a
`default` or another `transform`: `old` exposes the pre-evolution value, not the value
that change produces, so the result would be computed from data the same evolution is
replacing. To reinterpret a member's own stored value, add a new member computed from
it and retire the old one rather than transforming it in place.

Soundness rests on the read members, not on remembering the old types: before a
transform applies, every value the body reads must still decode under that member's
current type. A record whose stored bytes no longer decode fails the change closed
with a repair diagnostic, so a transform applies only over data that is unchanged or
compatibly widened in the members it reads.

The intent is checked against the source and the accepted catalog; it does not
itself rewrite stored data. Applying the change is an explicit maintenance action.

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
