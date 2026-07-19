# Durable Places

A durable place begins with `^` and a declared store root. The prefix marks
saved lifetime in the language. It does not expose a storage key or backend
representation. Every durable read and write named below reaches the store
through the compiler-checked path kernel; application code never receives a raw
key, an engine handle, or a transaction object.

## Store Declarations

A keyed store attaches a resource type to one typed identity key component:

```mw
module docs::durable

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn put(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn title(id: int): string? {
    return ^books[id].title
}
```

Each identity key component is drawn from the closed orderable durable-key scalar
set: `int`, `string`, `bool`, `bytes`, `date`, or `instant`. Nominal source types
are not durable identity keys and report `check.unsupported`. `duration` is a span
rather than an identity and is not a durable key. A resource declares fields that
are `required` or sparse; a sparse field may be absent. The store root is
project-wide: any module uses the declared root shape directly, and function
visibility does not change root access.

A project may declare more than one store root. Each root has its own name and its
own keyed entry family, and each is addressed by its own name in ordinary function
bodies. Two store roots may not share a name. A single `transaction` region may read
and write several roots; its writes commit, or on a fault roll back, as one atomic
unit across every root it touched.

### Durable Field Values

A stored field holds a value from the closed acyclic durable value set:

| Field value | Notes |
|---|---|
| a scalar | `int`, `bool`, `string`, `bytes`, `date`, `instant`, `duration` |
| a nominal scalar | stored as its base scalar |
| a dense `struct` | its leaves stored inline; the struct's leaves are structure, not separately named durable declarations |
| a closed `enum` | a user `enum`, `Option`, or `Result`; each variant is a member |
| an `Option` of any of the above | `Option<T>` where `T` is itself an admitted value |

The value graph must be acyclic (a struct or enum may not contain itself,
directly or transitively). A collection is not stored directly in a field — a
large collection belongs under a keyed `branch` — and a field does not store a
nested sparse resource, a place, a function, or a handle.

A `store` root is either a *singleton* or a *keyed tuple*. A singleton root
omits the key list and holds a single entry:

```mw
module docs::durable_singleton

resource Settings {
    required locale: string
}

store ^settings: Settings
```

A keyed tuple has one or more ordered key components (up to eight); a composite key
identifies each entry by the whole tuple in component order:

```mw
module docs::durable_composite

resource Enrollment {
    required grade: int
}

store ^enrollments[student: string, course: string]: Enrollment

pub fn enroll(student: string, course: string, grade: int) {
    transaction {
        ^enrollments[student, course] = Enrollment(grade: grade)
    }
}

pub fn gradeOf(student: string, course: string): int? {
    return ^enrollments[student, course].grade
}
```

