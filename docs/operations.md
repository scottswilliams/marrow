# Operations

This page covers the v0.1 CLI and native store. Marrow does not ship a
background service manager; operator procedures are ordinary CLI commands over a
project directory and its configured store. `marrow serve` is a foreground
serving profile over checked application-surface routes: loopback by default,
remote only with explicit Bearer authentication. Default mode serves read
routes, including computed reads under the read route prefix. `--write`
explicitly opens create, sparse-update, delete, and action routes.
`--cors-origin` is an explicit local browser-development allow-list for one
loopback origin; remote CORS and cursor-token mode use separate explicit flags.

## Writer Model

The native store allows one write-capable open for a store file. Read-only opens
coexist with each other; a read-only open and a read-write open exclude each
other in both directions and report `store.locked`. Close the other process and
retry. See [backend-contract.md](backend-contract.md#concurrency).

Commands that can take the write side include `marrow run` when it can write
saved data or freeze identity, `marrow evolve apply`, `marrow restore`, and
`marrow data recover`. Read-only inspection commands can still block a writer
while they hold a native store open.

`marrow serve` without `--write` opens the configured native store
read-only through `ProjectSurfaceReadSession`, so it can coexist with other
read-only opens and blocks write-capable commands while it is running. With
`--write`, it opens `ProjectSurfaceSession`; the native writer lock makes that
server the owning process/session and excludes another writer or read-only
inspection handle. A `--write` start replays an unclean shutdown left by a prior
signalled writer with no interrupted commit, the same write-capable replay `run`
and `data recover` perform; neither mode auto-applies catalog drift, repairs
genuine corruption, or creates missing data.

## Deploying A Catalog Change

v0.1 supports one exact accepted catalog epoch and source digest at a time. A
binary whose checked source does not match the store fences before writing; old
and new binaries outside that exact window fail closed. See
[data-evolution.md](data-evolution.md#activation-fencing).

Use this choreography for a production catalog change:

1. Stop the current writer.
2. Run `marrow backup <projectdir> <archive>` from the currently accepted
   source as the rollback artifact.
3. Deploy the new source and binary together.
4. Run `marrow check <projectdir>`.
5. Run `marrow evolve preview <projectdir>` and inspect the witness.
6. Run `marrow evolve apply <projectdir>`. When preview names destructive
   retire work, add `--maintenance`, the required
   `--approve-retire <field-path>:<count>` arguments, and an explicit
   recovery-point decision (`--backup <archive>` or `--no-backup`); apply fails
   closed without it.
7. Start the new writer after apply succeeds.

The apply transaction advances accepted identity, data and index effects, and
commit metadata together in the live store, which is the sole write-time
authority. After commit, the CLI regenerates `marrow.lock` as a one-way
projection of the committed store snapshot; the committed lock follows the store
and never overrides it. Commit the regenerated `marrow.lock` alongside the
deployed source.

`marrow run` can auto-apply zero-record-mutation drift, but explicit
`preview`/`apply` is the deployment path when operators need a reviewed witness
and a named maintenance window.

## Diagnosing A Project

`marrow doctor <projectdir>` is the read-only triage command. It loads
`marrow.json`, runs the check summary, opens the store read-only when one exists,
and reports store, fence, and lock findings without repairing anything. When the
committed `marrow.lock` is stale or collides with the live store, `doctor` names
the regenerate step (`doctor.stale_lock`, `doctor.catalog_collision`,
`doctor.store_lock_epoch_mismatch`): the store is authoritative, so run the
program or `marrow evolve apply` to regenerate `marrow.lock`, then commit it. A
corrupt lock is reported as `doctor.lock_corrupt`; delete the corrupt
`marrow.lock` so the next run or `evolve apply` re-projects it from the
authoritative store (a run over a corrupt lock fails closed without regenerating
it).
`doctor` samples saved-data integrity within a bounded cap and names the full
`marrow data integrity` command when more is needed. See
[cli.md](cli.md#marrow-doctor) for the finding envelope.

## Commit And Epoch Lineage

The catalog epoch is a coarse store schema version. The source digest is the
schema-bearing fence: two different shapes can share an epoch, so tools compare
both.

Commit IDs are local to one store lineage. The baseline stamp is commit `0`; a
later committed write records the prior commit plus one, and rollback consumes
no commit ID. A restore mints a fresh store UID and replays catalog rows plus
data into the target, so its commit lineage is the restored store lineage, not
Git history and not the source store's physical file history.

## Store Growth And Repack

The native store is the production v0.1 durability profile. Marrow does not
ship an online vacuum or compaction command. The current repack path is typed
backup and restore:

1. `marrow backup <projectdir> <archive>`
2. prepare an empty target store for the same source tree
3. `marrow restore <projectdir> <archive>`
4. run `marrow data integrity <projectdir>`

Restore rebuilds generated indexes from the restored data. For an existing
target, use `marrow restore --replace --count N <projectdir> <archive>` only
when the live record count has been checked and the target may be replaced.

Backups are the canonical exit format. They carry the manifest, accepted catalog
rows, and canonical tree-cell data stream; they are not raw engine-file copies.
See [cli.md](cli.md#marrow-backup) and
[backend-contract.md](backend-contract.md#adapters-and-portability).

## Full-Scan Commands

The following commands intentionally scan a full saved-data snapshot:

- `marrow data stats` counts roots and records exactly.
- `marrow data dump` prints every declared data cell for operator inspection.
- `marrow data integrity` verifies every checked reachable cell and scans for
  orphaned managed cells.
- `marrow backup` traverses the canonical data-cell stream.

Run full scans outside hot paths for large stores. `marrow data get` is the
point-bounded inspection command.

## Branch Stores

Treat branch-local stores as disposable. For branch work that needs realistic
data:

1. `marrow backup <projectdir> <archive>` from the source version that owns the
   store.
2. Restore that archive into an empty branch-local store.
3. Run branch work against the restored copy.
4. Discard the branch store when the branch is done.

Do not merge native store files between branches. Source merges happen in Git;
stored data moves through backup and restore. `marrow data diff`, `marrow data
load`, restore merge modes, and cross-branch data merge are deferred; see
[future/data-tools.md](future/data-tools.md).

## Security

Marrow v0.1 has no database users-and-roles system. The security boundary is the
host process, filesystem permissions, backend credentials, and any transport the
host chooses to provide. At-rest protection is delegated to the filesystem or
selected backend.

On Unix, newly created native store files and backup archives use owner-only
`0600` modes. Existing directory permissions, backup archive handling, and
transport security remain operator responsibilities.

## Egress Regimes

Every emitting surface belongs to one regime:

| Surface | Regime | Boundary |
|---|---|---|
| `marrow run` program output, `print`, and granted `std::io` writes | Application egress | Output chosen by the program and host; not a store export or tooling protocol. |
| `marrow serve` responses and generated-client runtime output | Application egress | Checked application ABI output chosen by a declared `surface` and its boundary profile; not admin inspection, backup, repair, or raw saved-path export. |
| `std::log`, run trace, dry-run, check, test, evolve, restore receipts, and data command reports | Tooling egress | Compiler/runtime/store facts for operators and tools; message prose is not a stable API. |
| `marrow data dump`, `data get`, and `data integrity` findings | Admin inspection egress | May expose saved paths or value bytes; not a backup format, sync format, or production data API. |
| `marrow backup` archives | Portable data egress | The canonical exit format for saved data: manifest, accepted catalog rows, and typed data cells. |
| `marrow.lock`, `marrow init`, and `marrow fmt --write` | Source-tree egress | Project/source artifacts in the working tree; not saved-data export. |

Encryption is not a `.mw` language feature in v0.1. Encryption belongs in the
filesystem, disk layer, or a future backend profile; backend profiles can grow
residency, tiering, encryption, and durability facts without adding a second
source-language model.

No captivity: a project is not trapped in the native redb file. Use
`marrow backup` as the typed portable exit, and `marrow restore` to rebuild a
conforming target store from that archive.
