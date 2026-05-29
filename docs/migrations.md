# Schema Changes And Migrations

Marrow schemas evolve through source changes plus explicit migration code.
Marrow never guesses a data migration: when a change moves data, populates a new
field, or rebuilds an index, that work is ordinary `.mw` code, run by a tool and
inspectable afterward. There is no migration DSL and no hidden migration ledger
in the database.

The saved-data model these changes operate on is defined in
[`language/resources-and-storage.md`](language/resources-and-storage.md), and
[Data Modeling](data-modeling.md) covers how to shape it. This page covers
changing a schema that already has saved data: adding sparse vs required
elements, tracking renames with `@id`, populating required fields, backfilling
indexes, the `--maintenance` capability, repair, and restore policy.

## What A Change Requires

A schema change is a source edit to a `resource` declaration. Some edits are
safe on their own; others leave existing saved data that the new schema does not
fully describe until a migration runs.

| Change | What it needs |
|---|---|
| Add a sparse (non-`required`) element | Source change only. Existing records stay valid; the element reads as absent until written. |
| Add a `required` element | A migration that populates the field on existing records before code depends on it. |
| Rename an element | Source rename, plus a migration if saved data must move. A stable `@id` lets tooling track the element across the rename. |
| Add an index | Backfill: rewrite the indexed records so the generated index tree is populated. |
| Remove an element | The data under it stays until a migration (or maintenance delete) removes it. |
| Delete a whole root, drop a required field, write raw segments | Maintenance work. Run with `--maintenance`. |

## Adding Sparse vs Required Elements

A sparse element is a source change. Add the field and ship it; existing records
remain valid. An unpopulated sparse element is absent, not zero or empty — read
it with `get(path, default)` or guard it with `exists(path)`.

```mw
resource Book at ^books(id: int)
    required title: string
    subtitle: string            ;; new sparse field — no migration needed
```

A `required` element is different. Existing records were written without it, so
they do not satisfy the new schema until you populate the field.

```mw
resource Book at ^books(id: int)
    required title: string
    required pages: int         ;; new required field — existing records lack it
```

What the current build actually does with an under-populated record:

- A whole-resource read (`var b: Book = ^books(id)`) materializes the fields that
  are present and does not, today, raise on the missing required field.
- A direct read of the absent field raises `run.absent_element`.
- `marrow data integrity` does not report the gap. It verifies that every stored
  value decodes as its declared type (`data.decode`) and flags data under
  unknown members (`data.orphan`); it does not check required-field completeness
  across records.

The practical rule: a new required field has no automatic enforcement sweep, so
populate it before any code reads it directly. Backfill in ordinary code:

```mw
pub fn backfillPages()
    for id in ^books
        ^books(id).pages = 0
```

Run it, then deploy code that depends on the field:

```sh
marrow run --entry app::backfillPages ./project
```

## Stable IDs For Renames

A field's source name is how code spells it. A rename changes that spelling, and
saved data keyed by the old name does not move on its own.

`@id(...)` gives an element a durable identity that survives a source rename, so
rename and migration tooling can recognize `displayTitle` as the same logical
element that used to be `title`:

```mw
resource Book at ^books(id: int)
    ;; Display title shown in search and shelf views.
    @id("book.title")
    displayTitle: string        ;; renamed from `title`, same stable id
```

What `@id` does and does not do today:

- It is source metadata, not a database catalog. It does not change the saved
  path, the field name, the runtime value, or the type.
- The checker enforces that stable IDs are unique within a project. A collision
  is `schema.duplicate_stable_id`.
- It does not move saved data. After a rename, the migration still decides what
  to read, rewrite, or delete. The ID gives tools a durable handle; it does not
  perform the migration.

Use stable dotted text that describes the logical element, not its current
spelling. Add IDs to elements that need durable identity across rename,
migration, generated docs, or external tooling; leave them off short-lived
private shapes.

Identity keys in the `at ^root(...)` clause are not ordinary fields and do not
take `@id`. Changing an identity key means "this is a different record": you
create a new record under the new key and delete or migrate the old one.

## Backfilling And Rebuilding Indexes

Adding an `index` to a resource that already holds data leaves the index tree
empty. Index entries are populated paths; they exist only once the indexed
values are written through a managed write.

```mw
resource Book at ^books(id: int)
    title: string
    shelf: string

    index byShelf(shelf, id)    ;; new index — empty for existing records
```

Right after adding the index, a lookup finds nothing, and `marrow data dump`
shows no `^books.byShelf(...)` entries. To populate it, rewrite the indexed field
on each record. A managed write validates the value, writes the field, and
updates the generated index entry as one coherent step:

```mw
pub fn rebuildByShelf()
    for id in ^books
        ^books(id).shelf = ^books(id).shelf
```

```sh
marrow run --entry app::rebuildByShelf ./project
```

After the backfill, `^books.byShelf("fiction")` resolves and `marrow data dump`
shows the generated entries (a non-unique index stores a marker value at the
identity lookup path). This is ordinary application code — index backfill does
not need maintenance mode, because each write goes through the managed resource.

