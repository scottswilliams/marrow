# Resources And Saved Data

Resources are the center of Marrow `.mw`. A resource is a typed tree shape. The
same shape can be used for local values, local keyed trees, or saved data.

This page uses "saved data" for data marked with `^`. Saved data persists in
the project's typed tree store. Local data has no `^` and exists only while code
runs.
The `^` sigil is a semantic lifetime marker, not a promise that the bytes live
on disk. The supported production saved-data backend is the native redb backend;
the in-memory store backs tests and programs with no durable declarations, and
`marrow run` refuses a durable program on it. Future backends may choose
different physical residency while still satisfying the backend contract;
source remains `^`.

Ordinary application code declares stores for saved roots. Inspection, data
evolution, backup, and restore tools operate through checked tree-cell facts
rather than treating backend keys as source semantics.

Saved address syntax is logical. Marrow decides how roots, keyed layers, fields,
and indexes are stored for the selected backend. Code depends on the resource
shape, not on physical storage details.

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

Saved roots are project-wide in v0.1. Any module may read or write any saved
root through its declared store shape; saved roots are not visibility gates.

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
const id: Id(^enrollments) = Id(^enrollments, "student-1", "course-9")

^enrollments(id).status = "active"
```

The runtime lowers that identity into the declared key segments before it
reaches the backend.

`Id(^store, key...)` constructs an identity from the declared key components. It
does not read the store, does not allocate a new identity, and does not prove the
record exists. The checker rejects the wrong number of key arguments, wrong key
scalar types, and unchecked `unknown` values; convert dynamic input before it
reenters typed saved data.

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

Documentation comments are accepted on declarations and members and preserved
by the formatter. They do not change the saved address, the field name, the runtime
value, or the type of the field. In the example above, code still reads and writes
`title`:

```mw
^books(id).title = "Small Gods"
```

Durable identity is owned by the accepted catalog, not by source spelling. The
committed artifact is `marrow.catalog.json`; the store keeps a private copy
beside the data it identifies so state-establishing commands can commit catalog
rows and data together and repair interrupted file renders. There is no JSON ABI,
no `^catalog` root, no catalog resource, standard-library, or data-CLI
surface, and no stable-id annotations in source. Each resource, store, field,
keyed layer, index, enum, and enum member gets an opaque stable id, recorded when
the project first runs against a durable store. The accepted catalog advances on
`run` for changes that mutate no stored record, and through `evolve apply` for
record work and data-loss decisions; every advance writes through one store
transaction, then renders `marrow.catalog.json` from the committed snapshot.
Because identity lives in the catalog, renaming a field in source does not change
its durable identity or move stored data — the rename carries identity forward
through `evolve rename` or an alias the accepted catalog already records (see
Evolution below).

Adding a sparse field is a source change. Adding a required field requires
explicit data-evolution work that populates existing saved resources before code
depends on the field. Adding an index requires a backfill or rebuild of the
generated index tree when matching base data already exists.

## Application Surfaces

A `surface Name from ^root` declaration is a checked application contract over an
existing store. It does not declare saved data, mint catalog identity, change the
source digest for durable data, or alter backup, restore, or evolution
obligations. The backing store, projected fields, generated write inputs, and
collection aliases resolve to existing checked facts.

The initial checker contract admits only direct store-backed shapes:

- `from ^root` resolves to one declared store root.
- `fields` resolves each name to a top-level unkeyed field on the backing
  resource. Identity keys, store indexes, groups, keyed child layers, nested
  paths, and fields from other stores are not field targets.
- `create` and `update` resolve each name to a field already named by `fields`.
  Generated write-only inputs are deferred.
- `collection ^root as alias` names the backing store root.
- `collection ^root.index as alias` names an index declared on the backing store.

The checker records typed surface facts over store, member, and index identities
and derives read-operation facts over the checked backing-record footprint and
public projection. It records no surface fact when the backing store, the
store's normalized resource shape, a referenced field, or a referenced index is
already rejected by schema or checker validation; best-effort schema facts are
not a public application contract.

Configured test-file `surface` declarations are still parsed and checked for
source-level name collisions, but only source-root declarations resolve into
application surface facts.

Those facts are transport-neutral: HTTP routes, opaque cursor-token codecs,
TypeScript names, generated clients, and write bodies are boundary profiles
layered later. Stable surface reads have checker-owned accepted-catalog
descriptors and operation tags. `marrow-run` exposes admitted transport-neutral
node and collection read executors over stable surface facts, and
`marrow-json` decodes checked read request-parameter DTOs and renders
already-executed read results as DTOs with accepted catalog IDs and typed
values. Runtime output uses accepted store and resource-member catalog IDs as
semantic identity; enum and identity field values use accepted catalog IDs as
well. Source names remain render labels. A stable exported surface read
operation cannot use proposal-only catalog IDs; until every referenced durable
fact has an accepted catalog ID, the facts carry a source-only catalog status
rather than a stable client contract. A pending catalog proposal for the checked
source is reported as its own blocker, because accepted IDs alone do not prove
the current store, member, or index shape is the committed shape. Deferred
surface profiles are tracked in [Surface ABI Future
Profiles](../future/surface-abi.md).

## Indexes

Use an index when a value is only an alternate lookup path:

```mw
resource Book
    required title: string
    required shelf: string

