# Data Modeling

How to shape saved data in Marrow: saved roots, child layers, identity keys,
sparse and required fields, relationships, history, indexes, and the lookup
patterns they enable.

[`language/resources-and-storage.md`](language/resources-and-storage.md) and
[`language/types.md`](language/types.md) give the full rules; this page shows how
to put them together.

A resource is a typed tree shape. The same shape can be a local value or saved
data. Saved data is marked with `^` and persists in the project database; local
data has no `^` and exists only while code runs.

## Saved Roots

A resource attaches to a saved root with an `at ^root(...)` clause. The resource
owns that root: writes under it go through the resource schema.

```mw
resource Book at ^books(id: int)
    required title: string
    required author: string
    shelf: string
```

`^books(id)` is now a saved `Book`. A saved root has exactly one owning
resource — another resource cannot claim `^books` with a different shape. Use
child layers, indexes, or a separate root instead.

A resource can also own a root with no identity keys — a singleton:

```mw
resource Settings at ^settings
    theme: string
    required maxLoans: int
```

A singleton has no generated identity type; the root itself is the resource, and
its fields live directly at `^settings.theme` and `^settings.maxLoans`.

Marrow stores no hidden existence marker. A resource identity exists when its
saved path has a value or children. If existence must be detectable for an
otherwise-empty record, model at least one `required` field.

## Child Layers

Indentation in a declaration mirrors the saved tree. There are two kinds of
nesting:

```mw
resource Patient at ^patients(id: string)
    name
        first: string
        last: string

    visits(date: date)
        note: string
        provider: string
```

- An unkeyed group (`name`) is structural. It groups fields under the containing
  resource. A `required` field inside it is required for the whole resource. Its
  fields read as `^patients(id).name.first`.
- A keyed layer (`visits(date: date)`) creates repeatable children. Each entry
  is addressed by its key: `^patients(id).visits(someDate).note`. Required-field
  checks apply only to entries that exist, not to every possible key.

Use a child layer for data owned by the parent and normally reached through it.
Use a separate saved resource when the child has its own identity, lifecycle, or
important lookup paths.

A whole-resource read materializes top-level scalars and unkeyed groups into a
local value. It does not pull in keyed child layers — those are read through
their saved paths or traversed with `keys`, `values`, and `entries`:

```mw
var local: Book = ^books(id)        ; scalars + unkeyed groups
for pos in keys(^books(id).tags)    ; keyed layers read directly
    write($"tag {pos}: {^books(id).tags(pos)}")
```

## Identity Keys

Keys in the `at ^root(...)` clause identify the record. For one key, the
generated identity type wraps that stored key:

```mw
resource Book at ^books(id: int)
    required title: string

const id = Book::Id(17)
^books(id).title = "Small Gods"
```

Composite identities list more than one key and still produce one identity type:

```mw
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    required status: string
    enrolledAt: instant

const id = Enrollment::Id(
    studentId: "student-1",
    courseId: "course-9",
)
^enrollments(id).status = "active"
```

`Enrollment::Id` is treated as a single opaque identity, not a tuple. The
runtime lowers each key component into the saved path. With one record at
`studentId: 1, courseId: "cs101"`, the saved tree is:

```text
^enrollments(1)("cs101").status   active
```

Rules to model around:

- Identity keys live in the path, not as fields. A resource keyed by `id` does
  not also declare a field, group, or layer named `id`. To also expose the same
  business value as a readable field, give the field a different name.
- Identities are opaque and may have gaps. Do not encode business meaning into
  them or treat them as gap-free counters; failed or rolled-back work can leave
  unused IDs behind.
- Identity keys do not change in place. Changing a key means "this is a different
  record" — create a new record and delete or migrate the old one.

For a single `int` key, `nextId` allocates the next identity:

```mw
const id: Book::Id = nextId(^books)
```

`nextId` is only available for a single-`int` identity. Composite and
non-integer identities are allocated by application code, then constructed:
`Enrollment::Id(studentId: ..., courseId: ...)`. Convert at boundaries (URLs,
command arguments, host IO) with the identity constructor, e.g. `Book::Id(17)`.

