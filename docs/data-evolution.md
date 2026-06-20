# Data Evolution And Maintenance

Marrow schemas evolve through source changes plus source-native evolution
intent. Durable identity is recorded automatically the first time a project with
saved data runs or applies an evolution, data-attached preview proves what saved
data needs, and evolution apply commits the exact preview witness or fails
closed.

The saved-data model these changes operate on is defined in
[`language/resources-and-storage.md`](language/resources-and-storage.md), and
[Data Modeling](data-modeling.md) covers how to shape it.

Apply is shaped as a compiler-owned activation job created from the exact
preview witness. V0.1 executes that job immediately in one transaction, but the
shape is durable: verify the witness, perform catalog/data/index work, stamp the
commit, and return an operator receipt. Future large rebuilds, backfills, and transforms can
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
| Rename a field | `evolve rename`, applied with `marrow evolve apply`; the stable identity moves with the rename, and stored cells addressed by that identity remain attached. A bare source rename over populated data still fails closed, but when exactly one populated dropped field and one same-resource added field share a durable leaf shape, the repair guidance points at `evolve rename` before destructive retirement. |
| Change a leaf's type | A populated leaf-type change fails closed; `marrow evolve preview` reports it. Add a new field of the new type, populate it with an `evolve transform` from the old field, then retire the old field. An empty leaf changes freely. |
| Remove or unselect an enum member | Fails closed while saved data still selects the member (removal, marking it `category`, and giving it children all unselect it); migrate affected records to a current member first. Reordering members preserves every identity, mutates nothing, and auto-applies. Rename a member with `evolve rename`; a bare source rename reads as remove-plus-add and fails closed. |
| Add an index | `marrow evolve preview` proves the rebuild and `marrow evolve apply` publishes index entries atomically. |
| Remove a field, resource, or store | If no stored cells exist, removal is a free source/catalog no-op. A populated destructive removal fails closed: stored cells under a field — or under a whole resource or store — the current source no longer declares would be orphaned, so `marrow evolve preview`, `marrow evolve apply`, and a plain `marrow run` all report repair-required and refuse to activate. Resolve it with `evolve retire` applied under `--maintenance --approve-retire`, or maintenance repair that deletes or moves the data before activation. |
| Change a store's identity key shape | Not supported over saved data; any change to the key arity or a key type fails closed. Model a new store and migrate with maintenance code. |
| Re-key a keyed layer | Not supported over populated entries; any change to the layer's key arity or a key type fails closed. Model a new layer and migrate with maintenance code. |
| Reshape a group to or from a keyed layer | Not supported over populated data; the reshape fails closed. Model a new member of the new shape and migrate with maintenance code. |
| Delete a whole root or drop a required field | Explicit maintenance/repair code under `--maintenance`, checked before and after. |

The type-change check is total over leaf positions, required and sparse alike,
and compares the identity of the type the stored bytes were accepted under, so
every retype is caught: scalar to scalar, scalar to an enum or a reference, or
one named type to another — even when the new type's decoder would accept the
old bytes. A keyed-leaf layer's accepted token folds in both its key shape and
its value type, so a change to its value type, key arity, or key type is
detected the same way. A value Marrow cannot reduce to a single comparable leaf
— a `sequence` value or an `unknown` — records a stable untokenizable marker, so
a change into, out of, or between such values also fails closed rather than
being silently reinterpreted. A transform may not read the member it replaces,
so an in-place reinterpret is never the resolution.

Key and group shapes fail closed the same way. A store's identity keys live in
the saved path itself, so changing the key arity or any key type leaves every
existing record unaddressable — v0.1 has no graceful key migration. A
keyed-group layer's key change, and a reshape between a plain group and a keyed
layer, are caught as structural divergences; a keyed leaf's key change is
caught through its leaf token. Beyond the named cases, a default-deny backstop
fails closed any member whose recorded identity-aware shape diverges from
current source while it still holds data, descending through existing keyed
entries to any nesting depth, so no structural change v0.1 lacks a migration
path for can silently activate over saved data. An unpopulated member, having
nothing to orphan, reshapes freely.

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
- `marrow data integrity` verifies stored value encodings, identity referents,
  required fields, and orphaned paths.

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

A field's source name is how code spells it. Its saved-data identity is owned by
the live store, not by source annotations, source order, or a best-effort source
diff. A rename is an explicit decision:

```mw
evolve
    rename Book.title -> Book.displayTitle
```

