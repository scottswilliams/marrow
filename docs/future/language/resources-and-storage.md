# Resources And Saved Data

Future counterpart of
[`../../language/resources-and-storage.md`](../../language/resources-and-storage.md).

## Assigned stable element IDs

Today an element's stable identity is a name-derived string token written by
hand, `@id("book.title")`, which marks a field or layer's rename identity. The
approved redesign replaces that string with an assigned, opaque stable id: an
id-typed token allocated by the LSP rather than derived from the element name.
Because the token is assigned and not name-shaped, a rename never desyncs the
identity, and one uniform marker covers every element. The current name-derived
`@id("...")` remains the implemented form until this lands.

## GUID identity allocation

Saved-root identities are allocated as a single auto-incrementing `int`. A
designed extension adds a GUID allocation policy, written `^x(id: guid)`,
alongside that single `int` policy, for identities that must be unique without a
central counter.

## Scratch data

A designed extension adds scratch roots, written with `~`, for typed tree data
that outlives one local binding but is not saved:

```mw
resource WordStat at ~words(word: string)
    count: int

fn count(word: string)
    ~words(word).count = (~words(word).count ?? 0) + 1
```

Local data has no sigil, scratch data uses `~`, and saved data uses `^`.
Scratch data is run-local: it is available to code that can name the root during
one command, one server request, or one test, and it is cleared before the
next run. It is not included in backups, restores, migrations, saved-data
integrity checks, or ordinary saved-data inspection.

Scratch roots use the same resource shapes, path syntax, presence rules,
traversal helpers, and type checks as saved roots. They are for caches, work
sets, memo tables, intermediate indexes, and other data that needs shared
identity during a run without becoming durable application state.

Because scratch data has no portable saved representation, an implementation may
choose memory-optimized structures for common access patterns. Code still
depends on Marrow's typed tree behavior, not on a particular in-memory data
structure.

## Collection spellings

A designed extension adds `list[T]`, `map[K, V]`, and `set[K]` as spellings for
common tree shapes. These spellings do not add a second object model; they name
ordinary Marrow access patterns.

| Spelling | Tree shape | Use |
|---|---|---|
| `list[T]` | positive integer-keyed sequence | ordered append and traversal |
| `map[K, V]` | keyed tree with values | lookup by a typed key |
| `set[K]` | presence-only keyed tree | membership by a typed key |

Collection keys use the same key rules as keyed trees. A `list[T]` is the
developer-facing spelling for a sequence when storage positions are not the
point of the model. A `map[K, V]` is the developer-facing spelling for a keyed
tree when lookup is the point. A `set[K]` stores membership, not a user-visible
`bool`; a member is present or absent.

A set entry is populated with `insert(path)` and removed with `delete path`.
There is no element value to read.

```mw
var tags: list[string]
append(tags, "fiction")

var counts: map[string, int]
counts(word) = (counts(word) ?? 0) + 1

var seen: set[string]
insert(seen(word))

if exists(seen(word))
    write(word)

delete seen(word)
```

Local and scratch `map` and `set` values may use hash-based in-memory
representations. Saved collection data remains typed tree data: it is ordered,
inspectable, portable, and reached through paths. Use a declared index when a
saved resource needs a maintained alternate lookup path.
