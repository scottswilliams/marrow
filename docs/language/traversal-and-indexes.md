# Traversal And Indexes

`for` traverses an integer range, a local collection, or a durable root or
branch family. Durable traversal is always bounded: it visits at most a declared
number of immediate keys and states, at the traversal head, what to do when more
remain.

## Bounded Durable Traversal

A durable `for` head names a store root or a single-level keyed branch, an
`at most N` bound, an optional inclusive `from` key, and a mandatory `on more`
clause cuddled after the loop body like an `else`:

```text
for k[, p] in <place> at most N [from f] {
    statements
} on more {
    statements
}
```

`<place>` is a store root such as `^books` (the root entry family) or a keyed
branch such as `^books[id].notes` (the branch family beneath one fixed parent
entry, at any nesting depth). The first loop variable `k` binds each immediate key
in ascending [typed key order](types-and-values.md#key-types); the value at that key
is read separately inside the body. `N` is a positive integer literal no larger
than the traversal ceiling (65536). An inclusive `from f` starts the walk at or
after the key `f`.

An optional second variable `p` binds a per-iteration address pin: a
[`place`](durable-places.md#named-places) over the entry at the current key,
scoped to the loop body. It reads nothing on its own and establishes no presence
fact — the frozen key names an entry an earlier iteration may already have erased —
so a read or write through `p` is an ordinary durable access whose presence must be
established the usual way (an `if exists(p)` test dominates a present-entry write).

The traversed layer must be keyed by a single column: the loop binds one immediate key
and takes one inclusive `from`. A `for` head over a composite-keyed root or branch layer
is the typed `check.unsupported` rejection, since the language spells no composite-key
iteration.

This shapes the data model: model each level you iterate as its own
single-column keyed branch, nesting one branch per level, so every level is
traversable. Reserve a composite key for a tuple that is one indivisible identity
— one whose components are never enumerated on their own — since a composite-keyed
layer is addressed by a full key but is not iterated.

```mw
module docs::traversal

resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

pub fn sumFirstIds(): int {
    var sum = 0
    for id in ^books at most 100 {
        sum += id
    } on more {
        sum = -1
    }
    return sum
}

pub fn sumNoteKeys(id: int): int {
    var sum = 0
    for pos in ^books[id].notes at most 100 from 1 {
        sum += pos
    } on more {
        sum = -1
    }
    return sum
}
```

The traversal freezes the first `N` immediate keys — acquiring at most one key
beyond them to decide the `on more` arm — and then runs the body once per frozen
key in order. The `on more` block runs exactly when an `(N + 1)`th key existed
beyond the frozen set **and** every frozen body completed normally. A `break`,
`return`, or fault out of a body leaves the loop without running `on more`.

### Frozen-Set Isolation

The frozen key set is captured before any loop body runs, so writes a body
performs to the traversed family — creating, erasing, or replacing entries,
including through a called helper — cannot change which keys the loop visits or
the `on more` decision. Traversal is freeze-then-run, not interleaved. Because a
key is frozen rather than re-proven present, the loop variable names a key whose
entry an earlier body iteration may already have erased: a read of that entry
inside the body is an ordinary read that may be absent, not a guaranteed-present
access.

### Bounded Work

The walk costs work proportional to `N` regardless of how many descendants sit
beneath skipped siblings: a child that carries branch descendants but no payload
of its own is passed in one seek, and its own fan-out is never read. The frozen
keys materialize as one ordinary `List<K>` value and are therefore subject to the
same aggregate-byte ceiling every collection obeys; a traversal over wide keys can
reach that ceiling — a `run.collection_limit` fault — at fewer than `N` keys.
There is one collection ceiling, not a separate traversal-specific one.

## Ranges

A `for` head over an integer range binds one name to each integer the range
covers, in ascending order. `..` marks an excluded end and `..=` an included
end; both endpoints are `int` expressions, evaluated once. An optional `by step`
advances the counter by a positive integer literal each iteration, defaulting to
`1`. A range takes no `at most N` bound — its length is determined by its
endpoints.

```mw
module docs::ranges

pub fn sumTo(n: int): int {
    var s = 0
    for i in 1..=n {
        s += i
    }
    return s
}

pub fn evens(): int {
    var count = 0
    for value in 0..10 by 2 {
        count += 1
    }
    return count
}
```

A dead or empty range runs the body zero times: `for i in 5..3` and
`for i in 5..=4` never enter. The step must be a positive integer literal; `by 0`,
a negative step, and a computed step are refused at compile time. A range that
reaches the integer domain boundary ends the loop rather than faulting. Range
iteration is over integers only; a temporal range is not current behavior.

## Local Collections

A `for` head over a local `List` or `Map` walks it positionally. A list yields
its elements in insertion order under one variable; a map yields its keys under
one variable, or its keys and values under two, in
[`CollectionKeyOrder`](types-and-values.md#lists-and-maps). A local collection
takes no `at most` bound — its length is already finite and known.

## Index Declarations

A keyed `store` root may declare narrow compiler-maintained ordered indexes. An
index is an ordered projection of the root's identity keys and top-level fields;
it stores no data of its own and has no application write operation. A non-unique
index distinguishes each row by ending with the complete store identity suffix; a
`unique` index may omit it.

```mw
module docs::indexes

resource Book {
    required title: string
    shelf: string
    isbn: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

pub fn label(): string {
    return "books"
}
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
application code.

## Reading An Index

A source program reads a managed index through the store root that declares it,
`^root.indexName`. The read shape follows the index kind.

A **non-unique** index is scanned with a bounded `for` head, holding its leading
field components as a bracket prefix and binding the source-root identity `Id(^root)`
of each matching entry:

```mw
module docs::index_scan

resource Book {
    required title: string
    required shelf: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
}

pub fn countOnShelf(shelf: string): int {
    var count = 0
    for bookId in ^books.byShelf[shelf] at most 100 {
        if const b = ^books[bookId] {
            count += 1
        }
    } on more {
        count = -1
    }
    return count
}
```

The scan is bounded exactly like a durable traversal (`at most N`, a mandatory
`on more`, freeze-then-run over the frozen identities). It holds every leading field
component of the projection and yields the trailing identity component, so the loop
variable is the `Id(^root)` of each entry — dereference it with `^root[id]` to read the
entry. The store root's identity is a single key column, so the yielded component is a
whole identity; a composite-identity root, a `from` cursor, and a per-iteration address
pin are not admitted on a scan.

A **unique** index is read with a complete-key bracket access `^root.indexName[keys]`,
supplying the whole projection and yielding the optional matching identity — present
with exactly the one `Id(^root)`, or absent, never a sibling. An `if const` head binds
the present identity:

```mw
module docs::index_lookup

resource Book {
    required title: string
    required isbn: string
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
}

pub fn titleByIsbn(isbn: string): string? {
    if const found = ^books.byIsbn[isbn] {
        return ^books[found].title
    }
    return absent
}
```

A non-unique index read is a progressive typed-prefix refinement (an incomplete prefix
yields the next distinct component; the complete projection yields the source-root key),
and a `unique` index read is a complete-key exact lookup that yields exactly the one
matching `Id(^root)` or absent — never a sibling.

The presence of a matching entry is asked directly with `exists(^root.indexName[keys])`,
supplying the whole projection and yielding a `bool` without binding the identity — the
presence half of the lookup:

```mw
module docs::index_exists

resource Book {
    required title: string
    required isbn: string
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
}

pub fn isbnTaken(isbn: string): bool {
    return exists(^books.byIsbn[isbn])
}
```

A non-unique index is scan-only and has no `exists` probe (see [Built-ins](builtins.md#presence)).

A bound `Id(^root)` addresses the whole entry: dereferencing it with `^root[id]` reads
the entry, and inside a transaction the same address is written or replaced — for
example `^root[found] = Resource(...)` updates the entry the lookup found — exactly as an
inline key address is.

**Future.** A scan that binds an intermediate distinct component (rather than the
trailing identity), a composite-identity scan, and a `from` cursor on a scan are not yet
spelled; a source read of those shapes reports a precise not-yet-supported diagnostic.