The accepted identity records the new canonical path, the old path as an alias,
and the same stable ID. Stored cells addressed by that stable member ID remain
attached to the renamed field; no best-effort name matching or migration script
preserves identity. A source rename without a matching `evolve rename` intent
fails closed: rename versus delete-and-add is ambiguous without the stated
intent. When the source diff has exactly one populated dropped field and one
same-resource added field with the same durable leaf shape, check-time repair
guidance names `evolve rename` first; otherwise it points at the destructive
retire path.

## Saved-Data Identity And `marrow.lock`

The live store is the sole authority for accepted saved-data identity. The store
records, in the same transaction that writes the data, the accepted identity of
every durable entity: a stable ID, lifecycle state, canonical path with any old
aliases, and the accepted shape its data was written under. `active` entries bind
current source to stable IDs; `reserved` entries remember retired spellings that
must not be reused, so a later source declaration at the same path is rejected
rather than minted as a fresh identity. Renames keep identity continuity by
recording old spellings as aliases.

`marrow.lock` is a generated, committed, never-hand-edited projection of that
accepted identity, kept in the project root and tracked in source control like
`Cargo.lock`. Each entry projects a stable ID, lifecycle, canonical path, and a
shape fingerprint; the lock also carries the append-only ledger of retired and
reserved IDs and the producing source shape. Each ledger tombstone records the
retired entity's `(kind, path)` alongside its ID, so the lock fully represents a
reserved path: a fresh checkout that re-seeds a lost store from the lock alone
reconstructs the reserved entries and still rejects re-declaring a retired path.
It is not Marrow language data — there is no `^catalog` root, resource,
standard-library, or data-CLI surface that can read, scan, or mutate it.

The lock is always subordinate to a valid live store. It does two things, and only
these two:

- It **seeds** a fresh empty store. When the store is empty and a committed
  `marrow.lock` exists, the first write adopts the committed identity from the lock
  instead of minting fresh, so a fresh checkout reproduces the committed identity
  exactly. Adoption fails closed: a corrupt lock refuses the command
  (`catalog.lock_corrupt`) rather than minting around it, and an adoption that
  would reissue a retired ID or regress the epoch is rejected. Fresh identity is
  minted only when no lock exists.
- It **reports staleness**. When the lock's recorded source shape is behind the
  current source, `marrow check` reports a non-fatal `check.stale_lock` advisory
  and still passes, since a later `run` or `evolve apply` regenerates the lock.
  `marrow check --locked` treats a stale lock as a failure, giving CI the
  enforcement a lockfile convention expects.

The lock can never override or repair a valid live store. There is no path that
rewrites the store from the file; the projection runs one way, store to lock.

### Branch And Team Workflow

Merged source is truth. Development stores are disposable. Production identities
flow only through deployed code and `marrow evolve apply`, never through a
developer's local `.data` directory.

Treat a `marrow.lock` merge conflict like any generated lockfile: do not
hand-merge it. Resolve the source conflict, then run the program or
`marrow evolve apply` to regenerate `marrow.lock` from the live store, and commit
the regenerated file. A local store from the losing branch is older than the
merged identity and is fenced by activation, typically as `run.store_behind`,
until `marrow evolve apply` activates or replaces that local store. Do not copy a
development store forward to rescue a merge; replay the accepted source path.

Accepted identity advances only through the flows that establish durable state —
running the program and `marrow evolve apply` — each writing the accepted identity
in the same store transaction as the data and metadata they commit. The first such
command on a project with a durable surface freezes the proposed identity into the
store, or seeds an empty local store from a committed `marrow.lock`. After the
commit, the CLI regenerates `marrow.lock` from the committed store snapshot; once a
baseline exists, later identity changes flow through `evolve apply` rather than
being written from a check. There is no separate command to inspect or accept a
proposal.

A stable ID is a random opaque 128-bit value in the `cat_<32 lowercase hex>`
shape. It is allocated independently of the source path, so it never changes
when a path changes, and it is random rather than a counter so identity minted
on two branches for different entities cannot collide when they merge. Once
committed it is frozen and never recomputed. A duplicate ID — from a corrupt
catalog table or an astronomically unlikely clash — fails closed rather than
corrupting storage.