A composite key addresses each entry by its whole tuple in component order, so
`^enrollments[student, course]` and `^enrollments[course, student]` are distinct
entries. Every whole-entry and field operation supplies one key operand per key
component, in declaration order; a `branch` may likewise declare more than one key
component. Bounded traversal, however, iterates a single key component, so a `for`
head over a composite-keyed root or branch layer is the typed `check.unsupported`
rejection (see
[Traversal](traversal-and-indexes.md#bounded-durable-traversal)).

Each root — singleton, singly keyed, or composite — is a distinct durable graph
node with its own complete identity (its placement, its stored product, one
identity per key component, and one per stored field; see
[Durable Identity](#durable-identity)).

A stored resource may also declare static `group` namespaces and keyed `branch`
placements (see [Resources](resources.md#groups-and-branches)). These are part of
the durable graph and complete their identity like a root.

Durable declarations compile, verify, and complete their identity. Durable
execution has returned for source tests: a `test` whose body reads or writes
durable data runs against a fresh in-memory *ephemeral attachment*, so the
operations below execute through the read kernel under `marrow test`. Each durable
test gets its own attachment minted from the verified test image with a ceiling
equal to the test-image demand union, so one test never observes another's writes,
and the attachment is discarded when the test ends — there is no persistent store.
Persistent `marrow run` execution is still in the trough: the CLI no longer opens a
store, so a durable export does not run from `marrow run` (it reports the typed
`cli.durable_unsupported` outcome) until the persistent companion runner lands
(F02). The operations described below are the current durable *language* — they are
checked and their identity is complete; see [Project status](../status.md).

Within that checked language, the *flat* keyed root is the form whose operations the
compiler fully lowers: a keyed root — one or more key components — whose fields are
each a
plain scalar or a widened value (a dense `struct`/record, a closed `enum`, or an
`Option`/`Result`), whose entries are read and written through the operations below. A
widened field value is framed inline in its single field-leaf cell and round-trips as a
runtime value. Such a root's root-level `group` members (of scalar or widened leaves, see
[Groups](#groups)) and its `branch` placements, with one or more key components each, are
executable in the same way; branches nest to any depth (see
[Keyed branches](#keyed-branches)).
A singleton root (no key components), a root whose resource declares a **nominal-typed**
field, or a group nested in a branch or in another group declares and verifies its full
identity, but its read and write operations are not yet lowered — an operation over one
is the typed `check.unsupported` rejection rather than a silent drop, until the remaining
durable runtime lands. (Declaring such a store is no longer a `check.type` on the
resource; the store is identity-complete, only its operations are deferred. A collection
in a field stays rejected outright — a collection belongs under a keyed `branch`, not
inline.)

The compiler emits an **operation site** for every node of the whole durable graph
— a whole-payload site for each keyed placement (the store root and every nested
`branch`), a whole-group site for each root-level `group`, and a field-leaf site for each
stored field (top-level, group-scoped, or branch-scoped) — and the verifier seals each one
by resolving its concrete address against the graph it independently reconstructs. A site
on the flat executable root (its fields scalar or widened), on one of its root-level
groups, or on one of its scalar-field branches at any depth, seals as executable; every
other site — over a group nested in a branch or another group, a nominal-typed field, or a
non-flat root — seals with a complete identity but parks, so its concrete address is
checked and recorded while its execution waits for the remaining kernel. A group leaf has
no site of its own: it is reached through its whole-group site. The site registry holds one
entry per graph node regardless of how many operations reference it, and appending a
sparse field adds one field-leaf site without disturbing any existing site.

## Durable Identity

Every durable declaration — the application, a store root, each of its key
components, the stored resource, each stored field, each static `group` namespace,
each keyed `branch` placement together with its own key components and fields, and
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
mints a ledger entry for each anchor that lacks one and never tombstones. So under the
current mint a rename mints a fresh identity for the new name and leaves the old
ledger entry live and orphaned, and deleting a declaration then re-adding the same path
readopts the old identity. This is harmless in the current trough — no
persistent store is reachable, so no stored data is bound to a stale or readopted
identity. The accepted apply action (future) is what classifies a rename as an
anchor move, records a genuine removal as a tombstone, and surfaces an orphaned
ledger entry.

A program's whole durable graph additionally carries a stable 32-byte
**durable-contract identity**, computed over the graph's ledger ids and shape:
each root's ordered key tuple — scalar and identity per key component — and its
resource's ordered **member tree**. The member tree is the resource's stored
fields (the `required` flag and the stored value shape per field) interleaved
with its static `group` namespaces (each an identity and its own member tree) and
its keyed `branch` placements (each an identity, an ordered key tuple, and its own
member tree). A field's value shape records its scalar, dense struct leaves, or
closed-enum members; a durable-reachable enum contributes its sum identity and
one member identity per variant, so appending an enum member (which preserves the
existing members' identities and order) is a distinct evolution from renaming or
re-typing one. A nested struct's leaves are structure, not separate durable
declarations, so they mint no identities of their own. Key-component and member
order are part of the identity. The compiler derives it from
the resolved
graph and records it in the program image; the independent verifier rebuilds
the descriptor from the image sections, recomputes the identity, and rejects any
image whose recorded identity does not match its graph. Because the descriptor
carries ledger ids rather than names, an anchor move preserves the durable
identity — the ledger anchor moves, the id stays — while every semantic change (a
changed key type, a field made required, a field added or removed, or a
delete-then-re-add onto a fresh id) changes it. These are ledger-model properties
the conformance tests pin. Spelling and declaration order that leave the graph
unchanged leave the identity unchanged. A rename becomes an anchor move only under
the accepted apply action (future); the additive-only `run` mint instead leaves a
renamed declaration's old ledger entry live, as described above. Operation sites — the
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
that may not exist yields `T?`, because the entry at that key may be absent.

Given a store root or a keyed branch family rather than one addressed entry —
`exists(^books)` or `exists(^books[id].notes)` — `exists` instead reports whether
that family has at least one payload-bearing child. It reads at most one child
key and establishes no per-entry presence fact; it answers only the
family-populated question, at a cost bounded like an `at most 1` traversal.

```mw
module docs::durable_presence

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn present(id: int): bool {
    return exists(^books[id])
}

pub fn subtitle(id: int): string? {
    return ^books[id].subtitle
}

pub fn titleOrNone(id: int): string {
    if const book = ^books[id] {
        return book.title
    }
    return "none"
}
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

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn retitle(id: int, title: string) {
    transaction {
        place book = ^books[id]
        book = Book(title: title)
        book.subtitle = "revised"
    }
}

pub fn subtitleOf(id: int): string? {
    place book = ^books[id]
    return book.subtitle
}
```

The right-hand side is a whole durable entry address `^root[key...]`. The key
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
address (`^books[id].title`), or to another place is rejected.

A place binds an address the same way an inline `^root[key...]` operation does, so a
place over a store shape whose operations are not yet lowered (a singleton or
nominal-field root, or one whose only durable content is a group nested in a branch or
another group) reports the same not-yet-executable result as the inline form. A place's
durable node — a store root or a keyed `branch` — follows the address it binds, not its
key-operand count: a place over a composite-key root addresses that root and reads its
fields exactly like the inline `^root[k1, k2].field` form.

## Field Assignment

Assigning one field changes that field and preserves the entry's other fields:

```mw
module docs::durable_field

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn setSubtitle(id: int, subtitle: string) {
    transaction {
        ^books[id].subtitle = subtitle
    }
}

pub fn clearSubtitle(id: int) {
    transaction {
        ^books[id].subtitle = absent
    }
}
```

A sparse field is a **present-or-clear** place: it accepts a value, an optional
value, or `absent`. A present value is stored; an absent value clears the field.
A required field does not accept an optional or absent assignment.

### Guarded And Unguarded Sparse Sets

A sparse-field set takes one of two forms depending on what the compiler knows
about the containing entry.

An **unguarded** set — a set on an inline `^root[key].field` address, or on a
named place whose entry has not been shown to exist — makes no assumption that the
entry is present. The field value is staged, and when the transaction commits the
entry is created if all its required fields are present, or the transaction rolls
back if a required field is missing. This is the create-or-reconcile behavior of
every sparse set; a clear (`= absent`) never creates an entry.

A **guarded** set — a set on a named `place` whose containing entry a preceding
test in the same block has shown to be present — is statically known to update an
existing entry. The entry is proven present by an `if exists(place)` test, by
binding it with `if const x = place`, or by a preceding whole-entry write
`place = Record(...)` in the same block. The compiler emits the strict form of the
set, which reads the place's one pre-evaluated key and updates the field of the
entry already there. Clearing the entry with `delete place` withdraws the
knowledge, so a set after it is unguarded again.

A presence fact is tracked per binding, not per entry. Two `place` bindings to the
same entry hold independent facts, so a `delete` through one binding does not
withdraw the proof held by another binding to the same entry. A strict set through
the now-stale binding does not corrupt data: it fails closed at the store's marker
check with a `run.corruption` fault rather than writing. Avoid the corner by
routing an entry's presence test, erase, and subsequent writes through a single
`place` binding. A check-time rule that recognized aliasing bindings is possible
but not yet implemented.

```mw
module docs::durable_guarded

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn setSubtitleIfPresent(id: int, subtitle: string) {
    transaction {
        place book = ^books[id]
        if exists(book) {
            book.subtitle = subtitle
        }
    }
}
```

Both forms have the same observable result when the entry is present; the guarded
form additionally records, in the compiled program and its independent verifier,
that the update lands on an entry the program proved to exist.

## Whole Resource Assignment

Assignment to the entry address is exact replacement:

```mw
module docs::durable_replace

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn replace(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}
```

The assignment stores every field named by the constructed value and removes any
omitted sparse field. A required field left unset when the entry commits rolls
the transaction back with `run.required_missing` rather than storing a partial
entry. The replacement rewrites the entry's own payload only; its keyed `branch`
descendants are preserved.

## Groups

A resource may declare a root-level unkeyed `group`: a static field-path namespace whose
leaves are part of the containing entry's own payload (see
[Resources](resources.md#groups-and-branches)). A group of scalar or widened leaves is
executable. A group is *markerless* — its presence is the entry's presence, so it is
addressed by the entry's own key-path — and it is a value unit: it is read, replaced, and
erased whole through `^root[key…].group`, and each leaf is read and rewritten through
`^root[key…].group.leaf`:

```mw
module docs::durable_group

resource Book {
    required title: string

    details {
        pages: int
        language: string
    }
}

store ^books[id: int]: Book

pub fn setBook(id: int, title: string, pages: int, language: string) {
    transaction {
        ^books[id] = Book(title: title, details: Book.details(pages: pages, language: language))
    }
}

pub fn pages(id: int): int? {
    return ^books[id].details.pages
}

pub fn setPages(id: int, pages: int) {
    transaction {
        ^books[id].details.pages = pages
    }
}

pub fn clearPages(id: int) {
    transaction {
        delete ^books[id].details.pages
    }
}

pub fn replaceDetails(id: int, pages: int) {
    transaction {
        ^books[id].details = Book.details(pages: pages)
    }
}

pub fn eraseDetails(id: int) {
    transaction {
        delete ^books[id].details
    }
}

pub fn detailPages(id: int): int? {
    if const d = ^books[id].details {
        return d.pages
    }
    return absent
}
```

A group leaf read (`^books[id].details.pages`) materializes the whole group and projects
the leaf, so it reads `absent` when the entry is absent and follows each leaf's own
presence otherwise. A group-leaf assignment or `delete` is a whole-group read-modify-write:
it reads the group, updates the one leaf, and writes the group back, so a sibling leaf is
preserved. Because the group is a value unit of an existing entry — never created on its
own — a leaf write over an absent entry is a no-op. A whole-group assignment
(`^books[id].details = Book.details(...)`) is exact: it rewrites the group's own leaves and
drops every leaf the assigned value omits, and a `delete` of the whole group clears only
that group's leaves. All three leave the entry's top-level fields and its keyed `branch`
descendants in place. A whole-entry read materializes the entry's root-level groups along
with its top-level fields, and a whole-entry assignment rewrites them exactly, dropping an
omitted all-sparse group's leaves along with any omitted top-level sparse field.

A group nested in a branch or in another group is not yet executable; its identity is
complete, but an operation over it reports the typed `check.unsupported` result.

## Keyed Branches

A resource may declare a keyed `branch`: a nested keyed subtree with its own key
component and stored fields (see [Resources](resources.md#groups-and-branches)). A
`branch` keyed by one component and holding only scalar fields is executable, and its
own members may include further such branches, so a chain of singly keyed scalar-field
branches is executable to any depth. Each level's entries are addressed by extending the
parent's key-path with the branch key — `^root[key].branch[bkey]`,
`^root[key].branch[bkey].sub[skey]` — and the same operations apply at every level:

```mw
module docs::durable_branch

resource Book {
    required title: string

    notes[noteId: string] {
        required text: string
        pinned: bool

        tags[tagId: int] {
            required weight: int
        }
    }
}

store ^books[id: int]: Book

pub fn addNote(id: int, noteId: string, text: string) {
    transaction {
        ^books[id].notes[noteId] = Book.notes(text: text)
    }
}

pub fn addTag(id: int, noteId: string, tagId: int, weight: int) {
    transaction {
        ^books[id].notes[noteId].tags[tagId] = Book.notes.tags(weight: weight)
    }
}

pub fn setPinned(id: int, noteId: string, pinned: bool) {
    transaction {
        ^books[id].notes[noteId].pinned = pinned
    }
}

pub fn tagWeight(id: int, noteId: string, tagId: int): int? {
    return ^books[id].notes[noteId].tags[tagId].weight
}

pub fn noteText(id: int, noteId: string): string? {
    if const note = ^books[id].notes[noteId] {
        return note.text
    }
    return absent
}

pub fn removeNote(id: int, noteId: string) {
    transaction {
        delete ^books[id].notes[noteId]
    }
}
```

A whole branch entry is created or replaced with the qualified constructor
`Resource.branch.…(field: value, …)` — here `Book.notes(text: text)` and
`Book.notes.tags(weight: weight)` — symmetric with the root constructor `Book(…)`, one
level down per branch. The head of the path is the resource type name: a value binding
may not shadow it in that position. As for the root, a branch create supplies every
required field, and `exists`, whole-entry read, whole replacement, and `delete` all
address the branch entry through its full key-path.

A field operation may address a branch field directly — `^root[key].branch[bkey].text`
or the deeper `^root[key].notes[nid].tags[tid].weight` — to read, set, or `delete` one
leaf without materializing the whole entry. Reading a whole branch entry with
`if const note = ^root[key].branch[bkey]` instead materializes the branch's record,
whose fields — such as `note.text` — read locally through the binding.

A branch entry is a distinct durable node from its ancestors. Creating a branch entry
under absent ancestors leaves each ancestor *descendant-only*: it has keyed descendants
but no payload of its own, so it reads payload-absent and `exists` is `false` until it
is given a payload with `create`. This holds uniformly at depth — a deep write under
absent ancestors is admitted, and the ancestors gain no marker. Giving a node a payload
does not disturb its branches, and a whole-entry `delete` or replacement is payload-only:
it removes the addressed node's own payload while preserving its keyed `branch`
descendants at every level.

## Deletion

`delete place` removes the addressed place:

```mw
module docs::durable_delete

resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn removeSubtitle(id: int) {
    transaction {
        delete ^books[id].subtitle
    }
}

pub fn remove(id: int) {
    transaction {
        delete ^books[id]
    }
}
```

Deleting a sparse field that is already absent is a no-op. Deleting a required
field is rejected. Deleting a whole entry removes the entry's payload — its marker
and its own stored fields; any keyed `branch` descendants persist. A node that has
keyed descendants but no payload of its own is *descendant-only*: it reads
payload-absent and `exists` is `false`, yet its descendants remain reachable at their
own addresses.

## Access Demand

Each exported function has a derived **access demand**: the set of durable places
its whole call graph touches, paired with the operation each makes there. An
operation is one of `read`, `write`, `presence`, `erase`, or `iterate` — the same
source vocabulary the operations above are written in. A place is named by its
durable path (the resource root and, for a field operation, the field), not by a
storage location. A function that reads one field demands `read` of that field; a
function that reads a field and then writes it demands both `read` and `write` of
that field.

Demand describes the access a program *requires*; it never grants access. The
compiler emits the operation points and the verifier independently rebuilds each
export's demand from them — the demand is not stored in the program image. Two
exports that touch the same place with the same operation carry the same demand
atom, and a program-wide union of every export's demand describes the whole
program's durable footprint. Whether an invocation is permitted to exercise a
demand is a separate concern that intersects demand with the access a deployment
and an invocation allow; that intersection is not yet part of the current
language.
