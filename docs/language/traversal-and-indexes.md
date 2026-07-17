# Traversal And Indexes

`for` traverses an integer or temporal range, a local collection, or a durable
root or branch family. Durable traversal is always bounded: it visits at most a
declared number of immediate keys and states, at the traversal head, what to do
when more remain.

## Bounded Durable Traversal

A durable `for` head names a store root or a single-level keyed branch, an
`at most N` bound, an optional inclusive `from` key, and a mandatory `on more`
block dedented like an `else`:

```text
for k in <place> at most N [from f]
    statements
on more
    statements
```

`<place>` is a store root such as `^books` (the root entry family) or a keyed
branch such as `^books(id).notes` (the branch family beneath one fixed parent
entry, at any nesting depth). The single loop variable `k` binds each immediate key
in ascending [typed key order](types-and-values.md#key-types); the value at that key
is read separately inside the body. `N` is a positive integer literal no larger
than the traversal ceiling (65536). An inclusive `from f` starts the walk at or
after the key `f`.

The traversed layer must be keyed by a single column: the loop binds one immediate key
and takes one inclusive `from`. A `for` head over a composite-keyed root or branch layer
is the typed `check.unsupported` rejection, since the language spells no composite-key
iteration.

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
keys materialize as one ordinary `List[K]` value and are therefore subject to the
same aggregate-byte ceiling every collection obeys; a traversal over wide keys can
reach that ceiling — a `run.collection_limit` fault — at fewer than `N` keys.
There is one collection ceiling, not a separate traversal-specific one.

## Ranges

Range loops use `..` for an excluded end and `..=` for an included end:

```mw
module docs::ranges

pub fn countEven(): int {
    var count = 0
    for value in 0..10 by 2 {
        count += 1
    }
    return count
}
```

Endpoints may be `int`, `date`, or `instant`. Integer ranges default to step
`1` and may descend with a negative step. Date ranges default to one calendar
day and accept positive whole-day durations. Instant ranges require an explicit
positive duration. A zero step is rejected. A step pointing away from the end
produces no iterations when its direction is not statically known; a
literal-provable dead range is rejected.

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
application code. A non-unique index read is a progressive typed-prefix refinement
(an incomplete prefix yields the next distinct component; the complete projection
yields the source-root key), and a `unique` index read is a complete-key exact lookup
that yields exactly the one matching `Id(^root)` or absent — never a sibling.

**Future.** Runtime index maintenance and index reads — traversal of a non-unique
index and the exact lookup of a `unique` index — are not yet executable on the beta
line. A source read through an index reports a precise not-yet-supported diagnostic
until the managed-index runtime lands (see `docs/status.md`).
