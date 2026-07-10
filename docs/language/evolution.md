# Evolution

An `evolve` declaration states how a current source change relates to durable
data already accepted by a store. It is checked with the program but runs only
through the evolution workflow.

## Forms

One block may contain rename, default, retire, and transform steps:

```text
evolve
    rename Book.title -> Book.heading
    default Book.language = "und"
    retire Book.legacyCode
    transform Book.searchTitle
        return std::text::toLower(old.heading)
```

The source on the right side of a rename and every default or transform target
must exist in the current program. A retire names a declaration removed from
current source but still known to the accepted durable state.

## Rename

```text
rename Book.title -> Book.heading
rename Status::draft -> Status::proposed
rename ^books.byTitle -> ^books.byHeading
```

Rename carries the same durable declaration forward under a new source
spelling. It may target resources, members, enum members, stores, and indexes
where their shapes remain compatible. A bare source rename without this intent
is treated as removal plus addition and is not assumed to preserve populated
data.

## Default

A default fills a missing required scalar member in existing entries, including
a sparse member changed to `required`. A newly introduced sparse member needs no
backfill and remains absent:

```mw
module docs::evolution_default

resource Book
    required title: string
    required language: string

store ^books(id: int): Book

evolve
    default Book.language = "und"
```

The value may be a `bool`, `int`, `decimal`, or string literal; a negated numeric
literal; or `date("...")`, `instant("...")`, `duration("...")`, or `bytes("...")`
over one string literal. Bare duration and bytes literals are not accepted here.
Enums, identities, resources, sequences, other calls, durable reads, arithmetic
computations, and per-record expressions require another approach. Use a
transform when records need different values.

## Transform

A transform computes one top-level resource member for every existing entry:

```mw
module docs::evolution_transform

resource Book
    required title: string
    searchTitle: string

store ^books(id: int): Book

evolve
    transform Book.searchTitle
        return std::text::toLower(old.title)
```

`old` is a read-only resource value for the entry before this evolution.
The body must return the target member type on every path.

Transform bodies are pure. They may use operators, constants, and pure
standard-library calls. They may not:

- read or write a `^` path;
- call a project function;
- perform host operations;
- open a transaction;
- read `old.<target>`;
- read a member changed by another default or transform in the same block.

The target must be a top-level member of a saved resource. Nested group members
and members inside keyed child entries are not transform targets in the current
implementation.

To reinterpret a member using its own old value, add a new member, transform
the new member from the old one, then retire the old member.

## Retire

```text
retire Book.legacyCode
retire ^books.byLegacyCode
```

Retire authorizes destructive removal of the named durable declaration and any
data it owns. Removing populated data from source without a matching retire is
rejected. A retired identity is not assigned to an unrelated later
declaration.

## Checking And Application

Preview compares the checked source with a read-only view of the current store.
It reports whether the change requires no data rewrite, needs a default or
transform, is destructive, or is rejected. Preview does not mutate the store.

Apply uses the exact checked preview result. Defaults, transforms, destructive
removals, the new accepted declaration state, and commit metadata are written
atomically. A transform fault or detected change since preview aborts the
operation without a partial result.

Normal program execution is refused while a data-changing evolution is
pending. A destructive apply requires an explicit backup path or an explicit
choice to proceed without one. Restore brings back both data and the accepted
declaration state needed to check the restored program.