store ^books(id: int): Book
    index byShelf(shelf, id)
```

Indexes are owned by stores. Declare the resource shape first, then declare the
saved root with `store ^books(...): Book` and place index declarations inside
the store body.

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

Saved keyspace traversal may replace the final provided key argument with an
ordered range bound. This applies to non-unique index branches, store-root
keyspaces, and keyed child layers:

```mw
for id in ^posts.byDate(start..end)
    print(id)

for y in ^cells(1, lo..hi)
    print(y)

for pos in ^books(id).tags(lo..hi)
    print(pos)
```

Accepted forms are `start..end`, `start..=end`, `start..`, `..end`, and
`..=end`. Earlier arguments remain exact prefix keys, so each traversal walks
only entries under that exact prefix and bounded final component. Store-root
keyspaces and keyed layers range their final declared key component under exact
leading components.
Non-unique indexes may range only a component whose following components are the
store identity suffix; the ranged traversal therefore yields store identities
and can be used directly in one-name or two-name resource loops. A range that
would leave another non-identity index component to enumerate is rejected; write
that component as an exact key first or model a different index order.
Range bounds are accepted for ordered scalar or enum index components and
ordered scalar store/layer key components, not identity-typed components, and
they do not apply to unique indexes. A bare `..`, `start..=`, non-trailing range,
composite endpoint, or `by` step in a saved key argument is rejected.
Ranged saved-key calls are traversal shapes, not value reads: use them as loop
iterables, through `keys`/`values`/`entries` loop wrappers, or in supported
cardinality/presence calls. In v0.1, ranged `exists(...)` is supported for
non-unique index branches; store-root and keyed-layer ranges are traversed
rather than tested as a single lookup value.

Index arguments may name store keys or top-level fields only. Field components
may be orderable scalars, enums, or `Id(^store)` typed references; an identity
field is indexed by a store-id-prefixed canonical identity payload, so
references to stores with the same key shape remain distinct. Fields nested
through unkeyed groups are rejected, whether written as a dotted path or as a
bare leaf name, and indexes do not walk keyed child layers. A non-unique index
ends with all store identity keys in declaration order so each entry is
distinct:

```mw
for id in ^books.byShelf("fiction")
    if const title = ^books(id).title
        print($"book {id}: {title}")
```

Indexes may be unique:

```mw
module docs::unique_index

resource Book
    isbn: string

store ^books(id: int): Book
    index byIsbn(isbn) unique

fn findByIsbn(isbn: string, fallback: Id(^books)): Id(^books)
    return ^books.byIsbn(isbn) ?? fallback
```

A unique index can omit the identity key because each populated lookup path
points to one store identity. The lookup is maybe-present — no book may carry
that isbn — so the read resolves like any maybe-present place.

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
the source namespace. A store cannot use the same name for a field and an index,
or for an identity key and a field. Encoded record keys are distinct from
structural names: a record key such as `"byShelf"` does not collide with an
index named `byShelf`.

Managed resource members use declared identifiers. Ordinary code cannot use
quoted field segments; the ordinary expression and operator grammar do not
accept them.

Ordinary code may read declared index trees. Ordinary code does not write them;
repair and derived rebuild are explicit data-evolution tooling work.

## Lookup And Traversal

Marrow reads saved data with paths, traversal, and declared indexes.

Use the primary saved root when identity is known:

```mw
const title = ^books(id).title ?? ""
```

Stores, indexes, and keyed child layers are durable iterables. Iterate one with an
ordinary `for` loop; it streams lazily rather than materializing the collection.
Use an index when the access pattern matters:

```mw
for id in ^books.byShelf("fiction")
    if const title = ^books(id).title
        print(title)
```

Bounded saved key ranges compose with the same streaming traversal:

```mw
for id in ^books.byShelfDate("fiction", start..end)
    print(id)

for id, book in ^books.byDate(start..end)
    print(book.title)

