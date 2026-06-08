# Data Evolution And Maintenance

Marrow schemas evolve through source changes plus source-native evolution
intent. Durable identity is recorded automatically the first time a project with
saved data runs or applies an evolution, data-attached preview proves what saved
data needs, and evolution apply commits the exact preview witness or fails
closed.

The saved-data model these changes operate on is defined in
[`language/resources-and-storage.md`](language/resources-and-storage.md), and
[Data Modeling](data-modeling.md) covers how to shape it. Future project
compilation ideas live in [future/data-evolution.md](future/data-evolution.md).

Apply is shaped as a compiler-owned activation job created from the exact
preview witness. V0.1 executes that job immediately in one transaction, but the
shape is durable: verify the witness, perform catalog/data/index work, stamp the
commit, and emit evidence. Future large rebuilds, backfills, and transforms can
be chunked or resumed as the same job model instead of becoming migration
scripts or hidden database history.

## What A Change Requires

A schema change is a source change to a `resource` or `store` declaration. Some
changes are safe on their own; others leave existing saved data that the new
schema does not fully describe until explicit data-evolution work runs.

| Change | What it needs today |
|---|---|
| Add a sparse field | Existing records stay valid; the field reads as absent until written. It changes the durable shape, so a populated store is re-stamped, but discharging the change mutates no stored record: a `marrow run` auto-applies it (see [Run-Time Auto-Apply](#run-time-auto-apply)), and `marrow evolve apply` discharges it explicitly. |
| Add a `required` field | `evolve default` or checked `evolve transform`, proven by `marrow evolve preview` and applied by `marrow evolve apply`. |
| Rename a field | `evolve rename`, applied with `marrow evolve apply`; the stable identity moves with the rename, and stored cells addressed by that identity remain attached. |
| Change a leaf's type | A populated leaf-type change fails closed: the stored bytes were written under the old type, so the new type cannot read them — even when its decoder would accept the old bytes — and `marrow evolve preview` reports it. A transform may not read the member it replaces, so an in-place reinterpret is not the resolution. Instead add a new field of the new type, populate it with an `evolve transform` computed from the old field, then retire the old field. The check is total over leaf-position members — every plain field and every `map[K, V]` value, required and sparse alike — and detects any change to a member's leaf type: scalar to scalar, scalar to an enum or a reference, or one named type to another. An empty leaf has no data to reinterpret and changes freely. |
| Remove or unselect an enum member | While saved data still selects the member, the change fails closed: a stored value carries the member's stable identity, which the new enum no longer offers. Removing the member, marking it `category`, or giving it children all make it unselectable, and `marrow evolve preview` reports repair-required. Every leaf referencing the enum is scanned, required and sparse alike, so a stored value naming the gone member is caught regardless of the holding field's requiredness. Migrate the affected records to a current member first. Reordering members keeps every identity and mutates no stored record, so it re-stamps the durable shape with no data work: a `marrow run` auto-applies it (see [Run-Time Auto-Apply](#run-time-auto-apply)). Renaming a member is identity-preserving the same way a field rename is: declare it with `evolve rename`, and the member's stable identity moves to the new spelling so stored values addressed by that identity stay valid. A bare source rename with no `evolve rename` intent is read as the old member removed plus a new one added, so a stored value naming the old member then fails closed at preview. |
| Add an index | `marrow evolve preview` proves the rebuild and `marrow evolve apply` publishes index entries atomically. |
| Remove a field | If no stored cells exist for that field, removal is a source/catalog no-op. Stored cells under a field the current source no longer declares are `data.orphan`; populated destructive removal needs `evolve retire` plus approval, or maintenance repair that deletes or moves the data before activation. |
| Change a store's identity key shape | Not supported over saved data. Changing the key arity or any identity key type fails closed: existing records are keyed by the old shape and the new shape cannot address them. Model a new store and migrate with maintenance code. |
| Re-key a keyed layer | Not supported over populated entries. A keyed layer — a keyed-leaf layer (including `sequence`/`map` sugar) or an author-written keyed-group layer — keys its entries by its key shape; changing that layer's key arity or any key type fails closed — existing entries are keyed by the old shape and the new one addresses none of them. A keyed-leaf `map[K, V]` is a leaf position whose token folds in both its key shape and its value type, so a change to its key arity or key type is detected as a leaf type change exactly like a value-type change; a keyed-group layer is detected as a structural divergence. Either way `marrow evolve preview` reports it; model a new layer and migrate with maintenance code. |
| Reshape a group to or from a keyed layer | Not supported over populated data. Turning a plain group into a keyed layer (or the reverse) changes the durable shape its data occupies, so it fails closed: the old data lives under the old shape and the new shape cannot read it. `marrow evolve preview` reports it; model a new member of the new shape and migrate with maintenance code. |
| Delete a whole root or drop a required field | Explicit maintenance/repair code under `--maintenance`, checked before and after. |

The type-change check covers every leaf the same way, by the identity of the type
its stored bytes were accepted under. A `map[K, V]` keyed-leaf layer is a leaf
position whose accepted leaf token folds in both its key shape and its value type
V, so a change to its value type — or to its key arity or key type — changes the
token and is detected and fails closed exactly like a plain-field retype:
`marrow evolve preview` reports it as repair-required and the resolution is the
same add-new, transform, retire path. A value type Marrow cannot reduce to a
single comparable leaf — a `sequence` value (`map[K, sequence[V]]`) or an
`unknown` — still records a stable "untokenizable" marker rather than no token, so
a change into, out of, or between such values reads as a different value and fails
closed the same way; it is never silently reinterpreted. The map's key shape rides
inside the same leaf token as a prefix on the value, so a key-arity or key-type
change is caught as a leaf type change, not through the store/keyed-group key-shape
path.

Beyond these named cases the check is total by construction. Each durable member
records the identity-aware shape its data was accepted under — its kind, its key
shape if it is a keyed layer, and its leaf token if it is a leaf — and a
default-deny backstop fails closed any member whose recorded shape diverges from
current source and which still holds data, even when no named classifier handles
that exact transition. The backstop is total over nesting depth: it reaches a member
nested below any number of keyed layers by descending through each layer's existing
entries, so a divergence deep under unchanged keyed ancestors is judged against the
entries it would orphan, not skipped. So a keyed-layer re-key, a group-to-keyed-layer
reshape, or any structural change v0.1 has no specific migration path for cannot
silently activate over saved data, at any depth; an unpopulated member, having nothing
to orphan, reshapes freely.

Changing a store's identity key shape — its key arity or any identity key type —
is not supported over saved data and fails closed. Existing records are addressed
by the old key bytes, which live in the saved path itself, so a record keyed by an
`int` cannot be found under a `string` key and a single-key record cannot be found
under a composite key. v0.1 has no graceful key migration: `marrow evolve preview`
reports the change as repair-required and `marrow evolve apply` refuses it. To
re-key, model a new store and migrate with maintenance code.

A keyed layer's key shape fails closed the same way over its populated entries.
Changing a layer's key arity or any key type orphans every entry addressed by the
old shape, so `marrow evolve preview` reports it as repair-required and the
resolution is to model a new layer and migrate with maintenance code. The two layer
shapes reach that verdict by different detectors: a keyed-leaf `map[K, V]` is a
leaf, and its key shape rides inside its leaf token as a prefix on the value, so a
key change is caught as a leaf type change; a keyed-group layer is not a leaf, so
its key change is caught as a structural divergence. Reshaping a plain group into a
keyed group, or the reverse, changes the durable shape its data occupies and fails
closed as a structural divergence the same way.

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
preserves identity. A source rename without a matching `evolve rename` intent is
a check error: rename versus delete-and-add is ambiguous without the stated
intent.

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

`active` entries bind current source to stable IDs. `deprecated` entries are old
aliases kept for identity continuity. `reserved` entries remember spellings that
must not be reused after a retire; a later source declaration at the same catalog
path is rejected rather than minted as a fresh identity. `reserved` is the
durable inactive spelling for a retired catalog path.

Source-only checks read this file when present. They propose replacement metadata
when it is missing or stale, but never write it, so `marrow check` stays
read-only and CI-safe; a project whose durable identity is not yet recorded is
reported informationally, not as a failure. Checked facts expose catalog-backed
IDs for resources, stores, store indexes, resource members, enums, and enum
members. Runtime value encoding remains a separate storage concern; the catalog
is the durable schema identity exposed to tools, evolution, and checked facts.

The file is written transparently by the flows that establish durable state —
running the program and `marrow evolve apply`. The first such command on a
project with a durable surface freezes the proposed identity into the file; once
a baseline exists, later identity changes flow through `evolve apply` rather than
being written from a check. There is no separate command to inspect or accept a
proposal.

A stable ID is a random opaque 128-bit value in the `cat_<32 lowercase hex>`
shape. It is allocated independently of the source path, so it never changes
when a path changes, and it is random rather than a counter so identity minted
on two branches for different entities cannot collide when they merge. Once
committed it is frozen and never recomputed. A duplicate ID — from a manual
catalog change, bad branch integration, or an astronomically unlikely clash —
fails closed at check rather than corrupting storage.

The catalog digest is `sha256:<64 lowercase hex>` over the canonical JSON object
`{"epoch": <u64>, "entries": <entries in file order>}` serialized by the
metadata writer. The digest covers stable IDs, paths, aliases, lifecycle states,
and accepted structural tokens, and is recomputed when catalog metadata is read;
an edited file whose digest no longer matches is rejected before its IDs bind.

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

The source digest binds the durable shape — every `resource`, `store`, `enum`,
and module constant — and not the `evolve` block. The fence governs the shape a
stored snapshot must match, not the transition that produced it, so an evolve
block can be deleted once consumed without reading as schema drift.

The source digest also uses `sha256:<64 lowercase hex>`. Its payload is the
compiler's canonical durable-shape summary: module constants, resources,
stores, enum trees, store identity-key shapes, and resource-member structural
tokens after catalog IDs have bound the durable identities those shapes refer
to. It intentionally excludes transition text such as `evolve` blocks.

An evolution apply stamps activation evidence in the same transaction as its
data effects: proposal/evolution digests, changed catalog IDs, default
backfill counts and bounded effect digests, transform counts, exact per-id
retire counts with a bounded evidence digest, and rebuilt-index counts. Receipts
do not store proposal catalog bodies or executable migration steps. The accepted
catalog file publishes only after those effects are verifiable; crash resume
recomputes the current proposal from source plus the accepted catalog, checks
the evidence against the current store effects, and then writes that generated
proposal.

A program with no accepted catalog has no durable activation context, so there is
nothing to fence against. A run records the baseline catalog before it reaches the
store, so a project with a durable surface is fenced against the identity it just
froze; a program that declares no saved data has no baseline and an evolution
apply refuses outright, with no epoch to advance from. An in-memory store carries
no durable context and is never fenced.

This is the v0.1 compatibility window: a binary supports exactly its own accepted
epoch and schema. Old and new binaries outside that exact window fail closed
before writing.

## Run-Time Auto-Apply

When a `marrow run` fences on `run.schema_drift` — the store holds a structurally
different shape at this binary's epoch — the run computes the evolution obligation
against the live committed data and discharges it itself when, and only when, doing
so mutates zero stored records. The digest still binds the full durable shape and
the fence comparison is unchanged; the run is performing the real evolution apply
(new digest, advanced epoch) for the subset where apply stages no data write.

An evolution mutates zero stored records when it is intrinsically additive — a
sparse field add, a new resource, store, enum member, or module constant, or an
enum-member reorder — or when the affected store holds no records. Emptiness is not
a special case: an empty store has nothing to backfill or lose, so any change
against it discharges its obligation. The same source edit therefore auto-applies
against an empty store and fences against a populated one that needs work.

A change that backfills a newly required field, rewrites records through a
transform, or destructively drops populated data does not auto-apply. The run fences
with an actionable diagnostic that names `marrow evolve apply` and the backfill
count where the witness proved one. A destructive drop against populated data stays
explicit and requires confirmation through `marrow evolve apply --maintenance`,
because losing data must never be a silent side effect of a run; a drop whose target
holds no cells has nothing to lose and auto-applies. An evolution that needs that
destructive approval never auto-applies, whatever its record count.

The obligation probe and the activation stamp run in one transaction under the store
write lock. The witness pins the store commit id, and apply re-checks that pin inside
the write transaction, so a write that commits between the probe and the stamp moves
the commit id and fails the apply closed: the auto-apply decision reflects committed
state at stamp time and can only become more conservative under a race, never stamp
against data it no longer describes. A run advancing the epoch fences other bindings
still on the old epoch (`run.store_evolved`), the same lockout an explicit
`marrow evolve apply` produces.

## Long-Term Online Activation Direction

v0.1 activation is intentionally strict and local. That strictness is the
foundation for future online activation, not the final OLTP operating model.

Future server/OLTP Marrow keeps the same source-native authority but stretches
activation over a compiler-owned job:

1. preview emits an exact witness;
2. start records a durable activation job from that witness;
3. background chunks backfill required fields, transforms, indexes, or shadow
   layouts through checked facts;
4. verification proves the affected durable facts;
5. publish advances the readable catalog epoch in a small commit;
6. close drains old runtime generations and deletes bounded adapters.

The job state and final activation receipt are evidence. They are not migration
history and cannot decide schema meaning. Source, accepted catalog, checked
facts, runtime, engine profile, and durable data remain the only semantic model.

Future compatibility windows are explicit and finite. A server may admit an old
compiled client only when its catalog epoch is in the declared window. Old reads
may use a compiler-generated typed adapter. Old writes are rejected unless the
compiler proves the adapter lowers them to latest-format write plans while
maintaining every active and building derived fact. The normal window supports at
most one old epoch; a wider window needs an explicit architecture decision.

Large key-shape, resource-shape, layout, or engine changes do not become raw
store rewrites. They use a shadow-decant workflow when needed: build a new
store/layout in chunks, bridge a bounded set of writes, verify identity/count and
checksum facts, publish a small binding change, then close the window and purge
the old physical state.

## Evolve Block Lifecycle

An `evolve` block is a one-off transition, not durable schema. It states the
intent for a single change — a rename, default, retire, or transform — that
`marrow evolve apply` discharges against saved data. Apply commits the witness
and advances the store; the block has then done its work.

Because the store fences on the durable shape and not on the evolve block, the
program runs the same whether the block is kept or deleted. Deleting a block once
its change has synced to saved data is the expected lifecycle: the recorded
identity and the migrated data carry the result forward, and a stale block left
in source only describes work already done. Keep a block only while its apply is
still pending.

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

Future online index builds use the same visibility law. A building index may be
maintained by writes before it is readable, but production reads may not use it
until the activation job verifies the whole derived tree and publishes the
catalog state that makes it visible.

Activation receipts record the source digest, previous and next catalog facts,
engine profile, affected stable IDs, counts, approvals, and final commit as
evidence. Tooling, backup, restore, and support may render or verify that
evidence, but receipts are not executable migration history.

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

- typed data integrity reports `data.decode`, `data.key_type`, and
  `data.orphan` problems. It is read-only.
- typed data inspection renders durable places from checked/catalog facts.
- A repair function run with `--maintenance` rewrites or deletes modeled data
  through managed paths, then `check --data` or `evolve preview` must prove the
  repaired snapshot before activation.

There is no dedicated `marrow repair` command. Repair is a maintenance run of
your own code, verified before and after with `marrow data integrity`.

## Backup And Restore

Typed backup/restore is a separate command pair (`marrow backup` and
`marrow restore`), not a raw engine-byte copy. A backup carries a manifest binding
the data to the source digest, accepted catalog epoch, engine profile, and
value-codec version it was written under, plus the canonical tree-cell data
stream as typed cell targets; the generated indexes are derived, so the stream
omits them and restore rebuilds them from the data. Restore validates that
binding and the data against the schema, rejects managed cells the current
source/catalog does not declare, and activates only after the one transaction
verifies. Backups are deterministic and portable across conforming backends at
the same layout and codec, but byte identity requires matching accepted catalog
facts, engine profile, value codec, and stored data. Stable IDs are random opaque
values that freeze when accepted, so divergent catalog histories may still
freeze distinct accepted IDs for source that looks equivalent.

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
