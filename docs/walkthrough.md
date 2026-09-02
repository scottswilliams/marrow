# A durable program, read through

The workshop tool crib is a catalog of tools. Each tool is an asset with three
identifying fields, details filled in over time, and a dated log of what happened
to it. A second root holds the crib's counters. The whole program is one file,
[`fixtures/v01/conformance/workshop/src/main.mw`](../fixtures/v01/conformance/workshop/src/main.mw),
and its `test` blocks run under `marrow test`, so every excerpt on this page is
code that runs. The [quickstart](quickstart.md) covers the commands.

## Shape

A `resource` declares the shape of one kind of entry. A `required` field is
present in every stored entry. A field without the marker is sparse and may be
absent.

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
others, and an absent sparse field stores nothing. `log[seq: int]` is a branch: a
keyed family of child entries one level under the asset, addressed by an `int`
sequence number.

## Places

A `store` declaration gives a resource a durable root and names its key.

```text
store ^assets[id: int]: Asset {
    index byCategory[category, id]
    index byTag[tag] unique
}

store ^tallies[name: string]: Tally
```

`^assets` and `^tallies` are the program's two
[durable roots](language/durable-places.md). `Tally` is a resource with one
required field, `count`. `^assets[id]` is one asset. `^assets[id].log[seq]` is one
of its log entries. `^tallies["moves"].count` is one field of one counter. The
`store` body declares two indexes on `^assets`. `byCategory` orders assets by
category and then by id. `byTag` is `unique`, so at most one asset carries a given
tag. Every write to `^assets` keeps both indexes current; there is no statement
that writes an index.

## Transactions

Every durable write sits inside a `transaction`. When the block ends, its writes
commit together; if it faults, none of them apply. `add` writes the asset, its
first log entry, and a counter on the other root:

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

`Asset(tag: ..., name: ..., category: ...)` constructs a value of the resource
by naming its fields, and `Asset.log(...)` constructs a value of the `log`
branch the same way. `exists(^assets[id])` asks whether the entry is present
without reading its fields. When the id is already taken, the guard returns
before any write. The three writes that follow span two roots and commit as
one. `??` supplies a default for an absent value, so the first catalogued asset
reads a prior count of `0`.

`recordMove` writes a sparse `location` and advances a counter with no guard:

```text
pub fn recordMove(id: int, location: string) {
    transaction {
        ^assets[id].location = location
        const priorMoves = ^tallies["moves"].count ?? 0
        ^tallies["moves"].count = priorMoves + 1
    }
}
```

If no such asset exists, the write creates an entry with only `location` set. Its
required `tag`, `name`, and `category` are still absent when the block ends, so the
commit fails with `run.required_missing` and the whole block rolls back, counter
included. The report names the durable outcome, `known_old`: nothing changed
([rollback](language/errors-and-transactions.md#rollback-and-isolation)).

## Presence

Reading a field through a key yields `T?`, because the entry may be absent.

```text
pub fn location(id: int): string? {
    return ^assets[id].location
}
```

`string?` says the location may be absent: the entry, or the sparse field
alone. The caller handles both with `??`, `if const`, or a let-else binding.
Binding the whole entry with `if const` proves it present, and its required
fields become bare, as `label` in the fixture shows.

A `place` names one entry's address and evaluates the key once:

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

`place slot = ^assets[id]` names the entry. `exists(slot)` asks whether it is
present. `slot.location = location` writes one field through the same address.
Nothing reads the whole asset, and the log branch beneath it is untouched. The
`place` itself proves nothing: without the guard, a write through `slot` on an
absent id creates an incomplete entry, and the block rolls back with
`run.required_missing` as in `recordMove`
([named places](language/durable-places.md#named-places)).

## Copies

Some updates are easier to express over the whole entry. Reading `^assets[id]` as
a value copies every field into a local. An ordinary function reworks the copy.
Writing it back replaces the entry's fields.

```text
fn withLocation(asset: Asset, location: string): Asset {
    var copy = asset
    copy.location = location
    return copy
}
```

```text
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
diverges when it is absent. `withLocation` is an ordinary function over an ordinary
value. The copy is by value, so nothing inside it reaches the store. The read
carried every field, so writing the reworked copy back preserves the fields the
change did not touch. The whole-entry write replaces the entry's own fields and
leaves the `log` branch in place.

## Erase and replace

`delete` removes an entry's own fields. A whole-entry assignment replaces them
exactly.

```text
pub fn replace(id: int, tag: string, name: string, category: string): bool {
    transaction {
        if not exists(^assets[id]) {
            return false
        }
        ^assets[id] = Asset(tag: tag, name: name, category: category)
    }
    return true
}

pub fn erase(id: int): bool {
    transaction {
        if not exists(^assets[id]) {
            return false
        }
        delete ^assets[id]
    }
    return true
}
```

Both leave the `log` branch where it is. After `erase(3)`, `present(3)` is false
and `noteText(3, 1)` still reads `"catalogued"`. Removing an asset together with
its log is a bounded walk over the branch and a `delete` per entry
([deleting](language/durable-places.md#deleting)).

`replace` writes an `Asset` that carries only the three required fields. An
omitted sparse field is dropped: after `setLocation(4, "Bay 3")` and
`replace(4, "T-400", "Table Saw", "cutting")`, `location(4)` is absent. To change
one field and keep the rest, write the field through a `place`, or copy the entry
as `relocate` does.

## Identity

A unique index yields an entry's identity, an `Id(^assets)`, and the identity
addresses the entry.

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

`^assets.byTag[tag]` yields an `Id(^assets)`: a value that names one entry of
`^assets` and nothing else. `^assets[found]` reads the entry through it, and
`^assets[found] = withName(current, name)` writes it back. An identity is
root-local. Using an `Id(^assets)` to address `^tallies` is a `check.type` error
([entry identity](language/types-and-values.md#entry-identity)).

## Traversal

Durable iteration is ordinary nested `for`, and it always states its bound.
`pinnedCount` walks the assets and, for each, walks its log:

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

`for id, asset in ^assets` binds the key and a pin. The pin `asset` is a
per-iteration address for `^assets[id]`; it reads nothing and proves nothing.
The frozen keys are taken before the body runs, and an entry erased by an
earlier iteration keeps its key, so `exists(asset)` asks whether the entry is
still present. `at most 4096` is a bound written as an integer literal: the loop
freezes the first 4096 keys and runs the body once per frozen key in key order.
`on more` is mandatory and handles overflow explicitly, here by returning `-1`
when a further key existed. To go on past the
bound, a later call adds `from k` to the loop head, which starts the walk at `k`
inclusive, so a continuation begins at the first key the previous call did not
reach
([bounded traversal](language/traversal-and-indexes.md#bounded-durable-traversal)).
An index walk reads the same way:

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

Iterating `^assets.byCategory[category]` binds each matching asset's identity, and
the body reads the entry through it. The index is a second way to reach an
asset, and every write to `^assets` keeps it current
([reading an index](language/traversal-and-indexes.md#reading-an-index)).

## Where next

- [Durable places](language/durable-places.md): roots, keys, reads, writes, deletion.
- [Traversal and indexes](language/traversal-and-indexes.md): bounded `for`, indexes.
- [Errors and transactions](language/errors-and-transactions.md): commit and rollback.
- [Tests](language/tests.md#durable-tests): the fresh in-memory store per durable test.
- The [fixture source](../fixtures/v01/conformance/workshop/src/main.mw) with its tests.
