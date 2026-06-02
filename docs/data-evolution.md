# Data Evolution And Maintenance

Marrow schemas evolve through source changes plus explicit data-evolution code.
Marrow does not currently run an automatic data compiler over saved stores: when
a change moves modeled data, populates a new required field, repairs schema
violations, or rebuilds an index, that work is ordinary `.mw` code or a Marrow
tool workflow that an operator can inspect.

The saved-data model these changes operate on is defined in
[`language/resources-and-storage.md`](language/resources-and-storage.md), and
[Data Modeling](data-modeling.md) covers how to shape it. Future project
compilation ideas live in [future/data-evolution.md](future/data-evolution.md).

## What A Change Requires

A schema change is a source edit to a `resource` or `store` declaration. Some
edits are safe on their own; others leave existing saved data that the new
schema does not fully describe until explicit data-evolution work runs.

| Change | What it needs today |
|---|---|
| Add a sparse field | Source change only. Existing records stay valid; the field reads as absent until written. |
| Add a `required` field | Code or tooling that populates existing records before code depends on the field. |
| Rename a field | Source rename, plus explicit data movement if saved data must move. |
| Add an index | Backfill/rebuild: rewrite indexed records so the generated index tree is populated. |
| Remove a field | The data under it stays until code or maintenance work removes it. |
| Delete a whole root or drop a required field | Maintenance work. Run with `--maintenance`. |

## Sparse And Required Fields

A sparse field is a source change. Add the field and ship it; existing records
remain valid. An unpopulated sparse field is absent, not zero or empty. Read it
with `path ?? default` or guard it with `exists(path)`.

```mw
resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book
```

A `required` field is different. Existing records were written without it, so
populate it before code reads it directly.

```mw
resource Book
    required title: string
    required pages: int

store ^books(id: int): Book
```

What Marrow does with an under-populated record:

- Adding `required pages` is data-attached evolution: activation runs the
  data-attached check, which proves every stored record has `pages` or reports the
  exact records that lack it (a Default or Transform obligation).
- A required field missing from stored data is a fatal data-attachment/corruption
  error, never a catchable branch.
- A bare (maybe-present) field reads as maybe-present and is resolved at the read
  site; an unresolved read is a compile error.
- `marrow data integrity` verifies stored value encodings and orphaned paths.

Backfill with ordinary code:

```mw
pub fn backfillPages()
    for id in keys(^books)
        ^books(id).pages = 0
```

Run it, then deploy code that depends on the field:

```sh
marrow run --entry app::backfillPages ./project
```

## Renames

A field's source name is how code spells it. Its durable identity is owned by
the accepted catalog metadata file, not by source annotations or source order. A
rename changes the spelling; saved data keyed by the old name does not move on
its own.

For v0.1, rename work has two parts:

- the accepted catalog records the new canonical path, the old path as an alias,
  and the same stable ID;
- explicit maintenance code or a tool workflow moves any saved data that must
  move physically.

```mw
pub fn renameTitle()
    for id in keys(^books)
        ^books(id).displayTitle = ^books(id).title
        delete ^books(id).title
```

Source stable-id annotations are not a production rename contract or v0.1
syntax. A source rename without accepted catalog intent is a check error.

## Accepted Catalog Metadata

The accepted catalog file is generated metadata committed in the source tree. Its
path is configured by `acceptedCatalog` in `marrow.json` and defaults to
`marrow.catalog.json`.

Each entry records:

- the declaration kind;
- the canonical catalog path and any old aliases;
- the stable ID;
- lifecycle state;
- the catalog epoch and digest.

Source-only checks read this file when present. They propose replacement metadata
when it is missing or stale, but never write it. Checked facts expose
catalog-backed IDs for resources, stores, store indexes, resource members, enums,
and enum members. Runtime value encoding remains a separate storage concern; the
catalog is the durable schema identity exposed to tools, evolution, and checked
facts.

## Index Rebuilds

Adding an `index` to a store that already holds data leaves the index tree empty
until indexed values are written through managed writes.

```mw
resource Book
    title: string
    shelf: string

store ^books(id: int): Book
    index byShelf(shelf, id)
```

To populate it, rewrite the indexed field on each record. The managed write
validates the value, writes the field, and updates the generated index entry as
one coherent step:

```mw
pub fn rebuildByShelf()
    for id in keys(^books)
        if exists(^books(id).shelf)
            ^books(id).shelf = ^books(id).shelf
```

```sh
marrow run --entry app::rebuildByShelf ./project
```

This does not need maintenance mode because each write goes through the managed
resource path.

## Maintenance Mode

Ordinary `marrow run` protects managed roots. Two operations are rejected
outside maintenance because they can remove large managed subtrees or violate
required-field contracts:

| Operation | Code without `--maintenance` |
|---|---|
| Delete a whole managed root (`delete ^books`) | `write.requires_maintenance` |
| Delete a `required` field (`delete ^books(id).title`) | `write.required_field` |

`marrow run --maintenance` grants the maintenance capability for that run. The
flag is an explicit escape hatch; the default run and `run.defaultEntry` cannot
inject it.

Maintenance permits whole-root deletes and required-field deletes. It does not
make undeclared fields valid, and it does not loosen type checks on managed
writes.

## Repair

Repair handles data that no longer matches the schema. Runtime maintenance code
can rewrite or delete schema-modeled data through managed writes; raw bytes,
corrupt paths, and orphaned paths are reported by data tools until typed repair
surfaces land. It uses the same inspection tools:

- `marrow data integrity ./project` reports `data.decode`, `data.orphan`, and
  `store.corrupt_path` problems. It is read-only.
- `marrow data dump ./project` and `marrow data get ./project <path>` show the
  raw stored paths and values.
- A repair function run with `--maintenance` rewrites or deletes modeled data
  through managed paths.

There is no dedicated `marrow repair` command. Repair is a maintenance run of
your own code, verified before and after with `marrow data integrity`.

## Backup And Restore

Typed backup/restore is deferred until the tree-cell backup manifest lands. The
backup contract must compile source, accepted catalog metadata, typed values,
index cells, sequence state, and engine-profile metadata together instead of
copying a raw saved-path stream.

## Also Deferred

These do not exist yet:

- automatic source/catalog/data activation against attached data;
- typed transforms as a dedicated planning surface;
- `marrow data diff` and `marrow data load` (see
  [future/data-tools.md](future/data-tools.md));
- non-empty restore modes.

CLI commands follow the standard contract from
[`error-codes.md`](error-codes.md): `0` on success, `1` for a recoverable check,
capability, runtime, storage, or tool failure, and `2` for a command-line usage
error before the command runs.