for pos in ^books(id).tags(lo..hi)
    print(pos)
```

Full-store traversal is explicit by iterating the store root, and streams the same
way:

```mw
for id in ^books
    if const title = ^books(id).title
        print(title)
```

Materialization stays in the tree model: `for id in ^books` streams identities,
and holding a result means building a local tree — a `sequence` or keyed layer,
the same shape you would save. There is no flat in-memory list and no
in-memory-versus-saved distinction; `^` is the only difference between a local
tree and a saved one. Every traversal is written in source; see
[Cost Model](cost-model.md) for the hidden-traversal rule.

Marrow does not add a separate saved-data access language. If code needs a new lookup
path, add an index to the store and rebuild the generated tree when existing
data should appear through it.

Value forms are the contract above index representation: code reads declared
paths, identities, keys, entries, and values, while the generated index tree
remains a maintained lookup structure.

## Managed Writes

Assignment to a typed saved resource is a managed write:

```mw
^books(id).shelf = "favorites"
```

If `shelf` participates in an index, Marrow handles the full managed write:

1. validate the new value,
2. read the old indexed value,
3. write the field,
4. remove the old index entry,
5. add the new index entry.

That managed write is internally coherent or it reports a typed capability or
storage error before success is visible. Ordinary app code does not call a
special `set(...)` function for indexed fields. Untyped writes that bypass a
managed store root are rejected; maintenance code still uses managed writes.

Field writes change existing resources. To create a resource or keyed entry
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

An explicit keyed field may name a resource as its entry type:

```mw
resource Comment
    required body: string
    meta
        author: string

resource Post
    comments(seq: int): Comment
```

Each `comments(seq)` entry is stored as a keyed group with `Comment`'s fields.
Whole-entry reads and writes use `Comment` values, required `Comment` fields are
checked for entries that exist, and unkeyed groups inside `Comment` materialize
with the entry. Keyed child layers inside `Comment` remain child layers and are
read, written, and traversed through their own saved addresses.
Entry resources must expand to a finite saved member shape; a typed keyed-entry
cycle is rejected. A named explicit keyed value that is not a resource entry must
resolve to an enum value; checker-only names such as `Error` have no saved leaf
encoding.

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
const title: string = ^books(id).title ?? ""
^books(id).shelf = "fiction"
```

Read or write whole local resources:

```mw
if const book = ^books(id)
    print(book.title)

var replacement: Book
replacement.title = "Small Gods"
replacement.author = "Terry Pratchett"
replacement.shelf = "favorites"
^books(id) = replacement
```

A whole-resource read materializes the resource's fields — its top-level scalars
and any unkeyed nested groups — into a local value. It does not pull in keyed
child layers such as history, sequences, or keyed trees; those are read, written,
and traversed through their saved addresses (for example `^books(id).versions(v)`).
A whole read is useful for small records and construction; read or traverse the
child layers you need directly.

Whole-record assignment is exact. It replaces the saved record at that
address, clearing every field, unkeyed group, and keyed child layer omitted from
the assigned value. To preserve children while changing current state, write the
specific fields instead of using `=`. When the replaced record shape has keyed
child layers, the checker warns so a read-modify-write reset is visible before
runtime.

The compiler checks resource fields before runtime. Runtime reads from saved
data also validate bytes before returning typed values.

If a whole-resource materialization finds missing required data, or if a read
finds bytes that do not decode as the checked leaf type, the command stops with
a fatal data-attachment or storage-corruption fault rather than a catchable
`Error`. Checked inspection can still report the stored bytes for repair.

## Sparse And Required Fields

An unpopulated field is not a value. It is absent from the tree. That is
different from an empty string, zero, or false.

```mw
subtitle: string
```

Rules:

- A maybe-present field must be resolved at the read. An unresolved read of a
  maybe-present place is a compile error at the read site;
  it never raises a runtime fault and never returns a stored null. Resolve it with
  `place ?? fallback`, an `if exists(place)` branch, an `if const name = place`
  binding guard, or optional chaining `a?.b?.c` that ends in one of those.
- `path ?? default` returns `default` for an unpopulated sparse path. It
  does not hide schema errors.
- `exists(path)` checks whether the addressed record node, value, or child tree
  exists and narrows the path inside the guarded block.
- `if const name = path` checks the same presence as `exists(path)`. When the
  path is present, it reads the value once, binds it immutably as `name`, and
  runs the guarded block. It can bind saved value reads such as fields,
  singleton roots, fully addressed records or keyed-layer entries, and complete
  unique-index lookups. It does not bind address-only collections such as a
  keyed root, a keyed child layer without its child key, or a non-unique index
  branch.
