# Traversal And Indexes

`for` traverses ranges and populated entries of local or durable collections.
Collection traversal follows the defined [typed key order](types-and-values.md#key-types)
and skips absent positions.

## Key-First Iteration

The first loop variable is an address key. A second variable binds the value at
that key.

```mw
module docs::traversal

resource Book
    required title: string
    tags(pos: int): string

store ^books(id: int): Book

pub fn printAll()
    for id, book in ^books
        print($"{id}: {book.title}")

        for pos, tag in ^books(id).tags
            print($"{pos}: {tag}")
```

With one variable, a store root yields `Id(^root)` values, a keyed child layer
yields its next key, and a local list yields its elements in insertion order.
With two variables, the second binding is the entry or leaf value.

Durable traversal is lazy. It visits stored entries without first creating a
local collection. `reversed` walks the same keys from high to low:

```text
for id in reversed ^books
    print(id)
```

## Composite Layers

For an N-column durable keyed layer, a loop head has either one variable or N+1
variables. One variable binds the outermost key. N+1 variables bind every key
column followed by the leaf value.

```text
for row in ^grids(id).cells
    for column, value in ^grids(id).cells(row)
        print(value)

for row, column, value in ^grids(id).cells
    print(value)
```

Intermediate arities are rejected. A fully keyed leaf is a value place, not an
iterable.

A local keyed collection always accepts one or two loop variables, even when it
has several key columns. One variable visits distinct first-column keys. Two
variables visit every row but expose only its first key and leaf value, so a
first key can repeat. `keys(local)` likewise materializes distinct first keys;
`values(local)` materializes every row leaf in full tuple order. Later key
columns are not exposed by current local iteration, and direct lookup requires
all declared keys.

## Positional Keyed Leaves

A durable one-`int`-key leaf such as `tags(pos: int): string` uses positive,
1-based positions. Entries may have holes. Reads at zero or a negative position
are absent, and a dynamic write to a non-positive position fails at runtime.
`append(place, value)` writes after the greatest present positive position and
returns that position; it does not fill an earlier hole, and iteration visits
present positions only.

Local ordered values use the `List[T]` and `Map[K, V]` collections instead; see
[Lists and maps](types-and-values.md#lists-and-maps).

## Ranges

Range loops use `..` for an excluded end and `..=` for an included end:

```mw
module docs::ranges

pub fn countEven(): int
    var count = 0
    for value in 0..10 by 2
        count += 1
    return count
```

Endpoints may be `int`, `date`, or `instant`. Integer ranges default to step
`1` and may descend with a negative step. Date ranges default to one calendar
day and accept positive whole-day durations. Instant ranges require an explicit
positive duration. A zero step is rejected. A step pointing away from the end
produces no iterations when its direction is not statically known; a
literal-provable dead range is rejected.

Ranges may also appear in the trailing key position of a durable keyed layer or
index traversal, where they restrict the ordered keys visited.

## Collection Built-Ins

`count(collection)` reports present entries. `next(place)` and `prev(place)`
return the neighboring present key, if any; the place supplies both its layer
and current key. For an entry identity, `key(value)` returns its sole declared
raw key.

`keys(local)` and `values(local)` materialize local lists from a local list or
map. They do not accept durable collections. They also
cannot appear directly as a `for` head; bind their result first if a copied view
is needed.

## Mutation During Traversal

A loop over a durable layer must not change the key set of that traversed layer
while it is active. Deleting an entry, appending, replacing a keyed entry, or a
write that changes membership in a traversed index is rejected. Writes beneath
the current entry that do not change the traversed layer may be allowed.

The restriction follows the layer being traversed, including changes made by a
called helper. Copy identities or keys into a local collection first when an
operation must restructure that same layer.

## Index Declarations

A keyed `store` root may declare narrow compiler-maintained ordered indexes. An
index is an ordered projection of the root's identity keys and top-level fields;
it stores no data of its own and has no application write operation. A non-unique
index distinguishes each row by ending with the complete store identity suffix; a
`unique` index may omit it.

```mw
module docs::indexes

resource Book
    required title: string
    shelf: string
    isbn: string

store ^books(id: int): Book
    index byShelf(shelf, id)
    index byIsbn(isbn) unique

pub fn label(): string
    return "books"
```

Each index argument names either one store identity column or one plain top-level
field of the stored resource, in projection order, and no component may repeat. A
non-unique index must end with
every identity key in declaration order, with no identity key in a leading position;
a `unique` index may omit the identity keys. A field reached through an unkeyed
group, a keyed child layer, or a keyed positional leaf cannot be an index component.
Each projected field must store an orderable durable-key scalar (`int`, `string`,
`bool`, `bytes`, `date`, or `instant`; a nominal erases to its base scalar). An index
name shares the root's source namespace with the identity keys and stored fields, so
it may not collide with either or with another index. An index requires a keyed root:
a singleton store admits none.

Each index carries its own stable durable identity in the machine-written
`marrow.ids` ledger (an `index` anchor at `<root>.<index name>`), distinct from every
other durable identity; renaming an index preserves it, and a retired index name is
never reused. The compiler maintains every index — assignment, clearing, whole
replacement, and deletion keep the affected indexes coherent; source has no
operation that writes an index, so an index can never be left incoherent by
application code. A non-unique index read is a progressive typed-prefix refinement
(an incomplete prefix yields the next distinct component; the complete projection
yields the source-root key), and a `unique` index read is a complete-key exact lookup
that yields exactly the one matching `Id(^root)` or absent — never a sibling.

**Future.** Runtime index maintenance and index reads — traversal of a non-unique
index and the exact lookup of a `unique` index — are not yet executable on the beta
line. A source read through an index reports a precise not-yet-supported diagnostic
until the managed-index runtime lands (see `docs/status.md`).
