# Data Evolution And Maintenance

Marrow schemas evolve through source changes plus source-native evolution
intent. Catalog preview/accept records durable identity, data-attached preview
proves what saved data needs, and evolution apply commits the exact preview
witness or fails closed.

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
| Add a `required` field | `evolve default` or checked `evolve transform`, proven by `marrow evolve preview` and applied by `marrow evolve apply`. |
| Rename a field | `evolve rename` plus `catalog accept`; stable identity moves, and stored cells addressed by that identity remain attached. |
| Add an index | `marrow evolve preview` proves the rebuild and `marrow evolve apply` publishes index entries atomically. |
| Remove a field | A sparse drop is deprecated when nothing reads it; populated destructive removal needs `evolve retire` plus approval. |
| Delete a whole root or drop a required field | Explicit maintenance/repair code under `--maintenance`, checked before and after. |

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

Backfill with source-native intent:

```mw
evolve
    default Book.pages = 0
```

Preview and apply the exact witness:

```sh
marrow evolve preview ./project
marrow evolve apply ./project
```

## Renames

A field's source name is how code spells it. Its durable identity is owned by the
accepted catalog metadata file, not by source annotations, source order, or a
best-effort source diff. A rename is an explicit catalog decision:

```mw
evolve
    rename Book.title -> Book.displayTitle
```

The accepted catalog records the new canonical path, the old path as an alias,
and the same stable ID. Stored cells addressed by that stable member ID remain
attached to the renamed field; no best-effort name matching or migration script
preserves identity. A source rename without accepted catalog intent is a check
error.

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

Use `marrow catalog preview` to inspect a proposed file and `marrow catalog
accept` to write exactly the current proposal. `accept` re-checks the project
after the write.

## Activation Fencing

A store records the catalog epoch, engine profile, and analyzed-source digest
each commit stamped. A compiled program is pinned to exactly the catalog epoch it
accepted and the schema shape that digest covers. Before a write-capable open — a
`marrow run` over a persistent store, or an evolution apply — the binary fences
itself against the store's stamp, so a binary cannot write a stale shape over a
store another binary has moved past, and data shaped for one schema cannot be run
against a different one.

| Store state | Outcome |
|---|---|
| Empty (no saved records, no stamp) | Adopted: the run or apply proceeds and the first commit stamps the program's epoch, profile, and digest. |
| Populated but unstamped (saved records, no activation stamp) | `run.store_unstamped`: run `marrow check --data` and `marrow evolve apply` to activate the accepted catalog first. |
| Stamped epoch equals the program's, and the source digest matches | Proceeds. An apply advances the store to the proposal epoch; a run executes normally. |
| Stamped epoch equals the program's, but the source digest differs | `run.schema_drift`: the store was stamped under a structurally different schema at this epoch. |
| Stamped epoch newer than the program | `run.store_evolved`: a newer binary evolved the store. Recompile or upgrade against the current accepted catalog. |
| Stamped epoch older than the program | `run.store_behind`: the store predates this catalog. Activate it to the program's epoch with an evolution apply first. |
| Engine profile differs from the binary's layout | `run.engine_profile`: the physical storage layout has drifted. |

The catalog epoch is a coarse version number; two structurally different schemas
can share an epoch, so the source digest is the schema-bearing fence that tells
them apart. A store stamped before digest fencing carries no recorded digest and
is adopted by the epoch match alone.

A program with no accepted catalog has no durable activation context: a run's
durable code is gated by the catalog-acceptance check rather than fenced here, and
an evolution apply refuses outright because it has no baseline epoch to advance
from. An in-memory store carries no durable context and is never fenced.

This is the v0.1 compatibility window: a binary supports exactly its own accepted
epoch and schema. Old and new binaries outside that exact window fail closed
before writing.

## Index Rebuilds

Adding an `index` to a store that already holds data creates a rebuild
obligation.

```mw
resource Book
    title: string
    shelf: string

store ^books(id: int): Book
    index byShelf(shelf, id)
```

```sh
marrow evolve preview ./project
marrow evolve apply ./project
```

Apply rebuilds the index entries from the checked store facts and stamps the same
transaction. A failed rebuild publishes no partial index data.

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

Repair handles checked data that no longer matches the schema and cannot be
discharged by rename/default/transform/rebuild/retire. A repair-required witness
blocks `check --data`, `evolve preview`, and `evolve apply`.

- typed data integrity reports `data.decode` and `data.key_type` problems. It is
  read-only.
- typed data inspection renders durable places from checked/catalog facts.
- A repair function run with `--maintenance` rewrites or deletes modeled data
  through managed paths, then `check --data` or `evolve preview` must prove the
  repaired snapshot before activation.

There is no dedicated `marrow repair` command. Repair is a maintenance run of
your own code, verified before and after with `marrow data integrity`.

## Backup And Restore

Typed backup/restore is deferred until the tree-cell backup manifest lands. The
backup contract must compile source, accepted catalog metadata, typed values,
index cells, sequence state, and engine-profile metadata together instead of
copying raw engine bytes.

## Also Deferred

These do not exist yet:

- multi-record transforms (split, merge, or a target computed from more than one
  record). The narrow per-record `evolve transform` — a pure body computing one
  saved member from a record's other, still-decodable members — is implemented; a
  reshape that crosses records is not;
- `marrow data diff` and `marrow data load` (see
  [future/data-tools.md](future/data-tools.md));
- non-empty restore modes.

CLI commands follow the standard contract from
[`error-codes.md`](error-codes.md): `0` on success, `1` for a recoverable check,
capability, runtime, storage, or tool failure, and `2` for a command-line usage
error before the command runs.
