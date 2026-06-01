# Data Evolution And Maintenance

Marrow schemas evolve through source changes plus explicit data-evolution code.
Marrow does not currently run an automatic data compiler over saved stores: when
a change moves data, populates a new required field, repairs raw bytes, or
rebuilds an index, that work is ordinary `.mw` code or a Marrow tool workflow
that an operator can inspect.

The saved-data model these changes operate on is defined in
[`language/resources-and-storage.md`](language/resources-and-storage.md), and
[Data Modeling](data-modeling.md) covers how to shape it. Future project
compilation ideas live in [future/data-evolution.md](future/data-evolution.md).

## What A Change Requires

A schema change is a source edit to a `resource` declaration. Some edits are
safe on their own; others leave existing saved data that the new schema does not
fully describe until explicit data-evolution work runs.

| Change | What it needs today |
|---|---|
| Add a sparse element | Source change only. Existing records stay valid; the element reads as absent until written. |
| Add a `required` element | Code or tooling that populates existing records before code depends on the field. |
| Rename an element | Source rename, plus explicit data movement if saved data must move. |
| Add an index | Backfill/rebuild: rewrite indexed records so the generated index tree is populated. |
| Remove an element | The data under it stays until code or maintenance work removes it. |
| Delete a whole root, drop a required field, write raw segments | Maintenance work. Run with `--maintenance`. |

## Sparse And Required Elements

A sparse element is a source change. Add the field and ship it; existing records
remain valid. An unpopulated sparse element is absent, not zero or empty. Read it
with `path ?? default` or guard it with `exists(path)`.

```mw
resource Book at ^books(id: int)
    required title: string
    subtitle: string
```

A `required` element is different. Existing records were written without it, so
populate it before code reads it directly.

```mw
resource Book at ^books(id: int)
    required title: string
    required pages: int
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

A field's source name is how code spells it. A rename changes that spelling, and
saved data keyed by the old name does not move on its own.

For v0.1, rename work is explicit maintenance code or a tool workflow that names
the old and new saved paths:

```mw
pub fn renameTitle()
    for id in keys(^books)
        ^books(id).displayTitle = ^books(id).title
        delete ^books(id).title
```

Source stable-id annotations are not a production rename contract or v0.1
syntax. Use explicit maintenance code until catalog-owned identity lands.

## Index Rebuilds

Adding an `index` to a resource that already holds data leaves the index tree
empty until indexed values are written through managed writes.

```mw
resource Book at ^books(id: int)
    title: string
    shelf: string

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

Ordinary `marrow run` protects managed roots. Three operations are rejected
outside maintenance because they can corrupt managed indexes, history layers, or
typed fields:

| Operation | Code without `--maintenance` |
|---|---|
| Delete a whole managed root (`delete ^books`) | `write.requires_maintenance` |
| Delete a `required` field (`delete ^books(id).title`) | `write.required_field` |
| Write or read a raw quoted segment (`^books(id)."oldTitle" = ...`) | `write.raw_requires_maintenance` |

`marrow run --maintenance` grants the maintenance capability for that run. The
flag is an explicit escape hatch; the default run and `run.defaultEntry` cannot
inject it.

Maintenance permits whole-root deletes, required-field deletes, and raw
quoted-segment access. It does not make unquoted undeclared fields valid, and it
does not loosen type checks on managed writes.

## Repair

Repair handles data that no longer matches the schema: undecodable values, raw
fields left by an earlier shape, corrupt paths, and orphaned paths. It uses the
same tools:

- `marrow data integrity ./project` reports `data.decode`, `data.orphan`, and
  `store.corrupt_path` problems. It is read-only.
- `marrow data dump ./project` and `marrow data get ./project <path>` show the
  raw stored paths and values.
- A repair function run with `--maintenance` rewrites or deletes the offending
  data.

There is no dedicated `marrow repair` command. Repair is a maintenance run of
your own code, verified before and after with `marrow data integrity`.

## Backup And Restore

`marrow backup ./project <archive>` writes the store's saved tree to a portable
archive: the canonical ordered path/value stream behind a small manifest.
Generated index entries are included as saved paths.

`marrow restore ./project <archive>` replays an archive into an empty store in
one transaction. Empty-target restore is the only restore mode implemented
today; restoring into a store that already holds data fails with
`restore.not_empty`.

Non-empty restore modes are deferred (see [future/cli.md](future/cli.md)).

## Also Deferred

These do not exist yet:

- automatic source/catalog/data compilation;
- catalog-owned opaque stable identities;
- typed transforms as a dedicated planning surface;
- `marrow data diff` and `marrow data load` (see
  [future/data-tools.md](future/data-tools.md));
- non-empty restore modes.

CLI commands follow the standard contract from
[`error-codes.md`](error-codes.md): `0` on success, `1` for a recoverable check,
capability, runtime, storage, or tool failure, and `2` for a command-line usage
error before the command runs.
