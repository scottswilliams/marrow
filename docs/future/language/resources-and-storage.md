# Resources And Saved Data

Future counterpart of
[`../../language/resources-and-storage.md`](../../language/resources-and-storage.md).

## GUID identity allocation

Saved-root identities are allocated as a single auto-incrementing `int`. A
designed extension adds a GUID allocation policy, written `^x(id: guid)`,
alongside that single `int` policy, for identities that must be unique without a
central counter.

## Ephemeral roots

The `~` sigil is reserved for typed ephemeral roots: process- or session-lived
resource-typed state that can be rebuilt and is never durable truth. The current
language reserves the sigil only; it does not implement `~` declarations or
writes.

```mw
cache ~bookSearch(term: string, book: Id(^books)): SearchHit

cache ~graph: GraphIndex
```

Local data has no sigil, ephemeral data uses `~`, and durable data uses `^`.
Ephemeral roots are for typed state that is reused or shared across calls in one
process or configured session, but can be discarded without corrupting the
application. If losing the data would corrupt the application, it belongs under
`^`.

`~` is not a way to ask for RAM. A future memory-resident durable store remains
a `^` store. Compound root sigils such as `^~` or `~^` are not part of the
model because they mix two different axes: semantic lifetime and physical
residency.

Ephemeral roots reuse resource shapes and ordinary checked reads, writes, and
iteration, but they do not receive catalog identity, do not appear in portable
backups, and are not data-evolved. A source, catalog, type, or build change
discards and rebuilds them by manifest match.

Natural uses are fast computed structures that should not become durable
B-tree indexes: full-text and inverted indexes, graph adjacency, vector indexes,
parsed-import buffers, precomputed models, analytics cubes, and hot read models.
Future language features may include manual process-private roots, derived roots
with `derives from` and `build`, and warm or project-shared caches.

If future ephemeral roots gain identity values, `Id(~root)` is scoped to the
ephemeral lifetime and must not be storable in `^` data.

## Nested index arguments

Declared indexes currently accept identity keys and top-level fields only. A
future extension may allow indexes to target scalar fields nested through
unkeyed groups:

```mw
resource Book
    location
        shelf: string

store ^books(id: int): Book
    index byShelf(location.shelf, id)
```

That extension needs schema resolution and generated write planning to move in
lockstep: writes to the nested field, writes to the containing group, sparse
presence changes, rebuilds, and unique-conflict checks must all maintain the
generated index tree. Dotted paths are the expected spelling because they name
the containing groups. A bare leaf shorthand such as `shelf` would need an
explicit ambiguity rule before it could become part of the language.

## No Triggers

Saved-data writes do not run hidden triggers. Future derived structures,
journals, outboxes, and caches are explicit resources or tooling contracts with
checked write plans; they do not attach imperative callbacks to arbitrary writes.
External effects happen after durable state is committed through ordinary code or
host workers, not through store-internal trigger execution.

## Collection spellings

Future collection spellings are a map/set collection family, not a v0.1
feature. The unbuilt future surface covers local map/set values,
`insert(path)`, `set[K]`, and a `map[K, V]` saved-member spelling returning
with the rest of the family. Ordered sequences already have a spelling,
`sequence[T]`, for the 1-based integer-keyed tree. Current code spells saved
keyed leaves explicitly, such as `scores(key: K): V`, until the whole
collection family is designed.