Indexes describe current resource lookup paths. They do not automatically index
history entries; if historical data needs its own lookup path, model it as a
resource or write an explicit backfill.

## Maintenance Mode

Ordinary `marrow run` protects managed roots. Three operations are rejected
outside maintenance because they can corrupt managed indexes, history layers, or
typed fields:

| Operation | Code without `--maintenance` |
|---|---|
| Delete a whole managed root (`delete ^books`) | `write.requires_maintenance` |
| Delete a `required` field (`delete ^books(id).title`) | `write.required_field` |
| Write or read a raw quoted segment (`^books(id)."oldTitle" = …`) | `write.raw_requires_maintenance` |

`marrow run --maintenance` grants the maintenance capability for that run, and
those operations succeed:

```sh
# Rejected: write.requires_maintenance
marrow run --entry app::dropBooks ./project

# Performs the whole-root delete (records and every generated index entry)
marrow run --maintenance --entry app::dropBooks ./project
```

The flag is an explicit escape hatch. An operator must type it; the default run
and a project's `run.defaultEntry` can never inject it. Use it deliberately for
migration, repair, and restore tooling.

What `--maintenance` permits, precisely:

- Whole-root delete. `delete ^books` removes every record under the root and
  every generated index branch in one subtree delete. Deleting a single identity
  (`delete ^books(id)`) is ordinary work and needs no maintenance.
- Required-field delete. `delete ^books(id).title` removes a required field in
  place. Outside maintenance this is rejected; delete the whole record instead.
- Raw quoted-segment access. `^books(id)."oldTitle"` reads and writes a literal
  backend segment, bypassing the schema's declared fields and index maintenance.
  This is the door to raw data the schema does not model — legacy fields from
  before a rename, import staging, repair. A raw write takes a string (raw
  segments are an untyped text boundary; a non-string scalar is a `run.type`
  error). A raw read of an absent segment is a catchable absent-element error.

What it does not loosen:

- An unquoted undeclared field (`^books(id).nope = …`) stays
  `write.unknown_field` even under maintenance. Maintenance grants raw (quoted)
  access only; an unquoted name is still treated as a typo against the schema.

A migration that drops a root and rebuilds it, or repairs raw bytes, is a normal
`fn` run in maintenance mode. There is no separate migration runner.

## Repair

Repair is migration work over data that no longer matches the schema — bytes a
typed read would reject, or raw fields left by an earlier shape. It uses the same
tools:

- `marrow data integrity ./project` reports what is wrong: `data.decode` for a
  stored value that is not a canonical form of its declared type, `data.orphan`
  for data under an unknown root or naming an undeclared member, and
  `store.corrupt_path` for an undecodable key. It exits `1` when it finds a
  problem and `0` when the tree verifies clean. It is read-only and never
  modifies the store.
- `marrow data dump ./project` and `marrow data get ./project <path>` show the
  raw stored paths and values, including index entries, so you can see exactly
  what needs fixing.
- A repair `fn` run with `--maintenance` rewrites or deletes the offending data —
  via raw quoted segments for data the schema does not model, or ordinary managed
  writes once the values are valid again.

There is no dedicated `marrow repair` command. Repair is a maintenance run of
your own migration code, verified before and after with `marrow data integrity`.

## Restore Policy

`marrow backup ./project <archive>` writes the store's whole saved tree to a
portable archive — the canonical ordered (path, value) stream behind a small
manifest. Generated index entries are included as saved paths.

```sh
marrow backup ./project books.mwbackup
# backed up 6 records to books.mwbackup
```

`marrow restore ./project <archive>` replays an archive into an empty store, in
one transaction (the target gains the whole archive or is left unchanged).
Restore reproduces the saved paths exactly, including the index entries that were
in the archive.

```sh
marrow restore ./project books.mwbackup
# restored 6 records from books.mwbackup
```

Empty-target restore is the only restore mode implemented today. Restoring into a
store that already holds data fails:

```sh
marrow restore ./project books.mwbackup
# restore.not_empty: restore target already holds data; restore writes into an empty store
```

Replace, merge, and repair restores — the non-empty cases — are deferred. When
they land they will be explicit maintenance actions routed through the
maintenance capability, not a relaxation of the empty-target guard.

To restore over existing data today, empty the target first with a maintenance
run (for example a `fn` that deletes the relevant roots, run with
`--maintenance`), then restore into the now-empty store.

## Also Deferred

These do not exist yet; do not plan migrations around them:

- `marrow data diff` and `marrow data load`. They overlap restore's
  replace/merge/repair modes and need typed source fingerprinting. They will
  route through the maintenance capability when implemented and will not loosen
  the read-only guarantee of the `marrow data` inspection group. Today
  `marrow data` provides only the read-only subcommands `roots`, `stats`, `dump`,
  `integrity`, and `get`.
- Replace / merge / repair restore (see above).

## Exit Codes

CLI commands follow the standard contract from
[`error-codes.md`](error-codes.md): `0` on success, `1` for a recoverable check,
capability, runtime, storage, or tool failure (including `restore.not_empty` and
a non-clean `data integrity`), and `2` for a command-line usage error before the
command runs.
