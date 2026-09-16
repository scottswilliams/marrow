# A durable program, read through

The workshop tool crib is a catalog of tools. Each tool is an asset with three
identifying fields, details filled in over time, and a dated log of what happened
to it. A second root holds the crib's counters. The whole program is one file,
[`fixtures/v01/conformance/workshop/src/main.mw`](../fixtures/v01/conformance/workshop/src/main.mw),
and its `test` blocks run under `marrow test`, so every excerpt on this page is
code that runs.

This page shows how the pieces compose in one program. The rules themselves are
in the [language reference](language/README.md), which each section links to;
the [quickstart](quickstart.md) covers the commands.

## Shape

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

store ^assets[id: int]: Asset {
    index byCategory[category, id]
    index byTag[tag] unique
}

store ^tallies[name: string]: Tally
```

Three fields identify and classify the asset and are present in every stored
entry; the rest are sparse, because a real crib fills in a manufacturer or a
location for some tools and not others. `log[seq: int]` is a branch: a keyed
family of child entries one level under the asset, so `^assets[id].log[seq]` is
one log entry of one asset ([resources](language/resources.md#members)).

`^assets` and `^tallies` are the program's two
[durable roots](language/durable-places.md). The `store` body declares two
indexes on `^assets`: `byCategory` orders assets by category and then by id, and
`byTag` is `unique`, so at most one asset carries a given tag. Every write to
`^assets` keeps both current; no statement writes an index
([index declarations](language/traversal-and-indexes.md#index-declarations)).

## One transaction over two roots

```text
pub fn add(id: int, tag: string, name: string, category: string, at: instant): bool {
    transaction {
        if exists(^assets[id]) {
            return false
        }
        place catalogued = ^tallies["catalogued"]
        catalogued = Tally(count: (catalogued.count ?? 0) + 1)
        ^assets[id] = Asset(tag: tag, name: name, category: category)
        ^assets[id].log[1] = Asset.log(text: "catalogued", at: at)
    }
    return true
}
```

`add` writes the asset, its first log entry, and a counter on the other root.
The guard returns before any write when the id is taken; the three writes that
follow span two roots and commit as one. `Asset.log(...)` constructs a value of
the branch the way `Asset(...)` constructs the entry
([errors and transactions](language/errors-and-transactions.md)).

## A proof covers a block

```text
pub fn recordMove(id: int, location: string) {
    transaction {
        place slot = ^assets[id]
        if exists(slot) {
            slot.location = location
            place moves = ^tallies["moves"]
            moves = Tally(count: (moves.count ?? 0) + 1)
        }
    }
}
```

A field write updates an entry the compiler has proved present and never creates
one. `place slot = ^assets[id]` names the address and evaluates the key once;
the `place` itself proves nothing, and `exists(slot)` is what covers the rest of
the block. Without that guard the write is `check.requires_presence` at check
time. For an absent asset the block writes nothing
([named places](language/durable-places.md#named-places)).

Reading through a key yields `T?`, because the entry, or the sparse field alone,
may be absent:

```text
pub fn location(id: int): string? {
    return ^assets[id].location
}
```

## Reworking a whole entry

Some updates are easier over the whole entry. Reading `^assets[id]` as a value
copies every field into a local, an ordinary function reworks the copy, and
writing it back replaces the entry's own fields.

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

The copy is by value, so nothing inside `withLocation` reaches the store. The
read carried every field, so writing the reworked copy back preserves the fields
the change did not touch, and leaves the `log` branch in place.

## Replacing and erasing

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

Both act on the entry's own fields and leave the `log` branch where it is: after
`erase(3)`, `present(3)` is false and `noteText(3, 1)` still reads
`"catalogued"`. Removing an asset together with its log is a bounded walk over
the branch and a `delete` per entry
([deleting](language/durable-places.md#deleting)).

`replace` writes an `Asset` carrying only the three required fields, so an
omitted sparse field is dropped: after `setLocation(4, "Bay 3")` and
`replace(4, "T-400", "Table Saw", "cutting")`, `location(4)` is absent. To
change one field and keep the rest, write through a `place` or copy the entry as
`relocate` does.

## Identity from an index

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

`^assets.byTag[tag]` yields an `Id(^assets)`, and `^assets[found]` reads and
writes the entry through it. The identity is root-local: addressing `^tallies`
with it is a `check.type` error
([entry identity](language/types-and-values.md#entry-identity)).

## Nested bounded traversal

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

Durable iteration is ordinary nested `for`, and every loop states its bound and
handles overflow in `on more`. `for id, asset in ^assets` binds the key and a
pin, a per-iteration address that reads nothing and proves nothing by itself:
the keys are frozen before the body runs, so an entry erased by an earlier
iteration keeps its key and `exists(asset)` is what asks whether it is still
there.

To go on past the bound, a later call adds `from k` to the loop head, which
starts the walk at `k` inclusive, so a continuation begins at the first key the
previous call did not reach
([bounded traversal](language/traversal-and-indexes.md#bounded-durable-traversal)).

An index walk reads the same way, binding each matching asset's identity:

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

The index is a second way to reach an asset, and every write to `^assets` keeps
it current ([reading an index](language/traversal-and-indexes.md#reading-an-index)).

## Reusing part of a program

The crib is one file, and nothing requires it to stay one. Pure code a second
project also needs — text helpers, a shared struct — moves into a project
directory of its own, which each consumer names in `marrow.toml` under an alias
and then reaches as `graphtext::text` and `graphtext::Pair`
([quickstart](quickstart.md#using-a-local-library),
[dependencies](language/modules-and-functions.md#dependencies)).

Durable places do not cross that boundary. A `store` root is addressable only
inside the project that declares it, so `^assets` and `^tallies` stay in this
file however much of the rest moves out.

## Where next

- [Durable places](language/durable-places.md): roots, keys, reads, writes, deletion.
- [Traversal and indexes](language/traversal-and-indexes.md): bounded `for`, indexes.
- [Errors and transactions](language/errors-and-transactions.md): commit and rollback.
- [Tests](language/tests.md#durable-tests): the fresh in-memory store per durable test.
- [Dependencies](language/modules-and-functions.md#dependencies): a library's modules and types.
- The [fixture source](../fixtures/v01/conformance/workshop/src/main.mw) with its tests.