The catalog digest is `sha256:<64 lowercase hex>` over the canonical object
`{"epoch": <u64>, "entries": <entries in canonical digest order>}`. Canonical
digest order sorts entries by declaration kind tag, canonical path, stable ID,
aliases, lifecycle tag, accepted store-key shape, accepted store-index shape, and
accepted structural signature. Source/member order and catalog-row order are not
digest inputs, so a pure enum-member reorder preserves catalog identity. New
catalog writes stamp the canonical digest, and reads accept only that canonical
digest. A snapshot with a stale row-order digest or any other digest mismatch is
rejected as `catalog.invalid` before its IDs bind.

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
| Populated but unstamped (saved records, no activation stamp) | `run.store_unstamped`: run `marrow evolve preview` to inspect the required work and `marrow evolve apply` to activate the accepted catalog first. |
| Stamped epoch equals the program's, and the source digest matches | Proceeds. An apply advances the store to the proposal epoch; a run executes normally. |
| Stamped epoch equals the program's, but the source digest differs | `run.schema_drift`: the store was stamped under a structurally different schema at this epoch; run `marrow evolve preview` or `marrow evolve apply`. |
| Stamped epoch newer than the program | `run.store_evolved`: a newer binary evolved the store. Recompile or upgrade against the current accepted catalog. |
| Stamped epoch older than the program | `run.store_behind`: the store predates this catalog. Run `marrow evolve apply` to activate it to the program's epoch first. |
| Engine profile differs from the binary's layout | `run.engine_profile`: the physical storage layout has drifted. |

The catalog epoch is a coarse version number; two structurally different schemas
can share an epoch, so the source digest is the schema-bearing fence that tells
them apart.

The source digest binds the durable shape — every `resource`, `store`, `enum`,
and module constant — and not the `evolve` block. The fence governs the shape a
stored snapshot must match, not the transition that produced it, so an evolve
block can be deleted once consumed without reading as schema drift.

The source digest also uses `sha256:<64 lowercase hex>`. Its payload is the
canonical formatter's rendering of every `resource`, `store`, `enum`, and
module `const` declaration, in deterministic order, so a shape change drifts
the digest while a whitespace reformat does not.

An evolution apply writes the data/index effects and a slim commit stamp in the
same store transaction. The durable stamp records only the commit id, catalog
epoch, layout epoch, source digest, engine-profile digest, and the root/index
catalog IDs touched by that commit. The CLI receipt still reports operator
counts for defaults, transforms, retires, and rebuilt indexes, but those counts
are not persisted in commit metadata or backup descriptors. Stale replay
suppression uses the target identity facts plus the recomputed witness gate, not
per-effect counts or digests. The accepted catalog rows, the catalog epoch, the
commit metadata, and the data and index cells all advance in that one store
transaction, so a reader sees either the whole activation or none of it. There
is no separate post-commit acceptance step: after the transaction commits, the
CLI regenerates `marrow.lock` as a one-way projection of the committed store
snapshot. The committed store is the authority; the lock follows it and never
the reverse. A failure before commit rolls every effect back to the prior
accepted snapshot.

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
transform, or destructively drops populated data does not auto-apply. The run
fences with an actionable diagnostic that names `marrow evolve apply` and the
backfill count where the witness proved one. A destructive drop against
populated data stays explicit and requires confirmation through
`marrow evolve apply --maintenance --approve-retire <catalog-id>:<count>`,
naming each retired identity at its exact populated count, because losing data
must never be a silent side effect of a run; a drop whose target holds no cells
has nothing to lose and auto-applies. An evolution that needs that destructive
approval never auto-applies, whatever its record count.

The obligation probe and the activation stamp run in one transaction under the store
write lock. The witness pins the store commit id, and apply re-checks that pin inside
the write transaction, so a write that commits between the probe and the stamp moves
the commit id and fails the apply closed: the auto-apply decision reflects committed
state at stamp time and can only become more conservative under a race, never stamp
against data it no longer describes. A run advancing the epoch fences other bindings
still on the old epoch (`run.store_evolved`), the same lockout an explicit
`marrow evolve apply` produces.

## Long-Term Online Activation Direction

v0.1 activation is intentionally strict and local: one exact witness, one
transaction. The designed online path — compiler-owned activation jobs, finite
compatibility windows, and shadow-decant for large reshapes — is specified in
[future/data-evolution.md](future/data-evolution.md).

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
blocks `evolve preview` and `evolve apply`.

- typed data integrity reports `data.decode`, `data.key_type`,
  `data.dangling_ref`, `data.incomplete`, and `data.orphan` problems. It is
  read-only.
- typed data inspection renders durable places from checked/catalog facts.
- A repair function run with `--maintenance` rewrites or deletes modeled data
  through managed paths, then `evolve preview` must prove the repaired snapshot
  before activation.

There is no dedicated `marrow repair` command. Repair is a maintenance run of
your own code, verified before and after with `marrow data integrity`.