## Sparse vs Required Fields

Fields are sparse by default. A sparse declaration says what type the element
has when populated. An unpopulated element is not an empty string, zero, or
false — there is simply no node in the tree.

```mw
subtitle: string         ; sparse: may be absent
required title: string   ; must be populated for a valid resource
```

Reading an unpopulated element raises `run.absent_element` unless the checker
can prove it exists. Guard with `exists`, or read with a default:

```mw
if exists(^books(id).subtitle)
    write(^books(id).subtitle)

const subtitle: string = get(^books(id).subtitle, "")
```

`get(path, default)` is for sparse paths only; it does not hide schema or
decode errors. A `required` field inside a keyed layer is required for entries
that exist, not for every possible key.

### Creating valid records

The reliable way to create a record with required fields is to build the whole
value and assign it, or build it inside a transaction that is valid before
commit:

```mw
var b: Book
b.title = "Small Gods"
b.author = "Terry Pratchett"
const id = nextId(^books)
^books(id) = b                     ; whole-resource assignment
```

A transaction can also build a record field by field as long as it is valid
before commit. This is the shape to reach for when one logical change spans the
record and its child layers:

```mw
const id = nextId(^books)
transaction
    ^books(id).title = "Small Gods"
    ^books(id).author = "Terry Pratchett"
```

Whole-resource assignment replaces the record for that identity: fields and
child entries absent from the assigned value are removed. Use single field
writes to update an existing record without disturbing its other fields or its
keyed child layers:

```mw
^books(id).shelf = "favorites"     ; current-only update
```

## Relationships

Marrow has no implicit foreign keys. Saving an identity does not create a
constraint, cascade, or join; it is a typed value. Applications enforce
relationship rules in code, or model the relationship as a resource plus an
index.

Model a reference by storing the related record's key as a scalar field, and
reconstruct the identity when you traverse:

```mw
resource Author at ^authors(id: int)
    required name: string

resource Book at ^books(id: int)
    required title: string
    required authorId: int
    index byAuthor(authorId, id)

pub fn link(authorId: Author::Id, title: string): Book::Id
    var bk: Book
    bk.title = title
    bk.authorId = int(authorId)        ; store the raw key
    const id = nextId(^books)
    ^books(id) = bk
    return id

pub fn printByAuthor(authorId: Author::Id)
    for id in keys(^books.byAuthor(int(authorId)))
        const ref = Author::Id(^books(id).authorId)
        print($"{^books(id).title} by {^authors(ref).name}")
```

Store the related key as a scalar (`int` here) and reconstruct the identity with
its constructor when needed. A field declared with a generated identity type
(for example `authorId: Author::Id`) is not yet supported by the runtime write
path; such a write fails with `run.unsupported`. Use a scalar key field plus
explicit identity construction, as above.

Cascading cleanup is ordinary application or migration code. `delete` does not
follow identity values stored in other resources.

## History as Keyed Child Layers

History is an ordinary keyed child layer. It is useful when only some fields are
historical and others stay current:

```mw
resource Policy at ^policies(policyId: string)
    required status: string
    required currentVersion: int

    versions(version: int)
        required title: string
        required changedAt: instant
```

Current fields stay at the top:

```text
^policies(policyId).status
^policies(policyId).currentVersion
```

History entries live under the keyed layer:

```text
^policies(policyId).versions(version).title
^policies(policyId).versions(version).changedAt
```

Writing a current field does not automatically create a new version; code writes
history entries deliberately, usually together with the current update in a
transaction:

```mw
pub fn revise(id: Policy::Id, title: string, changedAt: instant)
    transaction
        const version: int = ^policies(id).currentVersion + 1
        ^policies(id).currentVersion = version
        ^policies(id).versions(version).title = title
        ^policies(id).versions(version).changedAt = changedAt
```

Multiple history layers can advance independently (for example `names(version)`
and `addresses(version)` under the same resource).

The rule of thumb:

