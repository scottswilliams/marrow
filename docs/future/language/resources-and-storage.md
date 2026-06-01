# Resources And Saved Data

Future counterpart of
[`../../language/resources-and-storage.md`](../../language/resources-and-storage.md).

## Catalog-Owned Stable IDs

Stored entities need catalog-owned opaque stable identity. Tools may allocate or
preserve those identities while editing, but the project catalog is the
authority. Because the identity is not name-shaped, a rename never desyncs it
from the data it names.

## GUID identity allocation

Saved-root identities are allocated as a single auto-incrementing `int`. A
designed extension adds a GUID allocation policy, written `^x(id: guid)`,
alongside that single `int` policy, for identities that must be unique without a
central counter.

## Ephemeral roots

ADR 0209 reserves `~` for typed ephemeral roots: process- or session-lived
resource-typed state that can be rebuilt and is never durable truth. v1 reserves
the sigil only; it does not implement `~` declarations or writes.

```mw
cache ~bookSearch(term: string, book: Id(^books)): SearchHit

cache ~graph: GraphIndex
```

Local data has no sigil, ephemeral data uses `~`, and durable data uses `^`.
Ephemeral roots are for typed state that is reused or shared across calls in one
process or configured session, but can be discarded without corrupting the
application. If losing the data would corrupt the application, it belongs under
`^`.

Ephemeral roots reuse resource shapes and ordinary checked reads, writes, and
iteration, but they do not receive catalog identity, do not appear in portable
backups, and are not data-evolved. A source, catalog, type, or build change
discards and rebuilds them by manifest match.

Natural uses are fast computed structures that should not become durable
B-tree indexes: full-text and inverted indexes, graph adjacency, vector indexes,
parsed-import buffers, precomputed models, analytics cubes, and hot read models.
Future waves may add manual process-private roots, derived roots with
`derives from` and `build`, and warm or project-shared caches. Those are future
features, not v1 gates.

## Nested index arguments

Declared indexes currently accept identity keys and top-level fields only. A
future extension may allow indexes to target scalar fields nested through
unkeyed groups:

```mw
resource Book at ^books(id: int)
    location
        shelf: string

    index byShelf(location.shelf, id)
```

That extension needs schema resolution and generated write planning to move in
lockstep: writes to the nested field, writes to the containing group, sparse
presence changes, rebuilds, and unique-conflict checks must all maintain the
generated index tree. Dotted paths are the expected spelling because they name
the containing groups. A bare leaf shorthand such as `shelf` would need an
explicit ambiguity rule before it could become part of the language.

## Collection spellings

A designed extension adds `map[K, V]` and `set[K]` as spellings for two common
keyed-tree shapes. Ordered sequences already have a spelling, `sequence[T]`, the
1-based integer-keyed tree. None of these add a second object model; they name
ordinary Marrow access patterns over typed trees.

| Spelling | Tree shape | Use |
|---|---|---|
| `map[K, V]` | keyed tree with values | lookup by a typed key |
| `set[K]` | presence-only keyed tree | membership by a typed key |

A `map[K, V]` is the developer-facing spelling for a single keyed layer when
lookup is the point; its key follows the same rules as any keyed tree. For more
than one key, use a native multi-layer keyed tree rather than a nested `map`:
`counts(day: date, category: string): int` is flatter and more expressive than
`map[date, map[string, int]]`. Use a declared index when a saved resource needs
a maintained alternate lookup path.

Local map iteration follows the collection rule: one loop variable walks values,
two loop variables walk key/value entries, and `keys(...)` walks keys only.
Durable keyed layers stream keys with one loop variable and read values through
two-name loops, `values(...)`, or `entries(...)`.

A `set[K]` stores membership, not a user-visible `bool`; a member is present or
absent. Because a set member has no value, there is no right-hand side to
assign: `insert(path)` populates a member, much as appending allocates the next
key in a sequence. `delete path` removes a member and `exists(path)` tests one.

```mw
var counts: map[string, int]
counts(word) = (counts(word) ?? 0) + 1

for count in counts
    write($"{count}")

for word, count in counts
    write($"{word}: {count}")

for word in keys(counts)
    write(word)

var seen: set[string]
insert(seen(word))

if exists(seen(word))
    write(word)

for word in seen
    write(word)

delete seen(word)
```

Like `sequence[T]`, `map[K, V]` and `set[K]` are built-in spellings, not
user-instantiable generic types. A collection element accepts no undeclared
children: if an element needs child fields, model it as a named resource or an
explicit keyed group, and if set membership must carry metadata, it is no longer
a set — use `map[K, V]`, for example `map[string, Flag]`.

Saved collection data is typed tree data: ordered, inspectable, portable, and
reached through paths. A local or future ephemeral `map` or `set` has no
portable saved form, so an implementation may choose memory-optimized
structures for it; code still depends on Marrow's typed tree behavior, not on a
particular in-memory data structure.

## Local Tree Writes

Local sequence and keyed-tree variables support the same path-shaped reads and
writes as saved trees, without saved lifetime or backend capability checks:

```mw
var tags: sequence[string]
const first = append(tags, "fiction")
tags(first + 1) = "paperback"

var scores(playerId: string): int
scores(playerId) = (scores(playerId) ?? 0) + 1
```

A local subscript such as `scores(playerId)` is a typed path. It can be read,
assigned, defaulted with `??`, tested with `exists(...)`, deleted, traversed,
or merged according to the same presence and type rules as any other tree path.
The checker rejects keys whose static type does not match the declared layer.

`append(localSequence, value)` writes one greater than the highest populated
positive integer key in that local tree and returns the key it wrote. It skips
holes for the same reason saved sequence append skips holes: sequence positions
are stable tree keys, not dense array indexes.