The recovery-point gate binds to Retire-bearing apply witnesses. Operators must
choose a validated backup or an explicit no-backup receipt before mutation.

## Maintenance Migration And Repair, Worked

Leaf retype is not an in-place reinterpret. Add the new member, transform it
from the old member, apply, then retire the old member under explicit
maintenance approval:

```mw
resource Book
    required pages: int
    required pageLabel: string

store ^books(id: int): Book

evolve
    transform Book.pageLabel
        return $"pages:{old.pages}"
```

```sh
marrow evolve apply ./project
```

After the transform, source can drop `pages` only by stating the destructive
intent:

```mw
resource Book
    required pageLabel: string

store ^books(id: int): Book

evolve
    retire Book.pages
```

```sh
marrow evolve apply --maintenance --approve-retire <pages-catalog-id>:<count> ./project
```

The result is byte-stable: the new `pageLabel` cell is written under its own
catalog ID, and the populated old `pages` cell is deleted under the retired
catalog ID rather than reused.

Store re-key is a copy-and-delete migration in v0.1. Keep the old store declared
while adding the new keyed store, construct non-integer target identities
explicitly, copy the modeled data, then delete the old identities:

```mw
resource Book
    required title: string

store ^books(id: int): Book
store ^booksBySlug(slug: string): Book

pub fn migrate()
    const target: Id(^booksBySlug) = Id(^booksBySlug, "book-1")
    var b: Book
    b.title = ^books(1).title ?? ""
    transaction
        ^booksBySlug(target) = b
        delete ^books(1)
```

Run that function with the maintenance posture appropriate for the migration:

```sh
marrow run --maintenance --entry books::migrate ./project
```

The old int-keyed record is gone; the new string-keyed record is addressed by
the stored identity payload for `Id(^booksBySlug, "book-1")`. The old store ID
and int key are not silently reinterpreted as the new address.

For `data.orphan`, bracket repair with integrity. First run the target source
and confirm the problem:

```sh
marrow data integrity --format json ./project
```

If the orphan is a dropped member, run a repair source that still declares that
member, delete or move the modeled data through managed paths, then return to
the target source and rerun integrity:

```mw
resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn repair()
    delete ^books(1).subtitle
```

```sh
marrow run --maintenance --entry books::repair ./project
marrow data integrity --format json ./project
```

The repair is complete only when integrity reports no problems against the
target source.

## Backup And Restore

Typed backup/restore is a separate command pair (`marrow backup` and
`marrow restore`), not a raw engine-byte copy. A backup carries a manifest, the
accepted-catalog rows, and the canonical tree-cell data stream as typed cell
targets. The manifest binds the data to `source_digest`, `catalog_epoch`,
`catalog_digest`, `state_digest`, `store_uid`, the reserved empty
`parent_snapshot_digest` sentinel, `engine`, `commit`, `record_count`, and
`archive_checksum`. Generated indexes are derived, so the stream omits them and
restore rebuilds them from the data. Restore validates that binding and the data
conditions required for activation, rejects managed cells the current
source/catalog does not declare, and replays the catalog rows and data cells into
an empty store by default, or into a counted replace target with
`restore --replace --count`, in one transaction with a fresh store UID. A
rejected replay rolls back to the target's prior state, so the restored store
re-establishes its accepted identity and runs immediately. Backups are
deterministic and portable across conforming backends at the same layout and
codec, but byte identity requires matching accepted catalog facts, engine
profile, value codec, and stored data. Stable IDs are random opaque values that
freeze when accepted, so divergent catalog histories may still freeze distinct
accepted IDs for source that looks equivalent.

To roll forward from an older backup after source has advanced, restore with the
old source tree that matches the backup's source digest. Once the old source and
backup restore cleanly, re-state the pending `evolve` intents in the project
source and apply them again to advance the restored store to the desired catalog.

## Also Deferred

These do not exist yet:

- multi-record transforms (split, merge, or a target computed from more than one
  record). The narrow per-record `evolve transform` — a pure body computing one
  saved member from a record's other, still-decodable members — is implemented; a
  reshape that crosses records is not;
- `marrow data diff` and `marrow data load` (see
  [future/data-tools.md](future/data-tools.md));
- restore merge/repair modes and cross-engine restore. Counted
  `restore --replace --count` is implemented for replacing an existing target
  whose live record count matches the supplied count.

CLI commands follow the standard contract from
[`error-codes.md`](error-codes.md): `0` on success, `1` for a recoverable check,
capability, runtime, storage, or tool failure, and `2` for a command-line usage
error before the command runs.
