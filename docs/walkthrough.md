# The Durable Model, Narrated

This page reads one complete durable application and explains the durable model —
typed places, presence, transactions, and bounded traversal — as the code meets
it. The application is the **Workshop tool-crib catalog**, the durable preview
fixture at
[`fixtures/v01/conformance/workshop/src/main.mw`](../fixtures/v01/conformance/workshop/src/main.mw).
It is a conformance fixture: its own `test` declarations run its journeys through
the production compile, verify, and run path, so every excerpt below is executed,
not illustrative. Read the [quickstart](quickstart.md) first for the build and run
commands.

The catalog tracks the tools in a workshop's crib. Each tool is an *asset* with a
few identifying fields and many details filled in over time, plus a dated log of
things that happened to it. A second root holds application counters.

## Shape: a resource with required and sparse fields

A `resource` declares the stored shape of one kind of entry. A `required` field
must be present in every stored entry; a field without the marker is *sparse* and
may be absent.

```text
resource Asset {
    required tag: string
    required name: string
    required category: string

    manufacturer: string
    model: string
    location: string
    acquiredOn: date
    purchaseCents: int
    checkedOutTo: string
    conditionNote: string

    log[seq: int] {
        required text: string
        required at: instant
        pinned: bool
    }
}
```

Three fields identify and classify the asset and are always present. The rest are
sparse: a real crib fills in a manufacturer or a location for some tools and not
others, and an absent sparse field costs nothing. `log` is a *branch* — a keyed
family of child entries nested one level under the asset, addressed by an `int`
sequence number. This is the classic payload-plus-subnodes shape: the asset
carries its own fields and owns a family of dated log entries beneath it.

A second resource is a single counter, and the two roots are declared with
`store`:

```text
resource Tally {
    required count: int
}

store ^assets[id: int]: Asset {
    index byCategory[category, id]
    index byTag[tag] unique
}

store ^tallies[name: string]: Tally
```

`^assets` is a durable root keyed by an `int` id; `^tallies` is keyed by a
`string` name. The `^` prefix marks durable lifetime in the language — it is not a
storage key and exposes no backend representation. The `store` body declares two
narrow managed indexes on `^assets`: `byCategory` gives an ordered access path by
category, and `byTag` is `unique`, so at most one asset carries a given tag. The
compiler maintains both indexes atomically with the primary data; application code
never updates an index by hand.

## Transactions: atomic writes across roots

Every durable write happens inside an explicit `transaction`. A single region may
touch several roots, and it commits — or on any fault rolls back — as one unit.
`add` writes the asset's payload, its first log entry, and a counter on the other
root together:

```text
pub fn add(id: int, tag: string, name: string, category: string, at: instant): bool {
    transaction {
        if exists(^assets[id]) {
            return false
        }
        ^assets[id] = Asset(tag: tag, name: name, category: category)
        ^assets[id].log[1] = Asset.log(text: "catalogued", at: at)
        const priorCatalogued = ^tallies["catalogued"].count ?? 0
        ^tallies["catalogued"].count = priorCatalogued + 1
    }
    return true
}
```

`exists(^assets[id])` is a presence probe: it asks whether an entry is present
without reading it, and the guard returns early — without writing — when the id is
already taken. The three writes that follow span two roots (`^assets` and
`^tallies`). Because they share one `transaction`, a reader either sees all of
them or none: a fault before the region ends rolls back the asset, its log entry,
and the counter together. `??` supplies a default for an absent value, so the
first catalogued asset reads a prior count of `0`.

The presence rule is not cosmetic. `recordMove` writes a sparse `location` and
advances a counter with no `exists` guard:

```text
pub fn recordMove(id: int, location: string) {
    transaction {
        ^assets[id].location = location
        const priorMoves = ^tallies["moves"].count ?? 0
        ^tallies["moves"].count = priorMoves + 1
    }
}
```

If the asset is absent, setting its `location` alone cannot satisfy the entry's
`required` `tag`/`name`/`category`, so the whole region rolls back across both
roots with `run.required_missing` and the moves counter is not advanced by a move
that never happened. The counter can never drift from the relocations that
actually landed.

## Presence: reads and guarded updates

A durable read returns an optional value. Reading a sparse field yields the field
type made optional:

```text
pub fn location(id: int): string? {
    return ^assets[id].location
}
```

The `string?` return type states plainly that the location may be absent — the
entry may not exist, or the sparse field may be unset. The caller handles both.