- Identity keys define the record.
- Index keys find the record another way.
- History keys select a historical state inside the record.

Indexes describe current lookup paths; they do not automatically index history.
If historical data needs its own lookup path, model it as a resource or add an
explicit backfill.

## Indexes

Use an index when a value is an alternate lookup path, not a different record.
Indexes are declared members of keyed saved resources:

```mw
resource Book at ^books(id: int)
    required title: string
    shelf: string
    isbn: string

    index byShelf(shelf, id)
    index byIsbn(isbn) unique
```

Rules the project checker enforces (reported when you `marrow check
<projectdir>` or `marrow run`):

- A non-unique index must end with all identity keys in declaration order so
  each entry is distinct. Omitting them is rejected with
  `schema.index_missing_identity_keys`.
- A unique index may omit the identity key, because each populated lookup path
  points to exactly one record.
- An index requires a keyed root. A singleton has no identity for an entry to
  point to (`schema.index_requires_keyed_root`).
- Index arguments may name identity keys, fields, or nested fields through
  unkeyed groups. They do not walk keyed child layers.

Index entries exist only when every indexed value is populated; absent fields
create no placeholder entry. A unique index rejects conflicts among populated
keys with `write.unique_conflict`; absence is not a unique value.

Index maintenance is automatic. Writing an indexed field validates the value,
removes the old entry, and adds the new one as one coherent managed write — no
special `set` call. On a unique conflict the write is rejected with nothing
committed.

Ordinary code reads declared index trees but does not write them; index repair
and backfill are migration work — see [Schema Changes And
Migrations](migrations.md) for backfilling an index added to a populated root.

## Lookup Patterns

Marrow reads saved data with paths, traversal, and declared indexes. There is no
separate query language — if you need a new access pattern, add an index and
backfill its tree.

By identity, when the identity is known:

```mw
const title = ^books(id).title
```

By unique index, where one populated path resolves to one identity:

```mw
const id: Book::Id = ^books.byIsbn(isbn)
const title = ^books(id).title
```

By non-unique index, iterating the identities under a branch:

```mw
for id in keys(^books.byShelf("fiction"))
    print($"{id}: {^books(id).title}")
```

`keys(...)` is the lightest traversal — it yields identities only. On a managed
root, plain iteration follows the declared layer: `^books` yields book
identities, `^books.byShelf("fiction")` yields the identities in that branch.
Use `values(...)` and `entries(...)` on primary roots and ordinary keyed layers;
the marker values that back non-unique index entries are a raw-inspection detail,
not typed data.

By traversal, following a stored reference and reconstructing the identity:

```mw
const author = ^authors(Author::Id(^books(id).authorId)).name
```

## Inspecting the Saved Tree

Tools read the same saved tree that code does. The `marrow data` commands are
read-only and never modify the store:

```text
$ marrow data dump <projectdir>
^books(1).author          Terry Pratchett
^books(1).shelf           fiction
^books(1).title           Small Gods
^books.byShelf("fiction")(1)   1
^books.byIsbn("111")      0x028000000000000001
```

This makes the modeling rules visible: identity keys appear in the path
(`^books(1)`), not as fields; a non-unique index ends with the identity key and
stores a marker value; a unique index stores the encoded identity. Quoted path
segments such as `byShelf("fiction")` are index/string keys and never collide
with a structural name like an index named `byShelf`.

Other inspection commands: `marrow data roots` (list saved roots), `marrow data
stats` (count roots and records), `marrow data get <projectdir> <path>` (one
path's value), and `marrow data integrity` (verify stored values decode against
the schema — note this checks decoding, not required-field completeness).

## What Is Deferred

Some maintenance operations are not yet implemented:

- `marrow data diff` and `marrow data load` are deferred — they overlap
  restore's replace/merge/repair modes and need typed source fingerprinting.
- Restore today writes into an empty target. Replace, merge, and repair restores
  are deferred maintenance actions.
- A field declared with a generated identity type is not yet accepted by the
  write path; model references with a scalar key field plus identity
  construction (see [Relationships](#relationships)).