- `delete path` removes the value and child tree at that path. Deleting an
  already absent sparse path or store identity has no effect.
- `required` fields must be populated for a valid resource.
- A `required` field inside a keyed layer is required for entries that exist,
  not for every possible key.

```mw
if const subtitle = ^books(id).subtitle
    print(subtitle)

const subtitle: string = ^books(id).subtitle ?? ""
```

### Absent Records

An absent store identity is ordinary absence until a checked read proves
otherwise. Code must resolve maybe-present records and fields at the read site,
so an unchecked absent record is a check error rather than a runtime branch.

Declaring a field `required` does not by itself prove presence for arbitrary
saved data. A bare read of a required field through an identity, parameter, or
constructed `Id(^store, key...)` still needs read-site resolution unless the
checker has a concrete narrowing proof. This keeps the source contract honest
when attached data is absent, stale, or under repair.

Once a saved address has been fixed and a whole-resource materialization or
other total read requires data, missing required data is invalid attached data.
A required field missing from an existing record is a fatal
data-attachment/corruption fault. It is not a catchable `Error`, and `??` does
not hide it.

## Delete

`delete` removes the value and child tree at a path:

```mw
delete ^books(id).subtitle
delete ^books(id)
```

When `delete` targets a managed saved resource, Marrow also maintains the
generated index entries for that store.

Deleting a required field is rejected unless the surrounding keyed entry or
resource is being deleted, or a tool/admin maintenance run grants that
capability.

`merge` is a reserved word, not a v0.1 statement. To preserve existing data,
write specific fields rather than a whole-record `=`.

Deleting one store identity is ordinary application work. Deleting a whole
managed root is maintenance work. Ordinary source syntax cannot opt into it;
tools run with an explicit maintenance capability. The operation may still fail
with a typed storage limit when the selected store cannot delete that subtree
safely. Delete does not follow identity values stored in other resources.
Cascading cleanup is ordinary application or data-evolution code.

## Backup And Restore

Typed backup and restore are commands (`marrow backup`, `marrow restore`).
Backups are not engine files: a backup carries a manifest with the source,
catalog, engine-profile, and value-codec facts, plus the canonical tree-cell data
stream as typed cell targets, so it restores under the Marrow storage contract
rather than by copying raw bytes. The generated indexes are derived data, so a
backup omits them and a restore rebuilds them from the restored records.
Backups are deterministic and portable across conforming backends at the same
layout and codec, but byte identity requires matching accepted catalog facts,
engine profile, value codec, and stored data. Stable IDs are random opaque
values that freeze when accepted, so divergent catalog histories may still
freeze distinct accepted IDs for source that looks equivalent.

Restore replays a backup into an empty store by default, or into a counted
replace target with `restore --replace --count N`, and validates the data
conditions required for activation. The replay is all-or-nothing: any checksum,
framing, or verification failure rolls the target back to its prior state.
Managed cells under roots or members the current source/catalog does not declare
are rejected as data-attached integrity failures; restore never treats
raw saved paths as the production backup contract.

Backup-backed inspection is not restore. `marrow data ... --backup` and
`marrow evolve preview --from-backup` validate the same artifact framing,
catalog section, engine/value-codec identity, data checksum, and trailing-byte
contract, then replay the data into an ephemeral memory store. That mount is a
read target only: it never opens or locks the configured native store, never
renders a catalog artifact, and never writes durable state.

## Transactions

A transaction makes multiple saved writes commit or roll back together:

```mw
transaction
    ^books(id).shelf = "fiction"
    delete ^books(id).loanedTo
```

Most single-record managed writes do not need an explicit transaction in app
code. Use a transaction when a group of saved writes must stay coherent, such
as a record write plus an audit entry or several resources that must change
together.

Nested transactions join the enclosing durable transaction. A successful inner
transaction does not commit independently; its saved writes become durable only
when the outermost transaction commits. If an error escapes any transaction
block, the whole outermost transaction rolls back and the error propagates to the
first handler outside that outermost block. Handlers between a nested transaction
and the outermost boundary do not intercept that escaping error.

If a transaction block exits without an escaping error, it commits its saved
writes before leaving. That includes exit by `return`, `break`, or `continue`.
If an error escapes the block, saved writes from that transaction roll back,
including generated index writes. Local variable mutation is ordinary program
state and is not rewound by a transaction rollback. Rollback-sensitive host
effects are rejected inside a transaction before they run. The language builtin
output sink on this page is `print`, which must happen outside the transaction;
the standard-library host sinks `std::log::*`, `std::io::writeText`, and
`std::io::writeBytes` follow the same rule. Host capability reads, such as
clock, environment, and filesystem reads, do not change saved state and may run
inside transactions.