A `place` binds a durable location once and proves it present, so a field-exact
update touches one field of an entry known to exist without re-reading the whole
record:

```text
pub fn setLocation(id: int, location: string): bool {
    transaction {
        place slot = ^assets[id]
        if not exists(slot) {
            return false
        }
        slot.location = location
    }
    return true
}
```

`place slot = ^assets[id]` names the entry's location; `exists(slot)` proves it;
`slot.location = location` writes the single field. Nothing materializes the whole
asset, and the log branch beneath it is untouched.

## Partial copies: read the whole entry, rework it, write it back

Some updates are easier to express over a whole-entry value. Reading `^assets[id]`
as a value copies every field into a local; a plain function reworks the local
copy; and writing it back replaces the payload:

```text
fn withLocation(asset: Asset, location: string): Asset {
    var copy = asset
    copy.location = location
    return copy
}

pub fn relocate(id: int, location: string): bool {
    transaction {
        const current = ^assets[id] else {
            return false
        }
        ^assets[id] = withLocation(current, location)
    }
    return true
}
```

`const current = ^assets[id] else { return false }` binds the whole entry and
diverges when it is absent. `withLocation` is an ordinary function over an ordinary value —
no durable place is involved inside it, and the copy is by value, so it cannot
reach back into the store. The read carried every field, so writing the reworked
copy back preserves the fields the change did not touch. The payload-only
whole-entry write leaves the `log` branch in place.

## Entry identity from a unique index

The `byTag` unique index yields an entry's identity, which then addresses a
whole-entry write:

```text
pub fn renameByTag(tag: string, name: string): bool {
    transaction {
        const found = ^assets.byTag[tag] else {
            return false
        }
        const current = ^assets[found] else {
            return false
        }
        ^assets[found] = withName(current, name)
    }
    return true
}
```

`^assets.byTag[tag]` looks the tag up in the unique index and yields an
`Id(^assets)` — a first-class entry identity, not a raw key. `^assets[found]`
dereferences that identity to read the entry, and `^assets[found] = …` writes it
back. An entry identity is root-local: an `Id(^assets)` addresses `^assets` and
never `^tallies`, and the compiler and the independent verifier both reject using
one against another root.

## Bounded traversal: nested `for` with an explicit limit

Durable iteration is ordinary nested `for`, and it is always bounded. `pinnedCount`
walks assets and, for each, walks its log branch, counting pinned entries:

```text
pub fn pinnedCount(): int {
    var total = 0
    for id, asset in ^assets at most 4096 {
        if exists(asset) {
            for seq, entry in ^assets[id].log at most 4096 {
                if const e = entry {
                    if e.pinned ?? false {
                        total += 1
                    }
                }
            } on more {
                return -1
            }
        }
    } on more {
        return -1
    }
    return total
}
```

`at most 4096` is a compile-time bound: the loop freezes the first 4096 keys and
runs the body once per frozen key in key order. `on more` is mandatory and handles
overflow explicitly — here, returning `-1` when a further key existed. There is no
hidden cursor, page, or resumable continuation: traversal states its bound and its
overflow behavior in the source. An index scan reads the same way:

```text
pub fn countInCategory(category: string): int {
    var count = 0
    for assetId in ^assets.byCategory[category] at most 4096 {
        if exists(^assets[assetId]) {
            count += 1
        }
    } on more {
        return -1
    }
    return count
}
```

Iterating `^assets.byCategory[category]` binds each matching asset's identity; the
body dereferences it. The managed index supplies the access path by category
without a query planner, a projection root, or a hand-maintained secondary
structure.

## What the shape adds up to

The whole catalog is ordinary typed code: functions over values, `if`/`for`
control flow, and a handful of durable places marked with `^`. Every visible
durable construct corresponds to a real difference — `transaction` to atomicity,
`exists` to presence, `at most`/`on more` to bounded work, `Id(^assets)` to entry
identity, a managed index to an access path. There is no separate table model,
serializer, repository layer, migration script, or query language between the code
and the durable data. Read the same source with its `test` declarations at
[`fixtures/v01/conformance/workshop/src/main.mw`](../fixtures/v01/conformance/workshop/src/main.mw),
and the [durable places](language/durable-places.md),
[traversal and indexes](language/traversal-and-indexes.md), and
[errors and transactions](language/errors-and-transactions.md) references for the
exact rules.
