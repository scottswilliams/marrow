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
yields its next key, and a local sequence yields its 1-based integer positions.
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

## Sequences

`sequence[T]` and a one-`int`-key leaf use positive, 1-based positions. Entries
may have holes. Reads at zero or a negative position are absent. A dynamic write
to a non-positive position fails at runtime.

`append(collection, value)` writes after the greatest present positive
position and returns that position. It does not fill an earlier hole. Iteration
visits present positions only.

```mw
module docs::sequences

pub fn labels(): int
    var values: sequence[string]
    append(values, "first")
    values(3) = "third"

    var seen = 0
    for position, value in values
        print($"{position}: {value}")
        seen += 1
    return seen
```

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

`keys(local)` and `values(local)` materialize local sequences from a local
sequence or keyed collection. They do not accept durable collections. They also
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

Indexes belong to a store and are maintained from its typed fields:

```mw
module docs::indexes

resource Book
    required title: string
    shelf: string
    isbn: string

store ^books(id: int): Book
    index byShelf(shelf, id)
    index byIsbn(isbn) unique

pub fn printShelf(shelf: string)
    for id in ^books.byShelf(shelf)
        if const title = ^books(id).title
            print(title)

pub fn findIsbn(isbn: string): Id(^books)?
    return ^books.byIsbn(isbn)
```

A non-unique index ends with the complete store identity suffix. Iteration of a
branch yields store identities in index order. A complete unique lookup returns
`Id(^root)?`.

Each index argument names either one store identity column or one plain
top-level field of the stored resource. A field nested through an unkeyed group,
a keyed child layer, and a sequence field cannot be an index component. Index
components must be orderable key values. In addition to the ordinary scalar key
types, a plain top-level enum or entry-identity field may be indexed.

If an indexed sparse field is absent, the entry has no row in that index.
Assignment, clearing, whole replacement, and deletion update affected indexes
automatically and atomically with the source data.

An index is a durable iterable, not a materialized local value. A bare
non-unique index root traverses all stored identity rows in complete index
order. A keyed branch such as `^books.byShelf("fiction")` restricts the leading
index components.