An error caught before it escapes any transaction block is ordinary control
flow; rollback happens only if an error still escapes a transaction block.

Reads inside a transaction see earlier saved writes from the same transaction.
Outside the transaction, changes become visible through normal Marrow reads as
committed saved data, or they roll back. Marrow does not require application
code to handle half-applied generated indexes.

ID allocation is allowed to leave gaps, including gaps left by failed or
rolled-back work. Treat IDs as opaque identifiers, not business counters.

## Concurrency

Source-level `lock` is not part of v0.1. Use transactions for saved-data
atomicity in ordinary `.mw` code; a transaction is not a process lock and does
not coordinate external systems.

The native store allows multiple read-only inspection processes at the same
time. A process that needs write capability excludes and is excluded by every
read-only inspection. If a process opens second while the store file is held by
the other class of holder, the command reports `store.locked`: "The store file
is held open by another process (a writer or a read-only inspection)." Close the
other process, then retry.

Marrow does not provide a durable outbox primitive in source. If saved-data
changes must drive an external effect, model that as ordinary saved data: write
an outbox record in the same transaction as the state change, commit, and let a
separate worker send and mark the record idempotently.

## Presence Narrowing And Mutation

Presence facts are local control-flow facts, not durable promises. The checker
accepts a maybe-present saved read in value position only when that exact read
site is resolved or a still-valid narrowing proves it.

Read-site resolution forms are:

- `place ?? fallback`, which reads a present value or uses the fallback for
  absence;
- `exists(place)`, which returns `bool` and narrows the true branch;
- `if const name = place`, which checks presence, reads once, and binds the
  value in the true branch;
- `?.` optional field chains that end in one of the resolution forms above;
- attached-data traversal such as `for id, value in ^root`, `values(...)`, and
  `entries(...)`, where the traversal supplies the value it found.

An early-return guard also narrows the following statements:

```mw
if not exists(^books(id).subtitle)
    return
print(^books(id).subtitle)
```

That narrowing is valid only while the place and every key expression used to
address it remain unchanged. Reassigning a key variable, mutating a field used as
a key expression, deleting or writing the saved place, replacing a parent record,
or calling a helper that may write saved data invalidates dependent presence
facts. When in doubt, resolve the read at the use site with `if const` or `??`.

Declaring a field `required` is not a narrowing proof. It states what valid
populated records must contain; it does not prove that this run's attached data
currently has the cell at an arbitrary identity. Required data missing during a
whole-resource materialization is fatal invalid attached data, not a sparse
absence branch.

## Managed Saved Trees

When a store owns a saved root, writes under that root go through the
store schema. Raw untyped writes to managed roots are rejected.
Maintenance mode is selected by tools for data evolution, repair, restore, and
root-wide work. It is an admin capability, not source syntax available to
ordinary application code.

This protects managed indexes, history layers, and typed fields from
accidental corruption while still allowing deliberate maintenance functions
through managed writes. Otherwise the project treats the writer as an explicit
data-integrity risk.

## Evolution

Durable schema changes state their intent in an `evolve` block. A bare source
diff implies nothing about stored data: renaming a member in the resource alone
is ambiguous between delete-and-add and identity-preserving rename. The old
catalog entity and its stored cells remain tied to the prior accepted identity,
and the newly spelled member does not inherit them unless source evolution states
that intent. The `evolve` block names what the change means for durable identity:

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
or keyed layer is rejected. The same v0.1 fence applies inside typed keyed-entry
layers such as `Post.comments(seq): Comment`: nested retire, default, and
transform work below that layer fails closed rather than freezing an entry
evolution contract.

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
itself rewrite stored data. A change that backfills, transforms, or destructively
drops stored data is applied explicitly with `evolve apply`; a change that mutates
no stored record is applied automatically when the project next runs. An explicit
Retire-bearing apply requires a recovery choice: `--backup <path>` writes and
validates a typed backup before mutation, while `--no-backup` records the
operator's opt-out in the apply receipt.

## Passing Resource Values

Functions accept resource values as normal inputs. For resources without keyed
child layers, a caller can assign a replacement resource value directly. For
resources with keyed children, update the fields that changed so keyed layers are
preserved:

```mw
fn normalize(book: Book): Book
    var next: Book = book
    next.title = std::text::trim(next.title)
    return next

var draft: Book
if const saved = ^books(id)
    draft = saved
    draft = normalize(draft)
    ^books(id).title = draft.title
```

First-class storable references to saved places are not part of the ordinary
application model.
